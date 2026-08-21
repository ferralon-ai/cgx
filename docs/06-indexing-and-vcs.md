# 06 — Indexing and VCS Integration

**Status:** Mixed — IX-1, IX-2, IX-3 and IX-9 are substantially shipped (see the IX-0 envelope section below and the per-section status notes); IX-4 through IX-8 are design ahead of implementation, and IX-8's shipped location is *not* the one it decides on. IX-8's transport overlay (`cgx push`/`cgx pull`, sharing the index via `refs/cgx/index`) is shipped independently of the location decision — see below.
**Audience:** Engineers building or operating `cgx`; contributors; CI/CD integrators
**Cross-references:** docs/03-code-graph-model.md (GM-) · docs/09-architecture.md (AR-) · docs/05-queries.md

---

## Overview

`cgx` maintains a persistent, content-addressed index that stays current
automatically. No daemon runs in the background. On every query invocation,
`cgx` checks whether the index reflects the current HEAD; if not, it re-indexes
only the files that changed, then answers the query. Explicit commands exist for
eager indexing and for pruning stale entries, but normal use requires neither.

This document specifies:

- IX-0: The freshness envelope — how every answer reports its own index currency
- IX-1: Content-addressed index structure (blob OID and tree OID layers)
- IX-2: Per-query staleness check and synchronous delta re-index
- IX-3: Dirty-working-tree handling
- IX-4: Branch awareness and cross-branch queries
- IX-5: Branch lifecycle and garbage collection
- IX-6: Worktree model
- IX-7: Concurrent CLI access
- IX-8: Index location
- IX-9: Edge age and author attribution
- IX-10: Index-free history queries (`cgx coupling`)

---

## IX-0: The Freshness Envelope

*(Runs shown here are against the shared example repository defined in [`docs/05-queries.md` → "The example repository"](05-queries.md#the-example-repository); its recipe reproduces tree OID `7380e245ab98db2dfed12ed3ee3b005f0fbf7036` exactly.)*

Index currency is not something a reader has to infer from a warning or a timestamp. **Every
answer that consults the index reports its own freshness**, as the last line of `human` output,
as a `freshness` object in `json`, and as a `cgx/index-freshness` note in `sarif`.

```
freshness: current | indexed tree 7380e24, working tree clean
freshness: stale   | indexed tree 7380e24, 1 dirty file
freshness: unknown | indexed tree unknown (HEAD unknown), working tree not inspected
```

```json
"freshness": {
  "indexed_tree": "7380e245ab98db2dfed12ed3ee3b005f0fbf7036",
  "head_tree":    "7380e245ab98db2dfed12ed3ee3b005f0fbf7036",
  "matches_head": true,
  "dirty_files_base": "7380e245ab98db2dfed12ed3ee3b005f0fbf7036",
  "dirty_files":  0,
  "stale":        false
}
```

### The verdict word is conservative by construction

`current` requires **both** halves to have been established clean: the indexed tree matches
HEAD's tree, *and* the working tree was inspected and found undivergent. Anything left
uninspected reads `unknown`, never `current`. An envelope never claims currency it did not check.

### `matches_head` is three-valued, and `null` is the ordinary case over MCP

`true` / `false` / `null`. Over MCP the default is `include_dirty: true`, and under that default
**any indexed repository returns `null`** with a verdict of `unknown` — because the working-tree
enumeration and the dirty-file count apply different ignore rules and disagree. A freshly
indexed, clean repository reports `"matches_head": null`, `"dirty": true`,
`"dirty_files_analyzed": 3` over MCP while the CLI reports `matches_head: true` and a `current`
verdict for the same tree.

**A client that treats `matches_head` as a boolean is wrong in the common case, not the edge
case.** Read the verdict, or read `stale`; treat `null` as "not established," never as "false."

### `--at` changes what `dirty_files` counts

Without `--at`, `dirty_files` is uncommitted changes. **Under `--at <ref>`, divergence is
measured against the *pinned* tree**, so files changed by commits between that ref and the
working tree count as dirty and the verdict reads `stale`. That is correct — the answer really
was computed against a tree the working directory no longer matches — but "dirty_files =
uncommitted changes" is only true without `--at`.

### Which commands carry it

Not all of them, and the exceptions are principled rather than oversights:

| Surface | Freshness | Why |
|---|---|---|
| `callers`, `callees`, `reaches`, `paths`, `unused`, `flows-to`, `flows-from`, `query` | yes | answered from the index |
| `explain`, `search`, `symbols` | yes | read the index, so its currency is in question |
| `coupling` | **no** | reads committed git history only; never opens the index (IX-10) |
| `diff` (both modes) | **no** | both sides come from committed trees |
| `doctor` | **no** | reports *on* the index rather than from it |
| any command under `--format dot\|mermaid\|d2` | **omitted by design** | emitting a graph does not justify a working-tree walk, so an uninspected envelope is substituted |

The freshness envelope is the audit identity for a CLI answer: `indexed_tree` plus `head_tree`
is enough to reproduce the exact graph an answer came from. There is no separate
`graph_version` key on the CLI. The **MCP** surface does carry one
(`"7380e24+dirty.0a12fb8d97d6"` — indexed tree plus an overlay digest when a working-tree
overlay is in play).

---

## IX-1: Content-Addressed Index Structure

### Two-layer model

The index is split into two layers keyed by distinct git object identifiers.

**Layer 1 — Per-file facts (keyed by blob OID):**
Each source file, at every version, is identified by its git blob OID (the
SHA-1 or SHA-256 content hash of the file). `cgx` computes and stores per-file
semantic facts — symbol definitions, local call edges, scope structure — under
the blob OID as the primary cache key.

Consequence: if a file is unchanged across a branch switch, pull, or rebase,
its blob OID is identical and its facts are a cache hit. No re-analysis is
performed for unchanged files, regardless of how many branches reference them.

**Layer 2 — Cross-file call graph (keyed by tree OID):**
After all per-file facts for a commit are available in Layer 1, the cross-file
call graph — edges connecting callers in one file to callees in another — is
computed and stored keyed by the git tree OID of the commit. This layer is
invalidated whenever any file in the tree changes (i.e., whenever the tree OID
differs from the stored one). It is recomputed lazily on the first query after
a tree OID change, from the Layer 1 fact union.

### Rationale

This two-layer structure is validated by industrial indexers operating at scale:

- GitHub's code search explicitly uses git blob SHA as the cache key, reducing
  indexing cost to O(changed files) and enabling cross-branch deduplication.
- Glean (Meta) uses an "immutable base layer + thin incremental layer per
  changeset" model (stacked databases), achieving O(change) cost on diffs.
- Stack graphs (prior art — archived September 2025) pioneered per-file-subgraph
  independence: each file's subgraph is constructed without cross-file knowledge;
  cross-file resolution is deferred to query time. `cgx` applies the same
  principle at Layer 1.

See docs/09-architecture.md (AR-4) for storage implementation details.

---

## IX-2: Per-Query Staleness Check and Synchronous Delta Re-Index

On every `cgx` invocation (query or explicit index command):

1. **Staleness check** (~10ms): read the stored indexed tree OID from the index
   header; compare to the output of `git rev-parse HEAD^{tree}`. If equal, the
   index is current — proceed directly to query execution.

2. **Delta computation** (if stale): run `git diff-tree --raw <stored_tree>
   <current_tree>` to obtain the list of blob OIDs that changed. This is
   O(changed files), not O(all files).

3. **Incremental re-index**: re-analyze only the changed blob OIDs in parallel
   (rayon thread pool — see AR-6). Each blob is independent; Layer 1 writes are
   batch-committed in a single storage transaction. Then recompute the Layer-2
   cross-file graph **by relinking cached Layer-1 facts** (no reparse of unchanged
   blobs); target includes this relink (<500ms for 1–20 changed files at 100k LOC).

4. **Answer query**: the index now reflects HEAD; execute the query.

**Target latency:** the incremental re-index for a typical change (1–20 files)
completes in under 500ms. First query after a large feature branch touching
500 files may take several seconds; subsequent queries on the same HEAD are
instant.

**Staleness detection is tree-OID-based only.** mtime-based detection (used by
ctags/gtags) is unreliable after `git checkout` because git updates mtime on
checkout even when file content is unchanged. `cgx` never uses mtime as a
cache validity signal.

---

## IX-3: Dirty Working Tree Handling

A "dirty" working tree contains uncommitted modifications: staged or unstaged
changes not yet reflected in any git commit.

### Default behavior — shipped, but not via a warning

By default the **CLI** indexes and queries the committed state of HEAD; dirty files are not
indexed. What ships in place of the stderr warning below is the freshness envelope (IX-0),
which carries the same information on every answer rather than once per invocation and
survives being piped:

```console
$ echo '// edit' >> src/main.rs
$ cgx callers helper --repo .
rust_sample::helper  src/main.rs:20
└─ rust_sample::middle  src/main.rs:15
   └─ rust_sample::main  src/main.rs:1
approximation: exact (within modeled graph)
freshness: stale | indexed tree 7380e24, 1 dirty file
```

Nothing is written to stderr. Queries still succeed and return results for the committed state;
the `stale` verdict and the dirty-file count are how the caller learns that.

*(The stderr warning described immediately below was not built. Its text is retained for the
record.)*

```
warning: 3 files have uncommitted modifications; results reflect committed state only
```

### `--include-dirty` — **CLI: not shipped. MCP: shipped and on by default**

There is **no `--include-dirty` flag on any CLI subcommand**; passing it is a clap parse error,
exit 2. The working-tree overlay this section describes exists, but only on the MCP surface,
where every graph-backed tool takes `include_dirty` with a schema default of `true`. The
asymmetry is deliberate — an agent editing a file wants to see its own uncommitted change; a
CI gate wants the committed state — but a doc that presents `--include-dirty` as a CLI flag
sends a reader to a parse error.

The mechanism below is accurate for the MCP path.

With `include_dirty`, `cgx` hashes each dirty file's current content (outside
git, using the same SHA computation as git would) and uses the resulting
synthetic blob OID as the cache key. Facts for dirty blobs are flagged
internally as `dirty=true` and are not written to the persistent index (they are
held in a transient in-memory overlay for the duration of that invocation).

**UX trade-off:** `include_dirty` gives current results for files being
actively edited but adds analysis overhead proportional to the number of dirty
files and does not persist across invocations. The default (committed-state-only)
is more predictable for CI use and avoids stale in-memory facts from prior
`include_dirty` runs polluting the persistent index.

There is no option to refuse queries when the working tree is dirty; that would
be the worst UX for interactive use.

### MCP overlay lifetime

When `cgx` runs as an MCP STDIO server (`cgx mcp`), the `include_dirty`
semantics are extended to a per-tool-call overlay lifetime:

- **On each tool call with `include_dirty: true`** (the MCP default — see
  docs/07-interfaces.md IF-9): enumerate dirty files via `gix status`, compute a
  synthetic blob OID for each using the same SHA computation as git, build or reuse
  Layer-1 facts for those OIDs, and compute a Layer-2 overlay delta for the query.
  Nothing is written to the persistent index (the IX-3 never-persisted invariant
  is preserved).
- **In-process memoization**: the MCP process may cache overlay Layer-1 facts keyed
  by synthetic blob OID. Because the key is content-addressed, reuse across tool
  calls is automatically correct — a further edit changes the OID and misses the
  cache. This makes call N+1 on an unchanged dirty tree cheap without any
  session/staleness protocol.
- **Result honesty**: `structuredContent` gains `"dirty": bool` and
  `"dirty_files_analyzed": int`; `graph_version` becomes
  `"<tree-oid>+dirty.<overlay-digest>"` when the overlay is active, where
  `overlay-digest` is a deterministic hash of the sorted synthetic OIDs — so agents
  (and caches keyed by `(query, graph_version)`) can distinguish bases.
- **MCP tools default to `include_dirty: true`; the CLI default remains
  committed-state-only.** This divergence is principled: agents editing and then
  immediately verifying via MCP tool calls receive post-edit truth by default; CI
  pipelines invoking the CLI get the predictable committed-state default.

---

## IX-4: Branch Awareness and Cross-Branch Queries

### Per-branch index entries

Each branch tip maps to a distinct tree OID. `cgx` stores a separate Layer 2
cross-file call graph for each (branch-name, tree OID) pair. Layer 1 blob-level
facts are shared across all branches; only the cross-file composition is
per-branch.

Switching branches (via `git checkout` or `git switch`) causes the next query
to detect a tree OID mismatch and trigger an incremental re-index covering only
the files that differ between the old and new branch.

### Cross-branch query: branch-diff model

`cgx` supports queries that compare call graph state across branches, using a
merge-base diff on edge sets.

**Canonical example:** "Which branches introduced calls to
`SecurityHandler.validateToken()` in the exception path?"

Algorithm:
1. Enumerate branch tips: `git for-each-ref --format='%(refname:short)
   %(objectname)' refs/heads/`.
2. For each candidate branch, compute the merge-base with `main`:
   `git merge-base <branch> main` → a common ancestor commit.
3. Obtain the call graph edge set at the branch tip (from the Layer 2 index,
   re-indexing if necessary).
4. Obtain the call graph edge set at the merge-base (indexing that commit if not
   cached).
5. Compute the edge-set difference: edges present at the branch tip but absent
   at the merge-base are edges "introduced by this branch."
6. Filter by edge condition `exception` (see docs/03-code-graph-model.md GM-3)
   and callee matching `SecurityHandler.validateToken()`.
7. Return matching (branch, caller, callee) triples with provenance.

Both the branch-tip index and the merge-base index are cached in Layer 1/2. If
previously indexed, steps 3–4 are instant lookups; only step 5–7 involve
computation.

### Eager branch pre-indexing

`cgx index --all-branches` iterates all local branch refs and indexes each in
turn. Useful in CI to warm the cache before cross-branch queries.

`cgx index --ref <ref>` indexes a specific ref (branch name, commit OID, or tag).

---

## IX-5: Branch Lifecycle and Garbage Collection

### Orphaned entries after branch deletion

When a branch is deleted (`git branch -d`), its tip commit OID is no longer
reachable from any live ref. The blob OIDs that were unique to that branch
(i.e., not referenced by any other live branch or tag) become orphaned entries
in the Layer 1 index. They waste space but do not produce incorrect results;
content-addressed keying means a stale entry for a blob OID can never corrupt
facts for a different blob.

Orphaned entries are **not** removed automatically. `cgx prune` (Planned — not yet shipped in v0.3.0) will perform the cleanup:

1. Run `git for-each-ref` to obtain the set of live tip commit OIDs.
2. Run `git rev-list --objects --all` to collect all blob OIDs reachable from
   live refs.
3. Cross-reference the index's blob OID key set against the reachable blob OID
   set.
4. Delete index entries whose blob OID appears in the index but not in the
   reachable set.

This is safe: a blob OID reachable from any live commit keeps its entry alive
regardless of which branch deleted it. A blob OID unique to a deleted branch
has no live referents and is safe to remove.

**`--aggressive` flag (Planned):** `cgx prune --aggressive` will also compact the storage
file (removes freed pages, reclaims disk). Without `--aggressive`, deleted
entries would be simply marked as free in the B-tree but the file would not shrink.

### Refcounting semantics

The prune algorithm implements implicit reference counting: the reference count
of a blob OID entry equals the number of live commits (across all refs) whose
trees include a file with that blob OID. An entry is collected when its count
reaches zero.

### Explicit management commands

| Command | Effect | v0.3.0 status |
|---------|--------|---------------|
| `cgx index` | Index HEAD of current branch (default: auto on any query) | Shipped |
| `cgx index --no-dataflow` | Build a CALLS-only index, skipping the DATA_FLOW layer | Shipped |
| `cgx index --scip <path>` | Apply SCIP semantic-precision re-label pass (upgrade-only; requires user-supplied SCIP index) | Shipped (`Since: v0.2`) |
| `cgx index --all-branches` | Eagerly index all local branch refs | **Planned** |
| `cgx index --ref <ref>` | Index a specific ref | **Planned** |
| `cgx prune` | Remove orphaned blob-OID entries from deleted branches | **Planned (not yet shipped in v0.3.0)** |
| `cgx prune --aggressive` | Prune + compact storage file | **Planned (not yet shipped in v0.3.0)** |
| `cgx status` | Show indexed tree OID, HEAD tree OID, cached blob count, index size | **Planned** |

Fully automatic management: under normal use, only `cgx index` is needed (it runs automatically on every query when stale). `cgx` tolerates orphaned entries silently; `cgx prune` for disk-space reclamation is planned for a future release.

---

## IX-6: Worktree Model

### Headline requirement

`cgx` answers queries from the context of the main checkout (the repository
where `.git/` lives). Git worktrees are treated as branches on a common base,
not as separate repositories.

This is a first-class requirement: a user working in a git worktree at
`/path/to/repo/feature-branch/` expects `cgx` queries run from the main
checkout at `/path/to/repo/main/` to include or compare worktree state, and
vice versa.

### Git worktree internals

`git worktree add <path> <branch>` creates a linked worktree. All linked
worktrees of a repository share:

- `.git/objects/` (all git objects, including blobs)
- `.git/refs/` and `.git/packed-refs` (all branch and tag references)

Each linked worktree has its own `HEAD`, staging index, and merge state stored
in `.git/worktrees/<name>/`. Because the object store is shared, a blob OID
computed in any worktree is identical to the same blob OID in the main checkout.

### Shared blob-level index

The Layer 1 blob-OID index is shared by all worktrees of the same repository.
A file analyzed while working in `feature-branch/` is a cache hit when `main/`
later indexes the same blob OID — no duplicate work.

### Worktree enumeration

`cgx` enumerates all worktrees via `git worktree list --porcelain` and records
each worktree's current HEAD commit OID. Each worktree HEAD is treated as a
branch tip for the purposes of IX-4 (branch awareness): it gets its own Layer 2
cross-file call graph entry, and it participates in branch-diff queries.

A query run from `main/` can ask: "compared to main, what new calls appear in
the worktree at `feature-branch/`?" using the same edge-set diff algorithm as
IX-4. The worktree's branch tip is the branch; the merge-base with `main` is
the common ancestor.

### Per-worktree staleness

Each worktree's HEAD is tracked independently. A stale check for worktree W
compares W's HEAD tree OID to the Layer 2 entry for W's branch. Only blobs
that changed relative to the shared base are re-indexed for W.

---

## IX-7: Concurrent CLI Access

Multiple `cgx` processes may run simultaneously (e.g., two terminal windows, a
CI pipeline and a local query). The index must remain consistent under
concurrent access.

### MVCC shared reads

The storage engine provides MVCC (multi-version concurrency control) for reads.
Multiple concurrent read-only queries hold shared read transactions
simultaneously without blocking one another and without blocking a write
transaction in progress. Readers see a consistent snapshot of the index as of
their transaction start. See docs/09-architecture.md (AR-5) for the storage
choice that provides these semantics.

### Advisory locking for writes

Index write operations (re-indexing; `prune` when it ships) acquire an exclusive advisory file
lock on a lockfile in the index directory before modifying index state. Lock
acquisition has a 1–2 second timeout; if the lock cannot be acquired, `cgx`
exits with an error rather than blocking indefinitely.

If two invocations concurrently detect a stale index and both attempt to
re-index:

1. The first acquires the exclusive lock and re-indexes.
2. The second waits for the lock (up to timeout).
3. After the first releases the lock, the second acquires it, re-checks the
   tree OID (now current), finds no staleness, and skips re-indexing.

This double-check pattern (check → lock → re-check) prevents redundant work
without requiring a long-held lock.

### Worktree concurrency

Two worktrees querying simultaneously can both hold shared read locks on the
blob-level Layer 1 index without conflict. Contention only arises if two
worktrees simultaneously trigger a write for the same blob OID — an unlikely
event that is handled correctly by the locking protocol above.

---

## IX-8: Index Location

### Shipped: `.cgx/` in the repository root

**The implementation does not follow the XDG decision recorded below.** `cgx index` writes
into the repository root, and `~/.cache/cgx/` is never created:

```console
$ cgx index .
$ ls -a .cgx/
.gitignore  HEAD.json  index.db
$ cat .cgx/HEAD.json
{
  "graph_key": "7380e245ab98db2dfed12ed3ee3b005f0fbf7036",
  "graph_id": 1,
  "total_files": 1,
  "unsupported_files": 0
}
$ cat .cgx/.gitignore
*
$ git status --porcelain      # empty — the self-ignoring .gitignore keeps the tree clean
```

Three files: `index.db` (the SQLite store — `rusqlite`, bundled, WAL), `HEAD.json` (the
pointer naming the indexed tree OID and graph id), and a `.gitignore` containing `*`, which
is how `.cgx/` avoids the "polluted by `git status`" objection the table below raises against
this very location. `cgx index` also garbage-collects superseded graphs, keeping exactly one.

The rest of this section is the **original design decision**, retained because the trade-offs
it records are still the ones that matter if the location is ever revisited. It is not a
description of shipped behaviour. In particular there is no `<repo-id>` hash, no
`.git/cgx/config` pointer file, and worktrees of the same repository each get their own
`.cgx/` rather than sharing one cache.

### Shipped: sharing the index — the `refs/cgx/index` overlay (`cgx push`/`cgx pull`)

The location above (`.cgx/` in the repo root, self-ignored) is local-only: nothing under
`.cgx/` is committed, so a fresh clone or a CI runner starts with no index. `cgx push` and
`cgx pull` share a locally-built index over git's own transport, without writing anything to
a branch.

`cgx push [remote] [--repo <path>]` promotes the local `.cgx` committable set —
`objects/` + `refs/` + `HEAD.json`, the same content-addressed object model described in IX-1,
unchanged — into the repository's git object database using `git` plumbing (`hash-object`,
`mktree`, `commit-tree`, `update-ref`; no `libgit2` dependency). The objects are wrapped in a
**deterministic, parentless commit** (fixed author/committer identity and timestamp, fixed
message, no parent — two pushes of an unchanged tree produce a byte-identical commit OID) and
that commit is advanced, via compare-and-swap, at a dedicated non-branch ref:
**`refs/cgx/index`**. `cgx push` never writes `refs/heads/*` and creates no commit on any
branch; the objects it writes do not appear in `git log`, `git blame`, or a `refs/heads/*`
diff. The one honest caveat: a ref *is* a ref, and `git log --all` (which walks every ref,
not just branch history) does list the commit — invisibility holds for the ordinary
branch-history walk, not for a full-ref enumeration.

`cgx pull [remote] [--repo <path>]` fetches `refs/cgx/index` and materializes `.cgx/objects`,
`.cgx/refs`, and `.cgx/HEAD.json` locally, re-hashing each object's bytes against its OID as
it writes them (a mismatch is a hard integrity error, never served). `cgx push`/`cgx pull` are
transport only — they do not change the object model, the OID scheme, or the manifest format
described elsewhere in this document.

**Refspec ergonomics.** A plain `git clone` or a default `git fetch` does not bring
`refs/cgx/*` down — git only fetches refs a refspec names. One step configures it for a given
clone:

```
cgx configure-remote [remote]
```

which writes `remote.<remote>.fetch`/`remote.<remote>.push = +refs/cgx/*:refs/cgx/*` into
that repository's git config (idempotent; defaults `remote` to `origin`). The same effect is
had by hand-editing `.git/config`:

```
[remote "origin"]
    fetch = +refs/cgx/*:refs/cgx/*
```

`cgx pull` itself always passes an explicit `+refs/cgx/index:refs/cgx/index` refspec on the
`git fetch` it runs, so **pulling works without `configure-remote` having been run** — the
refspec matters for anything that does a *bare* `git fetch`/`git pull` outside of `cgx pull`
(a CI job's checkout step, an editor's background fetch, a teammate's manual `git fetch`).
Every clone or CI runner that wants `refs/cgx/*` visible to plain git needs this configured
independently — `cgx configure-remote` edits only the repository it is run in and **cannot**
reach into a CI runner's own git config on your behalf. If the ref is genuinely absent from
the remote (no one has pushed yet, or the wrong remote was named), `cgx pull` fails **loudly**
with an error naming the `+refs/cgx/*:refs/cgx/*` refspec as the fix — never a silent,
empty-but-successful pull.

**Portability posture.** `refs/cgx/index` is the primary, shipped rung: it works on
GitHub-class hosts that accept pushes to arbitrary custom refs. Custom refs are not accepted
everywhere — Azure DevOps rejects pushes outside `refs/heads/*` and `refs/tags/*` (verified
2026-06-29; see the host-compatibility table in the sparse-storage RFC, §4.7). For hosts in
that class, the documented next rung is an **orphan branch, `refs/heads/cgx-index`**: the same
object model and the same deterministic commit, retargeted to a normal branch ref instead of
`refs/cgx/index` — a one-line ref-name change, since the commit-wrapping the objects already
require (above) is exactly what an orphan branch under `refs/heads/*` needs. This rung is
**documented, not implemented, in this cycle.** Its tradeoff: a branch under `refs/heads/*` is
visible in ordinary branch listings and PR UIs (unlike `refs/cgx/index`), and an
organization's branch-protection or required-review policy applied to all branches may block
cgx's automated updates to it. A third, in-tree rung (committing the index directly into a
source branch) was considered and **dropped**; it is not offered as an option here or
elsewhere in `cgx`.

### Decision (design, not shipped): XDG cache directory

The index would live at:

```
~/.cache/cgx/<repo-id>/
```

where `<repo-id>` is a stable identifier derived from the absolute path of the
repository's git directory (`git rev-parse --absolute-git-dir`), hashed to a
fixed-length hex string.

**Rationale (trade-off assessment):**

| Location | Pros | Cons | Verdict |
|----------|------|------|---------|
| `~/.cache/cgx/<repo-id>/` (XDG) | Does not pollute repo; survives `git clean -fdx`; shared across all worktrees of the same repo automatically | Does not follow a clone to a new machine; requires stable repo ID | **Recommended** |
| `.git/cgx/` (in-repo git subdir) | Follows repo; auto-discovered; standard location for git tooling state | May be cleared by aggressive git maintenance commands; non-standard for user caches | Viable fallback |
| `.cgx/` (project root) | Simple discovery | User-visible; polluted by `git status`; not suitable | Avoid |

The XDG location is chosen because:

1. All worktrees of the same repository share a single `.git/objects/` store
   and therefore share the same blob OID space. An XDG cache keyed by
   `<repo-id>` is naturally shared by all worktrees without additional
   coordination.
2. The index survives `git clean`, branch deletes, and rebases.
3. Build tool precedent (ccache, cargo, bazel remote cache) favors out-of-tree
   caches for exactly this reason.

A pointer file at `.git/cgx/config` (created on first index, containing the
XDG path) allows future tooling to discover the index location without
hard-coding the XDG convention.

### Repo-ID stability

The repo ID is derived from `git rev-parse --absolute-git-dir` (the canonical
absolute path to the `.git` directory or the common `.git` dir for linked
worktrees). This is stable for the lifetime of the clone and identical across
all linked worktrees of the same repository.

If the repository is cloned to a new path, a new repo-ID is computed and
`cgx` re-indexes from scratch on first use. Transferring the cache to a new
machine requires relocating the `~/.cache/cgx/<old-repo-id>/` directory to
`~/.cache/cgx/<new-repo-id>/` on the destination (not automated in v1).

---

## IX-9: Edge Age and Author Attribution

**Status: core-extension** — the blob-OID and merge-base machinery already in place (IX-1, IX-4) provides the inputs needed to attribute each call edge to the commit that introduced it. This is derivable at index time without additional git object reads on the query path.

### Motivation

Two motivating queries make this the single highest-value security feature in the VCS integration layer:

1. **Newest edges into sensitive sinks.** Security review prioritization: "which call edges pointing to `sql`-, `shell`-, or `eval`-class sinks were introduced most recently, and by whom?" Reviewers can focus on the newest-introduced edges rather than reviewing the full reachable graph.

2. **Branch-diff security gate (CI).** "Did this pull request introduce any new edge from a network-tainted source to a sensitive sink?" If `introducing_commit` on a new call edge is one of the commits in this PR, the gate fires. This is the "diff gate" question in `docs/05-queries.md` Q-25 Worked Example 6.

### Derivation

Edge age is derived from the content-addressed index structure (IX-1) via the per-branch merge-base machinery (IX-4):

1. **Blob OID → commit OID.** Each edge in Layer 1 is keyed by the blob OID of the file that introduced it (IX-1). The git object store allows reverse-lookup of the set of commits whose trees include a given blob OID: `git log --all --find-object=<blob-oid>` returns the relevant commits. The earliest such commit (by topological order, not timestamp) is the **introducing commit** for that blob.

2. **Edge-level attribution.** Within a file, the line range of the call site is known from the edge's provenance record (GM-6.1). `git blame --porcelain -L <line>,<line> <file>` returns the commit and author that last changed that line. At index time, `cgx` runs blame on the call-site lines for new blobs and stores the result in the Layer 1 entry.

3. **Incremental cost.** Blame is run only for blobs that are new to the index — i.e., blobs whose OID does not appear in any prior Layer 1 entry. For unchanged blobs (cache hits), the stored attribution is reused. This keeps incremental indexing cost proportional to the number of changed files, not the total index size.

4. **Merge-base scoping.** For branch-diff queries, "introduced by this branch" means: the introducing commit is reachable from the branch tip but not from the merge-base with `main` (IX-4 step 2). This is computed via `git log <merge-base>..<branch-tip> --format='%H'` and stored as the branch-introduced commit set.

### Edge attributes

Each call edge in the Layer 2 index gains two additional attributes when IX-9 is active:

```
introducing_commit:  string   # commit SHA where this call edge first appeared
introducing_author:  string   # git author identity for that commit (name + email)
```

These attributes are optional: edges in blobs that were present before IX-9 was first run carry no attribution until the blob is re-indexed (i.e., until the file changes and the new blob is indexed with blame).

### Query surface

Both attributes are available in the full query language and as diff-subcommand filters.

**Layer 1 — diff subcommand:**

**Shipped (v0.3.0):** `cgx diff <BASE> <HEAD>` with `--newer-than` only.

```bash
# New edges added at HEAD that were absent at main (shipped)
cgx diff main HEAD --newer-than
```

**Planned (v0.4) — not yet runnable:**

```bash
# New edges into sensitive sinks introduced by this branch
# cgx diff main HEAD --repo . \
#     --calls-to-sink-class sql \
#     --calls-to-sink-class shell \
#     --calls-to-sink-class eval \
#     --format sarif > new-sink-edges.sarif

# Newest edges into sql sinks across all branches (review prioritization)
# cgx diff HEAD~30 HEAD --repo . \
#     --calls-to-sink-class sql \
#     --order introducing_commit \
#     --format json | jq '.[] | {edge, introducing_author, introducing_commit}'
```

**Layer 2 — query language:**

```cypher
-- Newest edges into sensitive sinks, with author attribution
MATCH (caller)-[r:CALLS]->(sink)
WHERE sink.sink_class IN ["sql","shell","eval","log","net-request"]
  AND r.introducing_commit IS NOT NULL
RETURN caller.name, caller.file, caller.line,
       sink.name, sink.sink_class,
       r.introducing_commit, r.introducing_author
ORDER BY r.introducing_commit DESC
LIMIT 50

-- Branch-diff gate: edges introduced by this PR's commits
MATCH (caller)-[r:CALLS]->(sink {sink_class:"sql"})
WHERE r.introducing_commit IN $pr_commits
RETURN caller.name, caller.file, caller.line,
       sink.name,
       r.introducing_commit, r.introducing_author
```

### Interaction with IX-4 (branch-diff)

The edge-age attribute and the branch edge-set diff (IX-4) answer related but distinct questions:

| Question | Mechanism |
|----------|-----------|
| Which edges are structurally new on this branch (not present at merge-base)? | IX-4 edge-set difference: present at branch tip, absent at merge-base |
| Which edge was introduced by a specific commit? | IX-9 `introducing_commit` attribute on the edge |
| Which author introduced a specific edge? | IX-9 `introducing_author` attribute |

An edge that is structurally new (IX-4) typically also has `introducing_commit` pointing to a commit on this branch. However, an edge may be new on this branch but have an `introducing_author` from a prior branch (e.g. a cherry-pick). Both attributes are stored and queryable independently.

### CI integration

The branch-diff security gate (docs/05-queries.md, Worked Example 6) combines IX-4 (which edges are new) with IX-9 (which commit introduced each new edge) to produce annotated SARIF findings:

The CI integration below uses `--calls-to-sink-class` and `--assert-empty` with `diff` — these are **Planned (not yet shipped in v0.3.0)**. At v0.3.0, use `cgx diff origin/main HEAD --newer-than` and inspect output manually.

```yaml
# PLANNED (v0.4) — not yet runnable
- name: Security diff gate
  run: |
    PR_COMMITS=$(git log origin/main..HEAD --format='%H')
    cgx diff origin/main HEAD --repo . \
        --calls-to-sink-class sql \
        --calls-to-sink-class shell \
        --calls-to-sink-class eval \
        --assert-empty \
        --format sarif > security-diff.sarif
  continue-on-error: false

- name: Upload SARIF
  uses: github/codeql-action/upload-sarif@v3
  with:
    sarif_file: security-diff.sarif
```

Each SARIF finding carries `introducing_author` in the `properties` bag, enabling GitHub's code-scanning UI to assign the finding to the commit author for review.

---

## IX-10: Index-Free History Queries (`cgx coupling`)

Not every VCS question needs a call graph. **`cgx coupling` reads committed git history and
nothing else** — no extraction, no linking, no `.cgx/`, no auto-index. It reports, for every
pair of files touched by the same commit in an explicit rev range, the co-change count and each
file's own change count.

```console
$ cgx coupling HEAD~4 HEAD --repo .
HEAD~4 (7ef5392)..HEAD (d3429b0)
4 commits considered · 0 merge excluded · 0 root excluded · 0 oversized excluded
cochanges  a-changes  b-changes  files
        4          4          4  src/main.rs  tests/t.rs
approximation: over- and under-approximate — co-change is attributed at file granularity; …
```

`<BASE>` is exclusive, `<HEAD>` inclusive. `--min-cochanges` (default 2), `--limit` (default 50)
and `--max-files-per-commit` (default 50) bound the report. `--format` accepts `human` and
`json`; the other four are exit 2.

### Why it carries an approximation contract but no freshness envelope

This is the clearest illustration of the two envelopes being independent (IX-0). `coupling`
**is** approximate — in several separate, *named* ways — so it carries the contract. It **never
opens the index**, so there is no index currency to report and the freshness envelope is
absent rather than empty. The MCP dispatch test encodes exactly this: `coupling` is the sole
member of its `INDEX_FREE_TOOLS` list, the closed set of tools exempt from the
freshness-on-every-answer invariant.

**The four reasons the worked example above emits**, and what each does to a row:

| Reason code | Direction | Consequence |
|---|---|---|
| `file-level-granularity` | over | A pair may co-change because *unrelated* symbols in those files were edited together. File-level signal, not symbol-level. |
| `bounded-rev-range` | under | Only single-parent commits inside `BASE..HEAD` were walked. Merges and root commits are excluded, and coupling outside the range is invisible. |
| `renames-not-tracked` | over | Rename detection is deliberately off so the walk cannot depend on ambient `diff.*` config — a determinism requirement. A rename reads as an unrelated delete plus add. |
| `cochange-threshold` | under | Pairs below `--min-cochanges` are not reported at all. |

**The full set is larger and conditional.** Seven further codes appear only when their condition
holds: `merge-commits-excluded`, `root-commits-excluded`, `large-commit-excluded`,
`result-limit`, `shallow-repository`, `history-boundary` and `empty-rev-range`. Consume the
`reasons` array keyed on `code` rather than assuming a fixed set — a new code can land without
any existing one changing.

### The shallow-clone trap, and how to detect it

The census line (`N commits considered · …`) is not decoration; it is how you confirm the range
you asked for is the range that was walked. **On a shallow clone it will not be.** The walk
paints the base commit's full ancestry eagerly and stops at the graft, so a range that appears
to sit well inside `--depth N` can return an **empty answer at exit 0** — a clean-looking
result that means "no history to walk," not "no coupling here."

`fetch-depth: 1` is the default for `actions/checkout`, which is precisely the environment
where this fires, and a CI job gating on exit code alone reads a false negative.

**The answer says so — in both formats.** Human output carries a dedicated `degraded:` line,
emitted whenever the walk truncated or the repository is shallow. Reproduced on a
`git clone --depth 2`:

```console
$ cgx coupling HEAD~1 HEAD --repo .
HEAD~1 (ae36d86)..HEAD (d3429b0)
0 commits considered · 0 merge excluded · 0 root excluded · 0 oversized excluded
degraded: walk truncated at a history boundary (missing or unreadable object) · shallow clone — deepen the clone to see more history
(no co-changed pairs)
approximation: over- and under-approximate — …
$ echo $?
0
```

That is the failure in full: **zero commits considered, zero pairs, exit 0** — and the
`degraded:` line is the only thing distinguishing it from a genuine "these files never
co-change." Note the range itself was valid; the graft, not the range, is why nothing was
walked.

For scripts, the same condition is a named under-approximation reason:

```json
{ "direction": "under",
  "code": "shallow-repository",
  "detail": "the repository is a shallow clone; history before the graft boundary is absent and its co-changes are not counted" }
```

Gate on the reason code rather than the exit status:

```bash
cgx coupling "$BASE" HEAD --repo . --format json > coupling.json
jq -e '.approximation.reasons | any(.code == "shallow-repository")' coupling.json \
  && { echo "refusing to trust coupling on a shallow clone"; exit 1; }
```

`history-boundary` (the walk stopped at a missing or unreadable object) and `empty-rev-range`
warrant the same guard. `git fetch --unshallow` before running is the other fix. The report's
top-level `shallow_repository` and `truncated_at_history_boundary` booleans carry the same
signal if you prefer a field lookup to a reason scan — and `commits_considered` remains a useful
sanity check, but it is the weaker one, because it cannot distinguish "no history" from
"no co-changes".
