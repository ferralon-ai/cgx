# cgx pull

Fetch `refs/cgx/index` from a git remote and materialize `.cgx/` locally.

## Synopsis

```
cgx pull [OPTIONS] [REMOTE]
```

## Description

`cgx pull` is the receiving half of [`cgx push`](push.md). It runs

```
git fetch <remote> +refs/cgx/index:refs/cgx/index
```

then writes the committed objects into `.cgx/objects` and `.cgx/refs`, and finally `.cgx/HEAD.json`. Objects are written before the pointer, so an interrupted pull never leaves `HEAD.json` naming an object that is not on disk. The result is byte-identical to what the pusher had: the same objects and the same `HEAD.json`.

**Every object is re-hashed.** Each object's bytes are checked against its OID as it is written. A mismatch is a hard error (exit 3), never served.

**An absent ref fails loudly.** If the remote has no `refs/cgx/index` — nobody has pushed, or the wrong remote was named — `pull` exits 3 with the refspec fix in the message. It never returns an empty, successful pull.

Because `pull` passes an explicit refspec, it works without [`cgx configure-remote`](configure-remote.md) having been run.

`pull` brings the graph, not the per-blob extraction caches (`fragments/`, `cache.db`), which are not part of the committable set. Queries run against the pulled graph immediately, including with `--no-auto-index`. The next `cgx index` on the same tree re-extracts every file once to rebuild those caches.

**Run `cgx index` after a pull before trusting `freshness:`.** `pull` does not write the self-ignoring `.cgx/.gitignore` that `cgx index` writes, so `.cgx/` shows up as untracked and every query reports the index as stale even when the pulled tree matches `HEAD`:

```
freshness: stale | indexed tree c19123d, 220 dirty files
```

Running `cgx index` once writes the `.gitignore`, and freshness then reads `current`.

**No approximation contract, no freshness envelope.** `pull` moves bytes; it answers no query.

## Arguments

| Argument | Required | Meaning |
|----------|----------|---------|
| `REMOTE` | no | Remote name to pull from. Defaults to `origin`. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--repo` | path | CWD | Path to the repository to materialize `.cgx/` in. |

## Examples

### Pull into a fresh clone

```
git clone <url> work && cd work
cgx pull
```

```
pulled from origin: materialized 217 object(s) across 1 graph(s)
```

Captured on a clone of a `fixtures/rust-sample` repository whose index had been pushed with [`cgx push`](push.md). The clone's `.cgx/` then holds `HEAD.json`, `objects/`, and `refs/`, byte-identical to the pusher's. Pulling again prints the same line.

### Remote has no index

```
cgx pull
```

```
cgx: refs/cgx/index absent after fetch — configure the fetch refspec (`cgx configure-remote`, or `git config --add remote.<remote>.fetch +refs/cgx/*:refs/cgx/*`) and pull again
```

Exit 3. The refspec hint is generic; the usual cause is that nobody has run `cgx push` against that remote yet.

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Index fetched and materialized. |
| `2` | Bad argument or malformed option. |
| `3` | The remote has no `refs/cgx/index`; an object failed its hash check; or the underlying `git` command failed. |

## See also

- [cgx push](push.md) — publish the local index
- [cgx configure-remote](configure-remote.md) — make plain `git fetch` carry `refs/cgx/*`, including on CI runners
- [06-indexing-and-vcs.md](../06-indexing-and-vcs.md) — the object model and the `refs/cgx/index` overlay
