# 06 — Indexing and VCS Integration

**Status:** Feature specification (pre-implementation)
**Audience:** Engineers building or operating `cgx`; contributors; CI/CD integrators
**Working name:** `cgx` (placeholder — see docs/README.md)
**Cross-references:** docs/03-code-graph-model.md (GM-) · docs/09-architecture.md (AR-) · docs/05-queries.md

---

## Overview

`cgx` maintains a persistent, content-addressed index that stays current
automatically. No daemon runs in the background. On every query invocation,
`cgx` checks whether the index reflects the current HEAD; if not, it re-indexes
only the files that changed, then answers the query. Explicit commands exist for
eager indexing and for pruning stale entries, but normal use requires neither.

This document specifies:

- IX-1: Content-addressed index structure (blob OID and tree OID layers)
- IX-2: Per-query staleness check and synchronous delta re-index
- IX-3: Dirty-working-tree handling
- IX-4: Branch awareness and cross-branch queries
- IX-5: Branch lifecycle and garbage collection
- IX-6: Worktree model
- IX-7: Concurrent CLI access
- IX-8: Index location

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
   batch-committed in a single storage transaction. Then recompute the Layer 2
   cross-file graph.

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

### Default behavior

By default, `cgx` indexes and queries the committed state of HEAD. Dirty files
are not indexed. If dirty files exist, `cgx` emits a warning on stderr:

```
warning: 3 files have uncommitted modifications; results reflect committed state only
```

Queries still succeed and return results for the committed state.

### `--include-dirty` opt-in

With `--include-dirty`, `cgx` hashes each dirty file's current content (outside
git, using the same SHA computation as git would) and uses the resulting
synthetic blob OID as the cache key. Facts for dirty blobs are flagged
internally as `dirty=true` and are not written to the persistent index (they are
held in a transient in-memory overlay for the duration of that invocation).

**UX trade-off:** `--include-dirty` gives current results for files being
actively edited but adds analysis overhead proportional to the number of dirty
files and does not persist across invocations. The default (committed-state-only)
is more predictable for CI use and avoids stale in-memory facts from prior
`--include-dirty` runs polluting the persistent index.

There is no option to refuse queries when the working tree is dirty; that would
be the worst UX for interactive use.

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

Orphaned entries are **not** removed automatically. `cgx prune` performs the
cleanup:

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

**`--aggressive` flag:** `cgx prune --aggressive` also compacts the storage
file (removes freed pages, reclaims disk). Without `--aggressive`, deleted
entries are simply marked as free in the B-tree but the file does not shrink.

### Refcounting semantics

The prune algorithm implements implicit reference counting: the reference count
of a blob OID entry equals the number of live commits (across all refs) whose
trees include a file with that blob OID. An entry is collected when its count
reaches zero.

### Explicit management commands

| Command | Effect |
|---------|--------|
| `cgx index` | Index HEAD of current branch (default: auto on any query) |
| `cgx index --all-branches` | Eagerly index all local branch refs |
| `cgx index --ref <ref>` | Index a specific ref |
| `cgx prune` | Remove orphaned blob-OID entries from deleted branches |
| `cgx prune --aggressive` | Prune + compact storage file |
| `cgx status` | Show indexed tree OID, HEAD tree OID, cached blob count, index size |

Fully automatic management: under normal use, none of these commands are
required. `cgx` re-indexes on every query when stale and silently tolerates
orphaned entries. `cgx prune` is for disk-space reclamation only.

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

Index write operations (re-indexing, prune) acquire an exclusive advisory file
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

### Decision: XDG cache directory

The index lives at:

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
