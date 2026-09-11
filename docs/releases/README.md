# Release notes

Every release ships with a human-written description. It lives here, one file
per tag:

```
docs/releases/<tag>.md      e.g. docs/releases/v0.3.0.md
```

The release workflow (`.github/workflows/release.yml`) reads this file **from
`main`** and uses it verbatim — plus a one-line Change-Date footnote it appends
automatically — as **both** the stamp commit's message and the GitHub Release
body. There is no "Automated release build" fallback: **if the file is missing
or empty, the release run fails.** Write it before you release.

## Two ways to release

### Forward release (normal) — push a tag

1. Write `docs/releases/vX.Y.Z.md` describing the release, and commit it to
   `main`.
2. Tag the release commit `vX.Y.Z` and push the tag.

The workflow stamps the dated `LICENSE`, (re)creates `vX.Y.Z` as an annotated
tag at the stamp commit, builds the reproducible binary, and publishes the
Release with your notes. The Change Date is the tag's publication date + 4
years.

### Backfill / cut from an arbitrary commit — run the workflow manually

For a release whose cleave point is an old commit (e.g. one that predates this
workflow), don't push a tag — a tag-push only runs the workflow version present
*at the tagged commit*. Instead run it from **Actions → release → Run workflow**
(`workflow_dispatch`) with:

- **version** — the tag to create, e.g. `v0.1.0`
- **cleave_sha** — the commit (SHA or ref) to cut the release from
- **publication_date** — `YYYY-MM-DD`; Change Date = this + 4 years
  (defaults to `2026-09-10`, the repo's public-launch date)

The notes at `docs/releases/<version>.md` must already be on `main`. Everything
else is identical to a forward release; the tag is created annotated, dated to
`publication_date`.

## Change Date basis

The BUSL-1.1 Change Date is keyed to **publication**, not code authorship: it is
the publication date + 4 years. For a forward tag-push that's the tag's own
date; for a backfill it's the `publication_date` input. So all releases cut at
launch share the same Change Date regardless of how old their code is.

The workflow appends this footnote after your content — you don't write it:

```
---

This release's BUSL-1.1 Change Date is stamped YYYY-MM-DD 00:00:00Z (converts to MIT on that date).
```

## Tips

- Keep the first line a short title-style summary; GitHub shows it prominently.
- Reference PRs/issues with `#123`; GitHub links them on the Release page.
- The stamp commit is a mint artifact (child of the cleave point, never merged
  to `main`); `main` is never touched by a release.
