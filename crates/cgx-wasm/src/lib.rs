//! # cgx-wasm
//!
//! The cgx engine as a `wasm32-wasip1` reactor module (`cargo run -p xtask --
//! wasm`). The host owns git, scheduling and the store's write lock; the module
//! owns extraction, linking, persistence of the index (through `cgx-store`, over
//! the host's `.cgx/` mounted at `/.cgx`) and query evaluation. Request semantics
//! live in `cgx-session`; this crate is only the calling convention.
//!
//! ## Calling convention (ABI [`cgx_session::ABI_VERSION`])
//!
//! The host calls `_initialize` once, then:
//!
//! - `cgx_abi_version() -> i32`.
//! - `cgx_alloc(len) -> ptr` / `cgx_free(ptr, len)` — byte buffers in guest
//!   memory.
//! - every op: `fn(req_ptr, req_len) -> i64`. The guest takes ownership of the
//!   request buffer (the host must not free it) and returns
//!   `(resp_ptr << 32) | resp_len`; the host copies the response out, then calls
//!   `cgx_free(resp_ptr, resp_len)`.
//!
//! A response's first byte is the status: `0` OK, followed by the body; `1`
//! error, followed by JSON `{"kind","message"}`. A frame is `u32le count`, then
//! `count × (u32le len, bytes)`. Paths travel only as raw bytes in frames, never
//! in JSON.
//!
//! | export | request | response |
//! |---|---|---|
//! | `cgx_info` | empty | JSON `{abi, cgx_version, store_format, schema_hash, tools, ops}` |
//! | `cgx_session_open` | JSON `{head_tree, cgx_toml, dataflow}` | JSON `{state, graph_key, pointer?}` |
//! | `cgx_index_begin` | frame `[opts, path, oid, …]` | JSON `{manifest_indices, graph_key}` |
//! | `cgx_index_plan` | frame `[content per manifest index]` | frame `[summary, (u32le index ‖ ctx)…]` |
//! | `cgx_extract` | frame `[ctx, content]` | frame `[{lang, fragment_version}, facts]` |
//! | `cgx_index_submit` | frame `[(u32le index ‖ u32le fragment_version ‖ facts)…]` | `{}` |
//! | `cgx_index_finish` | frame `[opts, scip?]` | JSON index report |
//! | `cgx_query` | JSON `{tool, args}` | JSON result |
//!
//! Instances take one of two roles: a **session** instance (one per graph, with
//! `.cgx` mounted) runs everything but `cgx_extract`; **extractor** instances
//! (no filesystem) run only `cgx_extract`. A panic or allocation failure prints
//! to stderr and traps; the host discards a trapped instance.

#![deny(unsafe_code)]

use std::cell::RefCell;

use cgx_session::store_loc::cgx_dir;
use cgx_session::{BeginOpts, OpenRequest, Session, SessionError, Submitted};
use cgx_store::ObjectStore;
use serde::Deserialize;
use serde_json::{json, Value};

/// Reactor initialisation: run the module's static constructors exactly once.
/// The host calls this before any other export.
///
/// Rust links a `wasm32-wasip1` cdylib without a crt start object, so `wasm-ld`
/// sees no explicit `__wasm_call_ctors` call and wraps *every* export in a
/// command-style shim that runs the constructors before the call and the
/// destructors after it — re-running `inventory` registrations (cgx-select's
/// tokenizers) and tearing state down on each call. Referencing
/// `__wasm_call_ctors` here, as wasi-libc's `crt1-reactor.o` does, turns the
/// shims off and makes the module a reactor.
#[cfg(target_family = "wasm")]
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn _initialize() {
    use std::sync::atomic::Ordering;
    extern "C" {
        fn __wasm_call_ctors();
    }
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        std::process::abort();
    }
    // SAFETY: provided by wasm-ld; runs each constructor once, guarded above.
    unsafe { __wasm_call_ctors() }
}

/// Set once `_initialize` has run the module's constructors. Ops refuse to run
/// before that: without the constructors, registrations made at load time (the
/// selector engine's tokenizers) are missing and answers would be silently wrong.
#[cfg(target_family = "wasm")]
static INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Status byte of a successful response.
const STATUS_OK: u8 = 0;
/// Status byte of an error response.
const STATUS_ERR: u8 = 1;

/// The ops a host can call, one per export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Info,
    SessionOpen,
    IndexBegin,
    IndexPlan,
    Extract,
    IndexSubmit,
    IndexFinish,
    Query,
}

/// An instance's state: the session (store opened lazily, so an extractor
/// instance never touches a filesystem).
#[derive(Debug)]
pub struct State {
    session: Session<ObjectStore>,
}

impl State {
    /// State over the `.cgx/` under `root` (`/` in the guest).
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        State {
            session: Session::new(root, Box::new(|root| ObjectStore::open(cgx_dir(root)))),
        }
    }
}

/// Run one op on `state` and encode its response (status byte first).
pub fn handle(state: &mut State, op: Op, req: &[u8]) -> Vec<u8> {
    match dispatch(state, op, req) {
        Ok(mut body) => {
            body.insert(0, STATUS_OK);
            body
        }
        Err(e) => error_response(&e),
    }
}

fn error_response(e: &SessionError) -> Vec<u8> {
    let mut out = vec![STATUS_ERR];
    serde_json::to_writer(&mut out, e).expect("error envelope serializes");
    out
}

fn json_body<T: serde::Serialize>(v: &T) -> Vec<u8> {
    serde_json::to_vec(v).expect("response serializes")
}

fn parse_json<'a, T: Deserialize<'a>>(what: &str, bytes: &'a [u8]) -> Result<T, SessionError> {
    serde_json::from_slice(bytes)
        .map_err(|e| SessionError::invalid_params(format!("{what}: {e}")))
}

/// No fields in ABI 1; an unknown field is rejected rather than ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FinishOpts {}

#[derive(Debug, Deserialize)]
struct QueryRequest {
    tool: String,
    #[serde(default)]
    args: Option<Value>,
}

fn dispatch(state: &mut State, op: Op, req: &[u8]) -> Result<Vec<u8>, SessionError> {
    let session = &mut state.session;
    match op {
        Op::Info => {
            let tools: Vec<&str> = cgx_mcp::output::tool_names().collect();
            Ok(json_body(&json!({
                "abi": cgx_session::ABI_VERSION,
                "cgx_version": env!("CARGO_PKG_VERSION"),
                "store_format": cgx_store::manifest::CURRENT_STORE_FORMAT,
                "schema_hash": cgx_session::schema_hash(),
                "tools": tools,
                "ops": cgx_session::SESSION_OPS,
            })))
        }
        Op::SessionOpen => {
            let open: OpenRequest = parse_json("cgx_session_open request", req)?;
            Ok(json_body(&session.open(open)?))
        }
        Op::IndexBegin => {
            let frame = decode_frame(req)?;
            let (opts, rest) = frame
                .split_first()
                .ok_or_else(|| SessionError::invalid_params("cgx_index_begin: empty frame"))?;
            let opts: BeginOpts = parse_json("cgx_index_begin options", opts)?;
            if rest.len() % 2 != 0 {
                return Err(SessionError::invalid_params(
                    "cgx_index_begin: entries must be (path, oid) pairs",
                ));
            }
            let pairs: Vec<(&[u8], &[u8])> = rest.chunks_exact(2).map(|p| (p[0], p[1])).collect();
            let committed = opts.committed as usize;
            if committed > pairs.len() {
                return Err(SessionError::invalid_params(format!(
                    "cgx_index_begin: {committed} committed entries of {} pairs",
                    pairs.len()
                )));
            }
            let (working, base) = pairs.split_at(pairs.len() - committed);
            Ok(json_body(&session.index_begin(opts, working, base)?))
        }
        Op::IndexPlan => {
            let contents = decode_frame(req)?.into_iter().map(<[u8]>::to_vec).collect();
            let (summary, misses) = session.index_plan(contents)?;
            let mut entries = Vec::with_capacity(misses.len() + 1);
            entries.push(json_body(&summary));
            for (index, ctx) in misses {
                let mut e = Vec::with_capacity(4 + ctx.len());
                e.extend_from_slice(&index.to_le_bytes());
                e.extend_from_slice(&ctx);
                entries.push(e);
            }
            Ok(encode_frame(&entries))
        }
        Op::Extract => {
            let frame = decode_frame(req)?;
            let [ctx, content] = frame[..] else {
                return Err(SessionError::invalid_params(
                    "cgx_extract: frame must be [ctx, content]",
                ));
            };
            let (meta, facts) = cgx_session::extract(session.registry(), ctx, content)?;
            Ok(encode_frame(&[json_body(&meta), facts]))
        }
        Op::IndexSubmit => {
            let mut files = Vec::new();
            for e in decode_frame(req)? {
                if e.len() < 8 {
                    return Err(SessionError::invalid_params(
                        "cgx_index_submit: entry shorter than 8 bytes",
                    ));
                }
                files.push(Submitted {
                    index: u32::from_le_bytes(e[0..4].try_into().expect("4 bytes")),
                    fragment_version: u32::from_le_bytes(e[4..8].try_into().expect("4 bytes")),
                    facts: e[8..].to_vec(),
                });
            }
            session.index_submit(files)?;
            Ok(b"{}".to_vec())
        }
        Op::IndexFinish => {
            let frame = decode_frame(req)?;
            let (opts, scip) = match &frame[..] {
                [opts] => (*opts, None),
                [opts, scip] => (*opts, Some(*scip)),
                _ => {
                    return Err(SessionError::invalid_params(
                        "cgx_index_finish: frame must be [opts] or [opts, scip]",
                    ))
                }
            };
            let _: FinishOpts = parse_json("cgx_index_finish options", opts)?;
            Ok(json_body(&session.index_finish(scip)?))
        }
        Op::Query => {
            let q: QueryRequest = parse_json("cgx_query request", req)?;
            let args = q.args.unwrap_or_else(|| json!({}));
            Ok(json_body(&session.call(&q.tool, &args)?))
        }
    }
}

/// Lay `entries` out as a frame.
pub fn encode_frame(entries: &[Vec<u8>]) -> Vec<u8> {
    let len = 4 + entries.iter().map(|e| 4 + e.len()).sum::<usize>();
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in entries {
        out.extend_from_slice(&(e.len() as u32).to_le_bytes());
        out.extend_from_slice(e);
    }
    out
}

/// Split a frame into its entries (borrowing `bytes`).
pub fn decode_frame(bytes: &[u8]) -> Result<Vec<&[u8]>, SessionError> {
    let bad = |m: &str| SessionError::invalid_params(format!("malformed frame: {m}"));
    let (head, mut rest) = bytes.split_at_checked(4).ok_or_else(|| bad("shorter than its header"))?;
    let count = u32::from_le_bytes(head.try_into().expect("4 bytes")) as usize;
    if count > rest.len() / 4 {
        return Err(bad("entry count exceeds the remaining bytes"));
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let (len, tail) = rest.split_at_checked(4).ok_or_else(|| bad("entry length truncated"))?;
        let len = u32::from_le_bytes(len.try_into().expect("4 bytes")) as usize;
        let (entry, tail) = tail.split_at_checked(len).ok_or_else(|| bad("entry truncated"))?;
        out.push(entry);
        rest = tail;
    }
    if !rest.is_empty() {
        return Err(bad("trailing bytes"));
    }
    Ok(out)
}

// --- exports ------------------------------------------------------------------

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::new("/"));
}

/// The ABI version this module speaks.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn cgx_abi_version() -> i32 {
    cgx_session::ABI_VERSION as i32
}

/// Allocate `len` bytes for the host to fill; returns their address.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn cgx_alloc(len: i32) -> i32 {
    let Some(layout) = byte_layout(len) else {
        return std::ptr::NonNull::<u8>::dangling().as_ptr() as i32;
    };
    // SAFETY: `layout` has a non-zero size.
    let ptr = unsafe { std::alloc::alloc(layout) };
    if ptr.is_null() {
        std::alloc::handle_alloc_error(layout);
    }
    ptr as i32
}

/// Free a buffer from [`cgx_alloc`] or an op's response.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn cgx_free(ptr: i32, len: i32) {
    if let Some(layout) = byte_layout(len) {
        // SAFETY: the host passes back a (ptr, len) this module allocated with
        // this layout and has not freed.
        unsafe { std::alloc::dealloc(ptr as *mut u8, layout) }
    }
}

fn byte_layout(len: i32) -> Option<std::alloc::Layout> {
    let len = usize::try_from(len).ok().filter(|&l| l > 0)?;
    std::alloc::Layout::array::<u8>(len).ok()
}

/// Take ownership of the request buffer, run `op`, and hand the response over.
#[allow(unsafe_code)]
fn call(op: Op, ptr: i32, len: i32) -> i64 {
    let req: Vec<u8> = match byte_layout(len) {
        // SAFETY: the host filled a buffer from `cgx_alloc(len)` at `ptr`; its
        // ownership passes to us, and `alloc` with this layout is what a
        // `Vec<u8>` of capacity `len` uses.
        Some(_) => unsafe { Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize) },
        None => Vec::new(),
    };
    #[cfg(target_family = "wasm")]
    let initialized = INITIALIZED.load(std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(target_family = "wasm"))]
    let initialized = true;
    let resp = if initialized {
        STATE.with(|s| handle(&mut s.borrow_mut(), op, &req))
    } else {
        error_response(&SessionError::internal(
            "the host must call _initialize once before any other export",
        ))
    };
    drop(req);
    let resp = resp.into_boxed_slice();
    let len = resp.len();
    let ptr = Box::into_raw(resp) as *mut u8 as usize;
    (((ptr as u64) << 32) | len as u64) as i64
}

macro_rules! op_export {
    ($name:ident, $op:expr) => {
        #[doc = concat!("The `", stringify!($name), "` op (see the crate docs).")]
        #[allow(unsafe_code)]
        #[no_mangle]
        pub extern "C" fn $name(ptr: i32, len: i32) -> i64 {
            call($op, ptr, len)
        }
    };
}

op_export!(cgx_info, Op::Info);
op_export!(cgx_session_open, Op::SessionOpen);
op_export!(cgx_index_begin, Op::IndexBegin);
op_export!(cgx_index_plan, Op::IndexPlan);
op_export!(cgx_extract, Op::Extract);
op_export!(cgx_index_submit, Op::IndexSubmit);
op_export!(cgx_index_finish, Op::IndexFinish);
op_export!(cgx_query, Op::Query);

#[cfg(test)]
mod tests {
    use super::*;
    use cgx_index::Repo;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn fixtures_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    fn copy_dir(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let to = dst.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                if entry.file_name() != ".cgx" {
                    copy_dir(&entry.path(), &to);
                }
            } else {
                std::fs::copy(entry.path(), &to).unwrap();
            }
        }
    }

    fn git(repo: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["-c", "user.name=cgx-test", "-c", "user.email=cgx@test.invalid"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
            .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
            .status()
            .expect("run git")
            .success();
        assert!(ok, "git {args:?}");
    }

    /// Every language fixture in one repository, each under its own directory.
    fn mixed_repo(dir: &Path) -> PathBuf {
        let repo = dir.join("repo");
        for f in ["rust-sample", "go", "python", "java", "ts"] {
            copy_dir(&fixtures_root().join(f), &repo.join(f));
        }
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "fixture"]);
        repo
    }

    fn ok(resp: Vec<u8>) -> Vec<u8> {
        assert_eq!(resp[0], STATUS_OK, "error: {}", String::from_utf8_lossy(&resp[1..]));
        resp[1..].to_vec()
    }

    fn ok_json(resp: Vec<u8>) -> Value {
        serde_json::from_slice(&ok(resp)).unwrap()
    }

    fn err_kind(resp: Vec<u8>) -> String {
        assert_eq!(resp[0], STATUS_ERR);
        let v: Value = serde_json::from_slice(&resp[1..]).unwrap();
        v["kind"].as_str().unwrap().to_string()
    }

    /// The `.cgx` files a store byte-identity check compares.
    fn store_files(repo: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd {
                let e = e.unwrap();
                if e.file_type().unwrap().is_dir() {
                    walk(root, &e.path(), out);
                } else {
                    let rel = e.path().strip_prefix(root).unwrap().to_string_lossy().into_owned();
                    out.insert(rel, std::fs::read(e.path()).unwrap());
                }
            }
        }
        let cgx = repo.join(".cgx");
        let mut out = BTreeMap::new();
        for sub in ["objects", "fragments", "refs"] {
            walk(&cgx, &cgx.join(sub), &mut out);
        }
        out.insert("HEAD.json".into(), std::fs::read(cgx.join("HEAD.json")).unwrap());
        out
    }

    /// Index `repo` through the export dispatch exactly as a host does: the
    /// session state gets the file list and manifest contents, a separate
    /// extractor state extracts each miss, results come back in batches.
    fn index_through_exports(session: &mut State, repo: &Path) -> Value {
        let git = Repo::discover(repo).unwrap();
        let tree = git.head_tree_oid().unwrap();
        let files = git.enumerate_tree().unwrap();
        let opts = json!({"mode": "head", "graph_key": tree, "dataflow": null});
        drive_index(session, opts, files, Vec::new())
    }

    /// `mode: "worktree"` through the exports, with the walk and committed list
    /// a host would send (here from the native git layer, so both sides of a
    /// comparison see the same working set).
    fn worktree_through_exports(session: &mut State, repo: &Path) -> Value {
        let git = Repo::discover(repo).unwrap();
        let tree = git.head_tree_oid().unwrap();
        let working = git.enumerate_workdir(repo).unwrap();
        let committed: Vec<(String, String)> = git.tree_blob_oids(&tree).unwrap().into_iter().collect();
        let opts = json!({"mode": "worktree", "head_tree": tree, "committed": committed.len()});
        drive_index(session, opts, working, committed)
    }

    fn drive_index(
        session: &mut State,
        opts: Value,
        mut files: Vec<cgx_index::SourceFile>,
        committed: Vec<(String, String)>,
    ) -> Value {
        files.reverse(); // the host's order is irrelevant
        let mut begin = vec![serde_json::to_vec(&opts).unwrap()];
        for f in &files {
            begin.push(f.rel_path.as_bytes().to_vec());
            begin.push(f.blob_oid.as_bytes().to_vec());
        }
        for (path, oid) in &committed {
            begin.push(path.as_bytes().to_vec());
            begin.push(oid.as_bytes().to_vec());
        }
        let r = ok_json(handle(session, Op::IndexBegin, &encode_frame(&begin)));
        let manifests: Vec<usize> = serde_json::from_value(r["manifest_indices"].clone()).unwrap();
        let contents: Vec<Vec<u8>> = manifests.iter().map(|&i| files[i].content.clone()).collect();

        let plan = ok(handle(session, Op::IndexPlan, &encode_frame(&contents)));
        let plan = decode_frame(&plan).unwrap();
        let summary: Value = serde_json::from_slice(plan[0]).unwrap();
        assert_eq!(summary["to_extract"].as_u64().unwrap() as usize, plan.len() - 1);

        let mut extractor = State::new("/nonexistent");
        let mut batch = Vec::new();
        for entry in &plan[1..] {
            let index = u32::from_le_bytes(entry[..4].try_into().unwrap());
            let ctx = entry[4..].to_vec();
            let req = encode_frame(&[ctx, files[index as usize].content.clone()]);
            let out = ok(handle(&mut extractor, Op::Extract, &req));
            let out = decode_frame(&out).unwrap();
            let meta: Value = serde_json::from_slice(out[0]).unwrap();
            let mut e = index.to_le_bytes().to_vec();
            e.extend_from_slice(&(meta["fragment_version"].as_u64().unwrap() as u32).to_le_bytes());
            e.extend_from_slice(out[1]);
            batch.push(e);
            if batch.len() == 7 {
                ok(handle(session, Op::IndexSubmit, &encode_frame(&batch)));
                batch.clear();
            }
        }
        ok(handle(session, Op::IndexSubmit, &encode_frame(&batch)));
        ok_json(handle(session, Op::IndexFinish, &encode_frame(&[b"{}".to_vec()])))
    }

    fn query(state: &mut State, tool: &str, args: Value) -> Value {
        let req = serde_json::to_vec(&json!({"tool": tool, "args": args})).unwrap();
        ok_json(handle(state, Op::Query, &req))
    }

    /// Store byte identity on the composition: the host-driven index through the export dispatch
    /// writes the same `.cgx/{objects,fragments,refs}` and `HEAD.json` bytes as
    /// the native in-process index, answers the same, and warm-opens.
    #[test]
    fn exports_and_native_index_write_identical_stores() {
        let tmp = tempfile::tempdir().unwrap();
        let a = mixed_repo(&tmp.path().join("a"));
        let b = mixed_repo(&tmp.path().join("b"));
        let tree = Repo::discover(&a).unwrap().head_tree_oid().unwrap();

        let mut native = cgx_session::Session::new(&a, Box::new(|r| ObjectStore::open(cgx_dir(r))));
        let native_report = native
            .index_native(cgx_session::Mode::Head, None, None, true)
            .unwrap();

        let mut host = State::new(&b);
        let info = ok_json(handle(&mut host, Op::Info, &[]));
        assert_eq!(info["abi"], json!(cgx_session::ABI_VERSION));
        assert_eq!(info["schema_hash"], json!(cgx_session::schema_hash()));
        let open = ok_json(handle(&mut host, Op::SessionOpen, br#"{"head_tree": null}"#));
        assert_eq!(open["state"], "missing");
        let report = index_through_exports(&mut host, &b);
        assert_eq!(report["graph_key"], json!(tree));
        assert_eq!(report["stats"]["nodes"], json!(native_report.stats.as_ref().unwrap().nodes));
        assert!(report["stats"]["blobs_extracted"].as_u64().unwrap() > 0);

        let (sa, sb) = (store_files(&a), store_files(&b));
        assert!(sa.len() > 10);
        let differ: Vec<&String> = sa
            .keys()
            .chain(sb.keys())
            .filter(|k| sa.get(*k) != sb.get(*k))
            .collect();
        assert!(differ.is_empty(), "stores differ at {differ:?}");

        for (tool, args) in [
            ("symbols", json!({"rank": "inbound", "max_results": 5})),
            ("search", json!({"all": true, "max_results": 3})),
            ("export_edges", json!({"max_edges": 25})),
        ] {
            let want = native.call(tool, &args).unwrap();
            assert_eq!(query(&mut host, tool, args), want, "{tool}");
        }

        // Warm open in a fresh instance: fresh, nothing to extract on re-plan.
        let mut warm = State::new(&b);
        let req = serde_json::to_vec(&json!({"head_tree": tree})).unwrap();
        let open = ok_json(handle(&mut warm, Op::SessionOpen, &req));
        assert_eq!(open["state"], "fresh");
        assert_eq!(
            query(&mut warm, "symbols", json!({})),
            native.call("symbols", &json!({})).unwrap()
        );
        let again = index_through_exports(&mut warm, &b);
        assert_eq!(again["stats"]["blobs_extracted"], json!(0));
        assert_eq!(store_files(&b), sb, "a re-index of the same tree rewrites nothing");

        // Worktree mode after a HEAD index, on unmodified checkouts: the store
        // under `.cgx/` is not part of the working set, so the tree is clean and
        // both drivers key it identically.
        let native_wt = native.index_native(cgx_session::Mode::Worktree, None, None, false).unwrap();
        let host_wt = worktree_through_exports(&mut host, &b);
        assert_eq!(host_wt["graph_key"], json!(native_wt.graph_key));
        let answer = query(&mut host, "symbols", json!({"max_results": 3}));
        assert_eq!(answer, native.call("symbols", &json!({"max_results": 3})).unwrap());
        assert_eq!(answer["dirty"], json!(false));
        assert_eq!(answer["graph_version"], json!(&tree[..7]));
    }

    /// A rejected submit aborts the index: nothing can be finished without the
    /// file, and nothing is written.
    #[test]
    fn a_rejected_submit_fails_the_index_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = mixed_repo(tmp.path());
        let git = Repo::discover(&repo).unwrap();
        let tree = git.head_tree_oid().unwrap();
        let files = git.enumerate_tree().unwrap();
        let mut s = State::new(&repo);

        let mut begin = vec![serde_json::to_vec(&json!({"mode": "head", "graph_key": tree})).unwrap()];
        for f in &files {
            begin.push(f.rel_path.as_bytes().to_vec());
            begin.push(f.blob_oid.as_bytes().to_vec());
        }
        let r = ok_json(handle(&mut s, Op::IndexBegin, &encode_frame(&begin)));
        let manifests: Vec<usize> = serde_json::from_value(r["manifest_indices"].clone()).unwrap();
        let contents: Vec<Vec<u8>> = manifests.iter().map(|&i| files[i].content.clone()).collect();
        let plan = ok(handle(&mut s, Op::IndexPlan, &encode_frame(&contents)));
        let plan = decode_frame(&plan).unwrap();
        let mut extractor = State::new("/nonexistent");
        let submit = |index: u32, version: u32, facts: &[u8]| {
            let mut e = index.to_le_bytes().to_vec();
            e.extend_from_slice(&version.to_le_bytes());
            e.extend_from_slice(facts);
            encode_frame(&[e])
        };
        let entry = plan[1];
        let index = u32::from_le_bytes(entry[..4].try_into().unwrap());
        let out = ok(handle(
            &mut extractor,
            Op::Extract,
            &encode_frame(&[entry[4..].to_vec(), files[index as usize].content.clone()]),
        ));
        let out = decode_frame(&out).unwrap();
        let meta: Value = serde_json::from_slice(out[0]).unwrap();
        let version = meta["fragment_version"].as_u64().unwrap() as u32;

        // A wrong fragment version, and (on a fresh plan) non-canonical bytes.
        assert_eq!(err_kind(handle(&mut s, Op::IndexSubmit, &submit(index, version + 1, out[1]))), "invalid_params");
        let finish = encode_frame(&[b"{}".to_vec()]);
        assert_eq!(err_kind(handle(&mut s, Op::IndexFinish, &finish)), "invalid_params");
        assert!(!repo.join(".cgx/HEAD.json").exists());

        ok_json(handle(&mut s, Op::IndexBegin, &encode_frame(&begin)));
        ok(handle(&mut s, Op::IndexPlan, &encode_frame(&contents)));
        let mut padded = out[1].to_vec();
        padded.push(0);
        assert_eq!(err_kind(handle(&mut s, Op::IndexSubmit, &submit(index, version, &padded))), "invalid_params");
        assert_eq!(err_kind(handle(&mut s, Op::IndexFinish, &finish)), "invalid_params");
        assert!(!repo.join(".cgx/HEAD.json").exists());
    }

    #[test]
    fn misuse_is_reported_not_trapped() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = State::new(tmp.path());
        assert_eq!(err_kind(handle(&mut s, Op::IndexPlan, &encode_frame(&[]))), "invalid_params");
        assert_eq!(err_kind(handle(&mut s, Op::IndexBegin, b"\x01")), "invalid_params");
        assert_eq!(
            err_kind(handle(&mut s, Op::Query, br#"{"tool":"callers","args":{"symbol":"x"}}"#)),
            "stale"
        );
        assert_eq!(err_kind(handle(&mut s, Op::Query, br#"{"tool":"export_edges"}"#)), "stale");
        let finish = encode_frame(&[br#"{"surprise":1}"#.to_vec()]);
        assert_eq!(err_kind(handle(&mut s, Op::IndexFinish, &finish)), "invalid_params");
        let key = "0".repeat(40);
        for (graph_key, oid) in [
            (key.as_str(), "a"),                          // too short to name a store path
            (key.as_str(), &*"A".repeat(40)),             // uppercase
            (key.as_str(), &*"0".repeat(41)),             // wrong length
            ("t", &*"0".repeat(40)),                      // graph key is not an OID
        ] {
            let opts = serde_json::to_vec(&json!({"mode": "head", "graph_key": graph_key})).unwrap();
            let frame = encode_frame(&[opts, b"a.rs".to_vec(), oid.as_bytes().to_vec()]);
            assert_eq!(err_kind(handle(&mut s, Op::IndexBegin, &frame)), "invalid_params", "{oid}");
        }
    }

    #[test]
    fn frames_round_trip_and_reject_malformed_input() {
        let entries = vec![b"".to_vec(), b"abc".to_vec(), vec![0xff; 3]];
        let f = encode_frame(&entries);
        assert_eq!(decode_frame(&f).unwrap(), entries.iter().map(Vec::as_slice).collect::<Vec<_>>());
        assert!(decode_frame(&f[..f.len() - 1]).is_err());
        let mut trailing = f.clone();
        trailing.push(0);
        assert!(decode_frame(&trailing).is_err());
        assert!(decode_frame(&[9, 0, 0, 0]).is_err());
        assert!(decode_frame(&[]).is_err());
    }
}
