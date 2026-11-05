# Keeping the forked frontends up to date

`ui/prowlarr/` is a **snapshot fork** of [Prowlarr](https://github.com/Prowlarr/Prowlarr)'s web frontend: its
`frontend/` folder and the root files its build reads (`package.json`, `yarn.lock`, `tsconfig.json`, `.yarnrc`,
`.editorconfig`, `LICENSE`), at the commit [`UPSTREAM.lock`](UPSTREAM.lock) pins in `[prowlarr]`. Lantern keeps
it as it is; its own changes are small, come as separate commits on top of each import, and are listed in
[`ui/prowlarr/LANTERN.md`](ui/prowlarr/LANTERN.md) so a conflict is easy to judge.

## How it works

`scripts/upstream-merge.sh` keeps a branch **`upstream/prowlarr`** whose commits are *snapshots*: only the
tracked paths of one upstream commit, already moved under `ui/prowlarr/`.

```text
Prowlarr/Prowlarr ── A ─── B ─── C            (full upstream history; never fetched in full)
                     ┆     ┆     ┆   snapshot = frontend/ and the build files, under ui/prowlarr/
upstream/prowlarr ── a ─── b ─── c            (straight line, one commit per import)
                      ╲     ╲     ╲
main ───────────────── M1 ── M2 ── M3         (merges into our history)
```

1. The upstream commit is fetched **shallowly** (`--depth 1`) into a cache repository, `.upstream/prowlarr.git`
   (git-ignored), so this repository never gets Prowlarr's history, its C# code or its other folders.
2. A snapshot commit is built there from the tracked paths (`git mktree`), with the previous snapshot as its
   parent and the upstream commit's date, and fetched into this repository as `upstream/prowlarr`. Its message
   records `Upstream-Repo`, `Upstream-Commit` and `Upstream-Date`. The same upstream commit always gives the same
   snapshot commit.
3. The snapshot is merged into the current branch with a three-way merge (`git merge-tree`) whose base is the
   snapshot of the commit imported last. That commit is named in `ui/prowlarr/UPSTREAM` (written by the script),
   and its snapshot is rebuilt from the upstream, so the merge base is right even after a pull request was
   squash-merged and the link between `main` and `upstream/prowlarr` is gone. Only the upstream's changes since
   the last import are applied on top of ours.
4. The result is committed as a merge of `upstream/prowlarr`, with `ui/prowlarr/UPSTREAM` updated.

A snapshot holds only `ui/prowlarr/…`, so a merge never touches `server/`, `web/`, `docs/` or anything else of
ours. Upstream deletions and renames inside the tracked paths are applied like any other change. Only git and bash
are needed.

## Updating

The pin in `UPSTREAM.lock` comes first: change it (and `docs/PARITY.md`) in its own pull request, as the lock
says. Then:

```sh
git switch -c claude/<thread>-prowlarr-ui-sync origin/main
scripts/upstream-merge.sh                      # the commit UPSTREAM.lock pins
scripts/upstream-merge.sh --ref v2.8.0.5800    # or another tag, branch or commit (prints a note)
git push origin upstream/prowlarr              # keep the snapshot line on the remote, when allowed
```

Then build and check the frontend (`cd ui/prowlarr && yarn install --frozen-lockfile && yarn build`), take the
screenshots in `docs/parity/p-ui/` again and open a pull request.

| Exit status | Meaning |
| --- | --- |
| `0` | Merged and committed, or already up to date. |
| `1` | Conflicts. The merge is left in progress: `git status` lists the files (`UU`), `git commit` waits until they are resolved and added, `git merge --abort` gives up. |
| `2` | Wrong use or a precondition failed (changes not committed, a merge in progress, a bad argument, the fetch failed). Nothing was changed. |

| Option | Effect |
| --- | --- |
| `--project NAME` | The `UPSTREAM.lock` section to import (default `prowlarr`; the only one so far). |
| `--ref REF` | Upstream tag, branch or commit (default: the commit `UPSTREAM.lock` pins). |
| `--repo URL` | Upstream repository (default: the lock's `repo`; `file://` for testing). |
| `--no-merge` | Only update `upstream/prowlarr`. |
| `--abort-on-conflict` | List the conflicts and change nothing (for CI). |

## The first import

The first import is Prowlarr v2.7.0.5723, commit `0dc20d8495bad9951c3fafe53f32b6e1c70fcf3b` (2026-10-26),
merged unmodified (with `--allow-unrelated-histories`, as `ui/prowlarr/` did not exist). Lantern's changes follow
as separate commits: the Encore look, the branding and links, and the build under `/prowlarr/`.

## Rules for fork changes

- Keep them small and grouped; prefer adding a file (a Lantern stylesheet, a theme file) to editing upstream's.
- Mark each edit in upstream's files with a `LANTERN:` comment and list it in `ui/prowlarr/LANTERN.md`.
- Keep the frontend talking Prowlarr's API: Lantern's `/api/v1` answers it, so the fork needs no API changes.
