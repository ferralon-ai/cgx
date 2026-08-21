//! A thin `std::process::Command` wrapper over the `git` binary on `PATH`.
//!
//! No libgit2 / no `git2` crate (criterion 6): the transport uses only ancient,
//! stable plumbing — `hash-object`, `mktree`, `commit-tree`, `update-ref`,
//! `push`/`fetch`, `cat-file`, `ls-tree`, `rev-parse`, `config`. Every non-zero
//! exit is turned into [`TransportError::Git`] carrying the failing command and
//! git's stderr, so a plumbing fault is diagnosable without a re-run.
//!
//! Idiom mirrors the one prod subprocess-git call site in the tree
//! (`cgx-cli/src/main.rs:2230`): `git -C <root> <args…>`, checked for success.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::error::TransportError;

/// The compare-and-swap sentinel for the first write of a ref that does not yet
/// exist: `git update-ref <name> <new> <expected>` with 40 zeros as `expected`
/// succeeds only if the ref is currently absent.
pub(crate) const ZERO_OID: &str = "0000000000000000000000000000000000000000";

/// One entry fed to `git mktree`: `<mode> <kind> <oid>\t<name>`.
pub(crate) struct TreeEntry {
    /// `"100644"` for a blob, `"040000"` for a subtree.
    pub mode: &'static str,
    /// `"blob"` or `"tree"`.
    pub kind: &'static str,
    /// The git object id (blob or tree) this entry points at.
    pub oid: String,
    /// The entry name (a single path component — no `/`).
    pub name: String,
}

/// A `git` invoker rooted at a repository working tree.
pub(crate) struct Git {
    root: PathBuf,
}

impl Git {
    pub(crate) fn new(root: impl AsRef<Path>) -> Self {
        Git {
            root: root.as_ref().to_path_buf(),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new("git");
        c.arg("-C").arg(&self.root).args(args);
        c
    }

    /// Run `git <args>` (optionally piping `stdin` and setting `env`), requiring a
    /// zero exit. Returns raw stdout bytes.
    fn exec(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        env: &[(&str, &str)],
    ) -> Result<Vec<u8>, TransportError> {
        let out = self.output(args, stdin, env)?;
        if !out.status.success() {
            return Err(TransportError::Git {
                command: format!("git {}", args.join(" ")),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(out.stdout)
    }

    /// Run `git <args>` and return the raw [`Output`] without asserting success —
    /// for verbs (`rev-parse`, `config --get-all`) whose non-zero exit is a normal
    /// "absent" answer, not a failure.
    fn output(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        env: &[(&str, &str)],
    ) -> Result<Output, TransportError> {
        let mut cmd = self.command(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| TransportError::Git {
            command: format!("git {}", args.join(" ")),
            stderr: format!("failed to spawn git: {e}"),
        })?;
        if let Some(bytes) = stdin {
            child
                .stdin
                .take()
                .expect("stdin was piped")
                .write_all(bytes)?;
            // stdin drops here, closing the pipe so git can proceed.
        }
        child
            .wait_with_output()
            .map_err(|e| TransportError::Git {
                command: format!("git {}", args.join(" ")),
                stderr: format!("failed to collect git output: {e}"),
            })
    }

    /// `git hash-object -w --stdin` — write `bytes` verbatim into the object DB as
    /// a blob, returning the **git blob OID** (which includes git's `blob <len>\0`
    /// framing, so it differs from the cgx object OID of the same bytes).
    pub(crate) fn hash_object_w(&self, bytes: &[u8]) -> Result<String, TransportError> {
        parse_oid(self.exec(&["hash-object", "-w", "--stdin"], Some(bytes), &[])?)
    }

    /// `git mktree` — assemble `entries` into a tree object, returning its OID.
    pub(crate) fn mktree(&self, entries: &[TreeEntry]) -> Result<String, TransportError> {
        let mut input = Vec::new();
        for e in entries {
            // <mode> <kind> <oid>\t<name>\n — names here are hex OIDs / known
            // literals, never containing a tab or newline.
            let _ = writeln!(input, "{} {} {}\t{}", e.mode, e.kind, e.oid, e.name);
        }
        parse_oid(self.exec(&["mktree"], Some(&input), &[])?)
    }

    /// `git commit-tree <tree>` with **fully pinned identity/date/message and no
    /// parent** — so the commit OID is byte-identical across pushes of the same
    /// tree (criterion 7 / D-c). GPG signing is disabled so a user's
    /// `commit.gpgsign` cannot perturb determinism.
    pub(crate) fn commit_tree(&self, tree: &str) -> Result<String, TransportError> {
        let env = [
            ("GIT_AUTHOR_NAME", "cgx"),
            ("GIT_AUTHOR_EMAIL", "cgx@localhost"),
            ("GIT_AUTHOR_DATE", "@0 +0000"),
            ("GIT_COMMITTER_NAME", "cgx"),
            ("GIT_COMMITTER_EMAIL", "cgx@localhost"),
            ("GIT_COMMITTER_DATE", "@0 +0000"),
        ];
        parse_oid(self.exec(
            &["commit-tree", tree, "-m", "cgx index", "--no-gpg-sign"],
            None,
            &env,
        )?)
    }

    /// `git update-ref <name> <new> <expected>` — compare-and-swap. `expected` is
    /// [`ZERO_OID`] for the first (create) write. A lost CAS surfaces as
    /// [`TransportError::Git`]; the caller re-reads and retries.
    pub(crate) fn update_ref_cas(
        &self,
        name: &str,
        new: &str,
        expected: &str,
    ) -> Result<(), TransportError> {
        self.exec(&["update-ref", name, new, expected], None, &[])?;
        Ok(())
    }

    /// `git push <remote> <refspec>`.
    pub(crate) fn push(&self, remote: &str, refspec: &str) -> Result<(), TransportError> {
        self.exec(&["push", remote, refspec], None, &[])?;
        Ok(())
    }

    /// `git fetch <remote> <refspec>`.
    pub(crate) fn fetch(&self, remote: &str, refspec: &str) -> Result<(), TransportError> {
        self.exec(&["fetch", remote, refspec], None, &[])?;
        Ok(())
    }

    /// `git ls-remote <remote> <ref>` — whether `remote` advertises `refname`.
    /// Returns `Ok(false)` on a successful-but-empty listing (the ref is genuinely
    /// absent, distinct from a fetch that hard-errors on a missing ref); a
    /// non-zero exit (unreachable remote, auth) propagates as [`TransportError::Git`].
    pub(crate) fn ls_remote_has(&self, remote: &str, refname: &str) -> Result<bool, TransportError> {
        let out = self.exec(&["ls-remote", remote, refname], None, &[])?;
        Ok(!out.iter().all(u8::is_ascii_whitespace))
    }

    /// `git cat-file blob <oid>` — the blob's exact bytes.
    pub(crate) fn cat_file_blob(&self, oid: &str) -> Result<Vec<u8>, TransportError> {
        self.exec(&["cat-file", "blob", oid], None, &[])
    }

    /// `git ls-tree -r -z <ref>` — every blob under the tree, as
    /// `(full relative path, git blob oid)`. `-r` recurses (so only blobs are
    /// listed) and `-z` NUL-delimits records to sidestep path quoting.
    pub(crate) fn ls_tree_r(&self, refname: &str) -> Result<Vec<(String, String)>, TransportError> {
        let out = self.exec(&["ls-tree", "-r", "-z", refname], None, &[])?;
        let text = String::from_utf8(out).map_err(|_| TransportError::NonUtf8("ls-tree".into()))?;
        let mut rows = Vec::new();
        for record in text.split('\0').filter(|r| !r.is_empty()) {
            // "<mode> <type> <oid>\t<path>"
            let (meta, path) = record
                .split_once('\t')
                .ok_or_else(|| TransportError::NonUtf8("ls-tree record".into()))?;
            let oid = meta
                .split_whitespace()
                .nth(2)
                .ok_or_else(|| TransportError::NonUtf8("ls-tree meta".into()))?;
            rows.push((path.to_owned(), oid.to_owned()));
        }
        Ok(rows)
    }

    /// `git rev-parse --verify --quiet <ref>` — the ref's OID, or `None` if the ref
    /// does not resolve (a normal answer, not an error).
    pub(crate) fn rev_parse(&self, refname: &str) -> Result<Option<String>, TransportError> {
        let out = self.output(&["rev-parse", "--verify", "--quiet", refname], None, &[])?;
        if !out.status.success() {
            return Ok(None);
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        Ok((!s.is_empty()).then_some(s))
    }

    /// `git config --get-all <key>` — all configured values (empty if the key is
    /// unset; a missing key exits non-zero, which is a normal "none" answer).
    pub(crate) fn config_get_all(&self, key: &str) -> Result<Vec<String>, TransportError> {
        let out = self.output(&["config", "--get-all", key], None, &[])?;
        if !out.status.success() {
            return Ok(Vec::new());
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty())
            .collect())
    }

    /// `git config --add <key> <value>`.
    pub(crate) fn config_add(&self, key: &str, value: &str) -> Result<(), TransportError> {
        self.exec(&["config", "--add", key, value], None, &[])?;
        Ok(())
    }
}

/// Trim git's trailing newline from a one-line OID output.
fn parse_oid(out: Vec<u8>) -> Result<String, TransportError> {
    let s = String::from_utf8(out).map_err(|_| TransportError::NonUtf8("oid".into()))?;
    Ok(s.trim().to_owned())
}
