# cgx configure-remote

Configure a git remote so `refs/cgx/*` travels with plain `git fetch` and `git push`.

## Synopsis

```
cgx configure-remote [REMOTE]
```

## Description

A default clone or `git fetch` only brings down the refs its refspec names, which excludes `refs/cgx/*`. `cgx configure-remote` adds

```
remote.<remote>.fetch = +refs/cgx/*:refs/cgx/*
remote.<remote>.push  = +refs/cgx/*:refs/cgx/*
```

to the git config of the repository in the current directory. It is idempotent: a refspec already present is not added twice.

You do not need it for [`cgx pull`](pull.md) or [`cgx push`](push.md), which pass explicit refspecs. It matters for anything that runs a bare `git fetch` — a CI checkout step, an editor's background fetch, a teammate's manual fetch.

**It edits this repository's config only.** It cannot change a CI runner's own git config. A runner that should see `refs/cgx/*` through plain git needs the same line in its own config:

```
[remote "origin"]
    fetch = +refs/cgx/*:refs/cgx/*
```

It writes the config entries whether or not the named remote exists, and does not contact it. Configuring a name before adding the remote leaves a `remote.<name>` section with no URL, which makes a later `git remote add <name> …` fail with `remote <name> already exists`; set the URL with `git remote set-url` instead, or add the remote first.

`configure-remote` takes no `--repo`; run it from inside the repository.

## Arguments

| Argument | Required | Meaning |
|----------|----------|---------|
| `REMOTE` | no | Remote name to configure. Defaults to `origin`. |

## Options

`configure-remote` takes no options beyond `--help`.

## Examples

```
cgx configure-remote
```

```
configured remote origin: fetch/push refspec +refs/cgx/*:refs/cgx/* (idempotent)
```

```
git config --get-all remote.origin.fetch
```

```
+refs/heads/*:refs/remotes/origin/*
+refs/cgx/*:refs/cgx/*
```

## Exit codes

| Code | Condition |
|------|-----------|
| `0` | Refspecs present in the config (added now or already there). |
| `2` | Bad argument. |
| `3` | The underlying `git config` command failed — for example outside a git repository (`fatal: not in a git directory`). |

## See also

- [cgx push](push.md) — publish the local index
- [cgx pull](pull.md) — fetch and materialize a pushed index
- [06-indexing-and-vcs.md](../06-indexing-and-vcs.md) — the `refs/cgx/index` overlay and its portability posture
