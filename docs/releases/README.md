# Release notes

Every release ships with a human-written description. It lives here, one file
per tag:

```
docs/releases/<tag>.md      e.g. docs/releases/v0.3.0.md
```

The release workflow (`.github/workflows/release.yml`) reads this file and uses
it verbatim — plus a one-line Change-Date footnote it appends automatically —
as **both** the stamp commit's message and the GitHub Release body. There is no
"Automated release build" fallback: **if the file is missing or empty, the
release run fails.** Write it before you tag.

## Process

1. Write `docs/releases/<tag>.md` describing what the release is about — what
   changed, what's new, anything a consumer needs to know. Plain Markdown; it
   renders on the GitHub Release page. Say what the release is, not that it was
   built by CI (the commit and release are already attributed to
   `github-actions` / `cgx-release-bot`).
2. Commit it to `main` along with the rest of the release.
3. Tag the release commit `vX.Y.Z` and push the tag. The workflow stamps the
   dated BUSL-1.1 `LICENSE`, moves the tag to the stamp commit, builds the
   reproducible binary, and publishes the Release with your notes as its body.

## What the workflow appends

After your content, the workflow adds:

```
---

This release's BUSL-1.1 Change Date is stamped YYYY-MM-DD 00:00:00Z (converts to MIT on that date).
```

The date is the tagged commit's authored date + 4 years — the same date it
writes into `LICENSE`. You don't write this line; don't duplicate it.

## Tips

- Keep the first line a short title-style summary; GitHub shows it prominently.
- Reference PRs/issues with `#123`; GitHub links them on the Release page.
- The file is read from the *tagged commit*, so edits made after tagging are
  not picked up unless you re-tag.
