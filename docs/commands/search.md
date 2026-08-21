# cgx search

Search the symbol table for definitions whose FQN matches `pattern`.

## Synopsis

```
cgx search [OPTIONS] [PATTERN]
```

## Description

`cgx search` answers the question: *what is the exact FQN for this symbol?* It performs a pure node-table scan — no graph walk — and returns every definition whose fully-qualified name matches the pattern. The result is a list of FQN, source location, and symbol kind.

The primary use case is resolving a partial or half-remembered name to exact FQNs that can then be passed to `callers`, `callees`, `reaches`, `flows-to`, or `flows-from`. This is especially useful for discovering dataflow value-node FQNs (e.g., `rust_sample::dataflow::flow_example::b#1`), which are SSA-derived names that do not appear in source code directly.

**Match modes:**

- Default: case-insensitive substring match against the whole FQN. Matches anywhere in the name.
- `--regex`: matches the whole FQN as a regular expression. The pattern is matched as an unanchored regex (use `^` and `$` to anchor explicitly).
- Selector grammar: `PATTERN` written with `**`, `(`, or `!` routes to the node-selector engine instead — see [Selector patterns](#selector-patterns) below.

**No-match behavior:** Finding nothing prints `(no results)` — followed, as always, by the freshness line — and exits 0. `search` is not the exact-symbol resolver; it is the discovery surface. An empty result is not an error.

**Freshness, no approximation.** Every answer ends with the index-freshness envelope (`--format json`: a top-level `freshness` key), including the empty one. `search` carries no approximation contract, and that is a property of the operation rather than a gap: a node-table scan performs no traversal, so there is no frontier to over- or under-approximate. See [Reading an answer](README.md#reading-an-answer).

**Format support:** Only `human` and `json` output formats are supported. Passing `--format sarif` (or `dot`, `mermaid`, `d2`) exits 2 with an error message.

Since: v0.2.

## Arguments

| Name | Required | Meaning |
|------|----------|---------|
| `PATTERN` | conditionally | The string, regex, or selector to match against each symbol's fully-qualified name. Substring match by default; regex when `--regex` is given; selector-engine match when the string carries `**`, `(`, or `!` grammar (`--regex` opts out of selector routing even then). Exactly one of `PATTERN` or `--all` must be given: neither exits 2 with `a search pattern is required (or pass --all to list every symbol)`, and both exits 2 as well. |

## Options

| Flag | Value | Default | Meaning |
|------|-------|---------|---------|
| `--all` | boolean flag | — | List every symbol instead of matching a pattern. Mutually exclusive with `PATTERN`; pair with `--kind` and `--limit` to browse a kind. |
| `--regex` | boolean flag | — | Treat `PATTERN` as a regular expression over the whole FQN. Replaces the default case-insensitive substring match. Invalid regex → exit 2. Also forces the legacy path for a pattern that would otherwise look like a selector. |
| `--agnostic` | boolean flag | — | Selector matches only (no effect on substring/regex `PATTERN`). Drops the node-selector engine's family gate: a selector normally matches only symbols in a language family whose tokenizer accepted the string; `--agnostic` matches the normalized pattern against every symbol regardless of source language. See [Native vs. agnostic matching](#native-vs-agnostic-matching-and-provenance). |
| `--kind` | `function\|method\|type\|field\|variable\|module\|constant\|macro\|lambda\|entrypoint` | — | Restrict results to a single symbol kind. No flag = all kinds returned. |
| `--limit` | integer | `50` | Maximum number of results to print. `0` = unlimited. When results exceed the limit, the sorted top-N print with a footer indicating how many were omitted. |
| `--repo` | path | CWD | Path to the indexed repository containing a `.cgx/` store. |
| `--format` | `human\|json` | `human` | Output format. `sarif`, `dot`, `mermaid`, and `d2` are not supported and exit 2. |
| `--no-auto-index` | boolean flag | — | Require an explicit prior `cgx index`; if no `.cgx/` store is present, exit 3 instead of auto-indexing. |

## Selector patterns

A `PATTERN` containing `**`, `(`, or `!` routes to the **node-selector engine**
(`cgx-select`) instead of the substring/regex scan above. This is **package pattern
matching** — not glob, not regex. It borrows some notation from both but is neither:
closer to shell glob than to `grep`-style regex, but its own grammar. Bring the wrong
mental model and one construct will surprise you (below); the value of naming the
category is avoiding that surprise.

**The regex trap:** `(a|b)*` reads, to a regex-trained eye, as "zero-or-more of the
group `(a|b)`" — i.e. it could match the empty string. In selector grammar it means
**"starts with `a` or `b`"**: `(a|b)` is an alternation over one segment's content, and
`*` after it extends that segment, it does not repeat the group. `(a|b)*` matches
`apple` and `banana`, not `""`, `"ab"`, or `"baba"`.

### Anchored by default

A bare selector segment is an **equals** match, not a substring match. `Service`
matches a symbol whose FQN *is* `Service`; it does not match `MyService` or
`ServiceImpl`. The whole FQN must be consumed end to end — `foo` does not match
`foo::bar`. This mirrors package-identity intuition most languages already have:
`com.foo` is not `com.foobar`. `*` and `**` are the explicit opt-in to looser
matching; nothing is loose by accident.

### Grammar

The uniform internal path is `::`-segmented; each registered language parses its own
surface syntax into that form (see [Per-language surface syntax](#per-language-surface-syntax)).

| form | meaning |
|------|---------|
| `Service` | segment equals `Service` |
| `*Service` | segment ends with `Service` (matches bare `Service` too) |
| `Svc*` | segment starts with `Svc` |
| `*Svc*` | segment contains `Svc` |
| `*` (a whole segment) | exactly one segment, any name — not zero, not two |
| `**` (a whole segment) | zero-or-more whole segments (globstar over depth) |
| `(foo\|bar)::Svc` | **two** segments: `foo` or `bar`, then `Svc` |
| `(foo\|bar)Svc` | **one** segment: `fooSvc` or `barSvc` |
| `!seg` | segment not-equals `seg` |
| `!seg*` | segment not-starts-with `seg` |
| `*!Test` | segment not-ends-with `Test` |
| `!seg*test` | not-starts-with `seg` AND ends-with `test` (`foobartest` matches) |
| `!(Mock\|Test)*Service` | ends-with `Service` AND starts with neither `Mock` nor `Test` |

**The separator is load-bearing in alternation:** `(foo|bar)::Svc` is two segments (a
selector for `foo::Svc` or `bar::Svc`); `(foo|bar)Svc` is one segment (`fooSvc` or
`barSvc`). Moving the separator inside or outside the parens changes what the selector
means, silently — there is no error to warn you.

**Negation (`!`) is single-segment only** — `!(a|b)::c` negates within one segment's
constraint set, never a whole subtree (`!**` does not exist). `!` immediately before a
wildcard is degenerate (`!*` would mean "not-any-string" = matches nothing), so the
engine rewrites it past the wildcard to the next atom and prints a warning:
`!*Test` becomes `*!Test` (not-ends-with-`Test`). The rewritten form is what actually
matches; the warning goes to stderr so stdout stays parseable.

**No `?`.** There is no single-character wildcard — `com.?.Service` is rejected with a
labeled error rather than silently doing something unexpected; use `*` (it already
covers the single-char case, since `com.?.Service`'s value over `com.*.Service` is nil).

### Quoting your selector (shell, not cgx)

`!`, `(`, `)`, and `*` all have meaning to bash/zsh before `cgx` ever sees the string.
**Single-quote the whole selector:**

```
cgx search --repo /path/to/repo 'com.service.!Test'
```

Single quotes make every one of those characters inert to the shell, including `!`
(interactive bash/zsh history expansion) and `()`/`*` (globbing) — nothing to escape,
nothing to get wrong. The traps if you don't:

- Unquoted `\!` yields a literal `!` to cgx, but `()`/`*` are back under the shell's
  glob expansion — you've fixed one problem and reopened another.
- Double-quoted `"\!"` is not a fix — double quotes don't treat `!` as an escapable
  character, so the backslash survives into the string cgx receives (`\!`, not `!`),
  corrupting the pattern. (The same backslash is PHP's path separator in a future
  PHP tokenizer, which is exactly the kind of mistake this trap invites.)

**This is a shell property, not a `cgx` one — and it's interactive-only.** History
expansion (`!`) fires in an interactive bash/zsh session reading from a terminal. A
script, a CI job, or an agent invoking `cgx` non-interactively never triggers it, so
the primary programmatic consumers of `cgx search` are unaffected; single-quoting is
a habit for people typing at a prompt, not a requirement for automation.

### Per-language surface syntax

Each supported language has its own tokenizer; a selector string self-selects the
families whose separator and identifier grammar it lexes cleanly under. There is no
central separator-to-language table — each tokenizer decides for itself.

| language | separator | example |
|----------|-----------|---------|
| Rust | `::` | `rust_sample::conditions::*_check` |
| Go | `/` (see below) | `github.com/org/repo/**/Handler` |
| TypeScript | `.` | `some.path.**` |
| Java | `.` | `com.foo.(Mock\|Test)*Service` |
| Python | `.` | `some.path.**` |

**Go is the one structural specialization:** the separator is `/`, but a host segment
like `github.com` carries a **literal `.` inside one segment**, not a separator. A Go
selector never uses `.` to divide segments.

A leading `::` in Rust is a root/global anchor (`::foo::Bar`), not an illegal empty
segment — `crate::`/`self::`/`super::` prefixes are unaffected.

### Native vs. agnostic matching, and provenance

By default, a selector matches only nodes whose source language belongs to a family
whose tokenizer accepted the string (**native** matching) — a selector written in Java
syntax (`com.foo.Bar`) does not, by default, match a Python node with the equivalent
FQN even though Python's tokenizer also accepts `com.foo.Bar`'s surface form. `--agnostic`
drops that restriction: the selector matches the normalized pattern against every
symbol regardless of language.

Multiple tokenizers can accept the same string (a bare `Foo`, or `a.b.c` under
Java/Python/TypeScript). When they do, the engine carries the **union** of clean
parses, and every match's provenance (which family produced it) travels with the
result. Running with `--agnostic` and getting a hit from an unexpected language is
disclosed, not hidden: `cgx` prints a stderr note —
`cgx: selector note: N match(es) came from a different source language than the
selector's surface syntax (agnostic mode dropped the family gate)`.

### Honest failure

**Zero valid parses is a labeled error, never a silent empty match.** If no registered
tokenizer can lex the string, `cgx search` exits 2 with the reason from every tokenizer
that rejected it, e.g.:

```
cgx: selector "rust_sample::(foo|bar)::@x": no valid parse (illegal character ':' at position 11; illegal character '@' at position 0)
```

This is deliberate: an unparseable selector must never be quietly interpreted as
"nothing matches" — that would be indistinguishable from a correct, empty result.

A pathological pattern (deeply nested `**` and alternation) can grow the matcher's
active-state set past its cap. When that happens the match is disclosed as
**truncated** (a stderr warning: `the active-state cap was reached while matching —
results may be incomplete (empty ≠ absent)`) rather than silently returning a partial
or empty result as if it were complete.

## Examples

### Common case: discover functions in a module

```
cgx search "rust_sample::conditions" \
  --kind function \
  --repo /path/to/worktree
```

```
rust_sample::conditions::clamp             fixtures/rust-sample/src/conditions.rs:97  [function]
rust_sample::conditions::cleanup           fixtures/rust-sample/src/conditions.rs:20  [function]
rust_sample::conditions::conditional_loop  fixtures/rust-sample/src/conditions.rs:85  [function]
rust_sample::conditions::count_down        fixtures/rust-sample/src/conditions.rs:60  [function]
rust_sample::conditions::dispatch          fixtures/rust-sample/src/conditions.rs:42  [function]
rust_sample::conditions::expensive_check   fixtures/rust-sample/src/conditions.rs:28  [function]
rust_sample::conditions::log_error         fixtures/rust-sample/src/conditions.rs:12  [function]
rust_sample::conditions::log_info          fixtures/rust-sample/src/conditions.rs:4  [function]
rust_sample::conditions::log_warn          fixtures/rust-sample/src/conditions.rs:8  [function]
rust_sample::conditions::maybe_log         fixtures/rust-sample/src/conditions.rs:33  [function]
rust_sample::conditions::process_all       fixtures/rust-sample/src/conditions.rs:51  [function]
rust_sample::conditions::process_item      fixtures/rust-sample/src/conditions.rs:16  [function]
rust_sample::conditions::retry_until_ok    fixtures/rust-sample/src/conditions.rs:71  [function]
rust_sample::conditions::validate          fixtures/rust-sample/src/conditions.rs:24  [function]
freshness: current | indexed tree 5ea331d, working tree clean
```

### Filter by kind: show only the methods of a type

```
cgx search "AsyncService" \
  --kind method \
  --repo /path/to/worktree
```

```
rust_sample::async_calls::AsyncService::get   fixtures/rust-sample/src/async_calls.rs:38  [method]
rust_sample::async_calls::AsyncService::new   fixtures/rust-sample/src/async_calls.rs:34  [method]
rust_sample::async_calls::AsyncService::post  fixtures/rust-sample/src/async_calls.rs:43  [method]
freshness: current | indexed tree 5ea331d, working tree clean
```

### Regex match: enumerate all symbols in a module exactly

```
cgx search "^rust_sample::dataflow::" \
  --regex \
  --kind function \
  --repo /path/to/worktree
```

```
rust_sample::dataflow::assemble      fixtures/rust-sample/src/dataflow.rs:18  [function]
rust_sample::dataflow::field_base    fixtures/rust-sample/src/dataflow.rs:56  [function]
rust_sample::dataflow::flow_example  fixtures/rust-sample/src/dataflow.rs:5  [function]
rust_sample::dataflow::helper        fixtures/rust-sample/src/dataflow.rs:36  [function]
rust_sample::dataflow::project       fixtures/rust-sample/src/dataflow.rs:12  [function]
rust_sample::dataflow::reorder       fixtures/rust-sample/src/dataflow.rs:45  [function]
rust_sample::dataflow::select        fixtures/rust-sample/src/dataflow.rs:24  [function]
rust_sample::dataflow::through_call  fixtures/rust-sample/src/dataflow.rs:31  [function]
freshness: current | indexed tree 5ea331d, working tree clean
```

Unlike the default substring match, the `^` anchor pins the match to the start of the FQN. Without `--kind`, the result includes SSA value-node variables (e.g. `rust_sample::dataflow::flow_example::b#1`) — add `--kind function` (or `--kind type`) to restrict to the symbol kinds you care about.

### JSON output

```
cgx search "AsyncService" \
  --kind method \
  --format json \
  --repo /path/to/worktree
```

```json
{
  "freshness": {
    "dirty_files": 0,
    "dirty_files_base": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "head_tree": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "indexed_tree": "5ea331d1c4fb42f4a1469ae5c4ced68dae74ef6a",
    "matches_head": true,
    "stale": false
  },
  "results": [
    {
      "file": "fixtures/rust-sample/src/async_calls.rs",
      "fqn": "rust_sample::async_calls::AsyncService::get",
      "kind": "method",
      "line": 38
    },
    {
      "file": "fixtures/rust-sample/src/async_calls.rs",
      "fqn": "rust_sample::async_calls::AsyncService::new",
      "kind": "method",
      "line": 34
    },
    {
      "file": "fixtures/rust-sample/src/async_calls.rs",
      "fqn": "rust_sample::async_calls::AsyncService::post",
      "kind": "method",
      "line": 43
    }
  ]
}
```

The hits live under `results`; `freshness` is the index-freshness envelope every
answer document carries — which tree the answer was computed over, whether that is
`HEAD`'s tree, and how many working-tree files diverge from `dirty_files_base`
(`null` there means "not established", never zero).

### Selector match: globstar + intra-segment affix

```
cgx search --repo /path/to/worktree 'rust_sample::**::*_check'
```

```
rust_sample::cfg_feature::legacy_auth_check            fixtures/rust-sample/src/cfg_feature.rs:13  [function]
rust_sample::cfg_feature::new_auth_check               fixtures/rust-sample/src/cfg_feature.rs:9  [function]
rust_sample::conditions::expensive_check               fixtures/rust-sample/src/conditions.rs:28  [function]
rust_sample::dead_code::UnusedService::internal_check  fixtures/rust-sample/src/dead_code.rs:30  [method]
freshness: current | indexed tree 9e36ae7, working tree clean
```

`**` matches zero-or-more whole segments between `rust_sample` and any final segment
ending in `_check`, at any depth — including `dead_code::UnusedService::internal_check`,
three segments down.

### Selector match: negation

```
cgx search --repo /path/to/worktree 'rust_sample::conditions::!log_*' --kind function
```

```
rust_sample::conditions::clamp             fixtures/rust-sample/src/conditions.rs:97  [function]
rust_sample::conditions::cleanup           fixtures/rust-sample/src/conditions.rs:20  [function]
rust_sample::conditions::conditional_loop  fixtures/rust-sample/src/conditions.rs:85  [function]
rust_sample::conditions::count_down        fixtures/rust-sample/src/conditions.rs:60  [function]
rust_sample::conditions::dispatch          fixtures/rust-sample/src/conditions.rs:42  [function]
rust_sample::conditions::expensive_check   fixtures/rust-sample/src/conditions.rs:28  [function]
rust_sample::conditions::maybe_log         fixtures/rust-sample/src/conditions.rs:33  [function]
rust_sample::conditions::process_all       fixtures/rust-sample/src/conditions.rs:51  [function]
rust_sample::conditions::process_item      fixtures/rust-sample/src/conditions.rs:16  [function]
rust_sample::conditions::retry_until_ok    fixtures/rust-sample/src/conditions.rs:71  [function]
rust_sample::conditions::validate          fixtures/rust-sample/src/conditions.rs:24  [function]
freshness: current | indexed tree 9e36ae7, working tree clean
```

Every `conditions::` function except the three whose name starts with `log_`
(`log_info`, `log_warn`, `log_error`). Note the whole selector is single-quoted —
`!log_*` would otherwise hit interactive shell history expansion (see
[Quoting your selector](#quoting-your-selector-shell-not-cgx)).

### Selector match: alternation

```
cgx search --repo /path/to/worktree 'rust_sample::conditions::(log_info|log_warn)'
```

```
rust_sample::conditions::log_info  fixtures/rust-sample/src/conditions.rs:4  [function]
rust_sample::conditions::log_warn  fixtures/rust-sample/src/conditions.rs:8  [function]
freshness: current | indexed tree 9e36ae7, working tree clean
```

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Success. Results found, or no results found (empty). Both are normal — `search` is the discovery surface, not the exact-symbol resolver. |
| `2` | Bad input: neither `PATTERN` nor `--all` given (or both); `--regex` given with an invalid regular expression; a selector-grammar `PATTERN` with zero valid parses (no registered tokenizer accepted it) or using `?`; inaccessible `--repo`; or `--format sarif` (or `dot`, `mermaid`, `d2`) given. |
| `3` | No index present and `--no-auto-index` was given. |

`search` does not support `--assert-empty`. There is no exit 1 or exit 4 for this command.

## See also

- [cgx callers](callers.md) — walk callers of a symbol once you have its exact FQN
- [cgx callees](callees.md) — walk callees of a symbol once you have its exact FQN
- [cgx explain](explain.md) — full provenance for one symbol including all incident edges
- [cgx flows-to](flows-to.md) — forward dataflow slice; use `cgx search` to find value-node FQNs first
- [cgx flows-from](flows-from.md) — backward dataflow pedigree; use `cgx search` to find value-node FQNs first
- [03-code-graph-model.md](../03-code-graph-model.md) — symbol kinds, FQN conventions, and the graph data model
