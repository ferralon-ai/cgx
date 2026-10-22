# cgx push

Publish the local index to a git remote under the non-branch ref `refs/cgx/index`.

## Synopsis

```
cgx push [OPTIONS] [REMOTE]
```

## Description

`cgx push` shares a locally built index over git's own transport, so a clone or CI runner can [`cgx pull`](pull.md) it instead of indexing from scratch.

It takes the committable part of `.cgx/` — `objects/`, `refs/`, and `HEAD.json` — writes it into the repository's git object database, and wraps it in a **deterministic, parentless commit**: fixed author and committer (`cgx <cgx@localhost>`), fixed timestamp (1970-01-01T00:00:00Z), fixed message (`cgx index`). Two pushes of an unchanged index produce the same commit OID. The local `refs/cgx/index` is advanced to that commit by compare-and-swap, then `cgx push` runs:

```
git push <remote> +refs/cgx/index:refs/cgx/index
```

It never writes `refs/heads/*` and creates no commit on any branch, so `git log`, `git blame`, and branch diffs are unaffected. `git log --all`, which walks every ref, does list the commit.

`push` does not index. Run [`cgx index`](index.md) first; with nothing indexed it exits 3. It does not check that the index matches `HEAD` — it publishes whatever `.cgx/` holds.

The local ref is advanced *before* the network push. If `git push` fails (no such remote, rejected ref, no network), `refs/cgx/index` is still updated locally and the command exits 3; rerunning after fixing the remote pushes the same commit.

Some hosts reject pushes outside `refs/heads/*` and `refs/tags/*`. See [the sharing section of 06-indexing-and-vcs.md](../06-indexing-and-vcs.md) for the portability posture.

**No approximation contract, no freshness envelope.** `push` moves bytes; it answers no query.

## Arguments

| Argument | Required | Meaning |
|----------|----------|---------|
| `REMOTE` | no | Remote name to push to. Defaults to `origin`. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the indexed repository. |

## Examples

### First push, then an unchanged re-push

```
cgx index .
cgx push
```

```
pushed 217 object(s) to origin: refs/cgx/index advanced to 652d47172c4edf622278c059921b73daabc199fc
```

Running `cgx push` again without re-indexing:

```
pushed 217 object(s) to origin: refs/cgx/index already at 652d47172c4edf622278c059921b73daabc199fc
```

Captured on a fresh copy of `fixtures/rust-sample` with a local bare repository as `origin`. The remote then advertises the ref:

```
git ls-remote origin refs/cgx/index
```

```
652d47172c4edf622278c059921b73daabc199fc	refs/cgx/index
```

### Nothing indexed

```
cgx push
```

```
cgx: nothing indexed in .cgx — run `cgx index` before pushing
```

Exit 3.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Ref advanced (or already current) and pushed. |
| `2` | Bad argument or malformed option. |
| `3` | Nothing indexed; the local compare-and-swap lost a race; or the underlying `git` command failed — the message quotes the git command and its stderr. |

## See also

- [cgx pull](pull.md) — fetch and materialize a pushed index
- [cgx configure-remote](configure-remote.md) — make plain `git fetch`/`git push` carry `refs/cgx/*`
- [06-indexing-and-vcs.md](../06-indexing-and-vcs.md) — the object model and the `refs/cgx/index` overlay
