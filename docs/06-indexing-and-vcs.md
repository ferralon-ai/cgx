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
- IX-9: Edge age and author attribution

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

### MCP overlay lifetime

When `cgx` runs as an MCP STDIO server (`cgx mcp`), the `--include-dirty`
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

```bash
# New edges into sensitive sinks introduced by this branch
cgx diff --base main --head HEAD ./ \
    --calls-to-sink-class sql \
    --calls-to-sink-class shell \
    --calls-to-sink-class eval \
    --format sarif > new-sink-edges.sarif

# Newest edges into sql sinks across all branches (review prioritization)
cgx diff --base HEAD~30 --head HEAD ./ \
    --calls-to-sink-class sql \
    --order introducing_commit \
    --format json | jq '.[] | {edge, introducing_author, introducing_commit}'
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

```yaml
- name: Security diff gate
  run: |
    PR_COMMITS=$(git log origin/main..HEAD --format='%H')
    cgx diff --base origin/main --head HEAD ./ \
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
