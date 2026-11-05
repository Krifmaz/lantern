#!/usr/bin/env bash
# Brings an upstream web frontend into this repository as a snapshot fork, at the commit UPSTREAM.lock pins.
#
# How it works (see UPSTREAM.md):
#   1. Fetches only the requested upstream commit (shallow) into a cache repository, .upstream/<project>.git
#      (git-ignored), so this repository never gets the upstream's history or its other folders.
#   2. Builds a snapshot commit there whose tree holds only the tracked paths, moved under the fork's folder
#      (for Prowlarr: frontend/ and the root build files, under ui/prowlarr/; for autobrr: the contents of web/
#      and the licence, under ui/autobrr/), with the previous snapshot as its parent, and keeps it as branch upstream/<project>: a straight line of upstream snapshots.
#   3. Merges the snapshot into the current branch with a three-way merge whose base is the snapshot of the
#      commit imported last (named in <folder>/UPSTREAM), so only the upstream's changes since then are
#      applied on top of ours. The base is rebuilt from the upstream itself, so this also works after the
#      import's merge was squashed.
#
# Exit status: 0 merged (or already up to date), 1 conflicts (the merge is left in progress, files listed),
# 2 wrong use or a precondition failed (nothing changed).
#
# Usage: scripts/upstream-merge.sh [--project prowlarr|autobrr] [--ref REF] [--repo URL] [--no-merge]
#                                  [--abort-on-conflict]
#   --project NAME       the section of UPSTREAM.lock to import: prowlarr (default) or autobrr
#   --ref REF            upstream tag, branch or commit (default: the commit UPSTREAM.lock pins)
#   --repo URL           upstream repository (default: UPSTREAM.lock's repo; file:// for testing)
#   --no-merge           only update upstream/<project>; merge it yourself later
#   --abort-on-conflict  list the conflicts, then leave everything as it was (for CI)
set -euo pipefail

project="prowlarr"
ref=""
repo_url=""
do_merge=1
abort_on_conflict=0
lock_file="UPSTREAM.lock"

die() {
    printf 'upstream-merge: %s\n' "$1" >&2
    exit 2
}

usage() {
    sed -n '2,24p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 2
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --project) [[ $# -ge 2 ]] || usage; project="$2"; shift 2 ;;
        --ref) [[ $# -ge 2 ]] || usage; ref="$2"; shift 2 ;;
        --repo) [[ $# -ge 2 ]] || usage; repo_url="$2"; shift 2 ;;
        --no-merge) do_merge=0; shift ;;
        --abort-on-conflict) abort_on_conflict=1; shift ;;
        -h | --help) usage ;;
        *) die "unknown argument: $1 (see --help)" ;;
    esac
done

# What each project's fork takes from the upstream, and where it goes. To fork another frontend, add a case.
# A path that ends in / brings that folder's contents, not the folder itself, straight into $dest.
case "$project" in
    prowlarr)
        dest="ui/prowlarr"
        # frontend/ and the root files its build reads: the packages and their lock, the TypeScript and yarn
        # settings, and the licence.
        paths=(frontend package.json yarn.lock tsconfig.json .yarnrc .editorconfig LICENSE)
        ;;
    autobrr)
        dest="ui/autobrr"
        # web/ is a whole Vite app of its own (package.json, pnpm-lock.yaml, pnpm-workspace.yaml, the Vite,
        # Tailwind and TypeScript settings, src/ and public/): its contents, plus the licence from the root.
        paths=(web/ LICENSE)
        ;;
    *) die "unknown project: $project (known: prowlarr, autobrr)" ;;
esac
branch="upstream/$project"

top="$(git rev-parse --show-toplevel 2>/dev/null)" || die "not inside a git repository"
cd -- "$top"

# The pin: [<project>] repo and commit in UPSTREAM.lock.
lock_value() {
    awk -v section="[$project]" -v key="$1" '
        /^\[/ { inside = ($0 == section) }
        inside && $1 == key && $2 == "=" { v = $3; gsub(/"/, "", v); print v; exit }
    ' "$lock_file"
}
[[ -f "$lock_file" ]] || die "$lock_file is missing"
pinned="$(lock_value commit)"
[[ -n "$repo_url" ]] || repo_url="$(lock_value repo)"
[[ -n "$ref" ]] || ref="$pinned"
[[ -n "$ref" && -n "$repo_url" ]] || die "$lock_file has no [$project] repo and commit"

# Everything that reaches git is checked first, so nothing can be read as an option or a path trick.
[[ "$ref" =~ ^[A-Za-z0-9][A-Za-z0-9._/-]{0,200}$ && "$ref" != *..* ]] || die "invalid --ref: $ref"
[[ "$repo_url" =~ ^(https://|file://|/)[A-Za-z0-9._~:/@%+-]+$ ]] || die "invalid --repo: $repo_url"

git rev-parse --verify --quiet HEAD >/dev/null || die "the repository has no commits"
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
    die "the working tree has changes that are not committed; commit them first"
fi
if git rev-parse --verify --quiet MERGE_HEAD >/dev/null; then
    die "a merge is already in progress; finish it or run git merge --abort"
fi

cache="$top/.upstream/$project.git"
if [[ ! -d "$cache" ]]; then
    mkdir -p -- "$top/.upstream"
    git init --quiet --bare -- "$cache"
fi
ucache() { git --git-dir="$cache" "$@"; }
bot=(-c user.name="Lantern upstream import" -c user.email="upstream@lantern.invalid")

# fetch_commit REF: the commit REF names upstream, fetched shallowly into the cache.
fetch_commit() {
    ucache fetch --quiet --depth 1 --no-tags -- "$repo_url" "$1" || die "fetching $1 from $repo_url failed"
    ucache rev-parse --verify 'FETCH_HEAD^{commit}'
}

# snapshot_tree SHA: a tree holding only the tracked paths of upstream commit SHA, under $dest.
snapshot_tree() {
    local sha="$1" entries="" p line t
    for p in "${paths[@]}"; do
        if [[ "$p" == */ ]]; then
            # the folder's own entries
            line="$(ucache ls-tree "$sha:${p%/}" 2>/dev/null || true)"
        else
            line="$(ucache ls-tree "$sha" -- "$p")"
        fi
        if [[ -n "$line" ]]; then
            entries+="$line"$'\n'
        else
            echo "warning: the upstream has no $p at $sha" >&2
        fi
    done
    [[ -n "$entries" ]] || die "none of the tracked paths exist upstream at $sha"
    t="$(printf '%s' "$entries" | ucache mktree)"
    # wrap it in each folder of $dest, innermost first
    local IFS=/ parts i
    read -r -a parts <<<"$dest"
    for ((i = ${#parts[@]} - 1; i >= 0; i--)); do
        t="$(printf '040000 tree %s\t%s\n' "$t" "${parts[$i]}" | ucache mktree)"
    done
    printf '%s' "$t"
}

echo "Fetching $ref from $repo_url ..."
upstream_sha="$(fetch_commit "$ref")"
upstream_date="$(ucache log -1 --format=%cI "$upstream_sha")"
echo "Upstream commit: $upstream_sha ($upstream_date)"
if [[ -n "$pinned" && "$upstream_sha" != "$pinned" ]]; then
    echo "note: $lock_file pins $pinned; change the pin (and docs/PARITY.md) in its own pull request." >&2
fi

# The previous snapshot: the local branch, else the one on origin (when it was pushed).
prev=""
if git rev-parse --verify --quiet "refs/heads/$branch" >/dev/null; then
    prev="$(git rev-parse "refs/heads/$branch")"
elif git fetch --quiet --no-tags origin "refs/heads/$branch:refs/heads/$branch" 2>/dev/null; then
    prev="$(git rev-parse "refs/heads/$branch")"
fi
prev_upstream=""
[[ -n "$prev" ]] && prev_upstream="$(git log -1 --format=%B "$prev" | sed -n 's/^Upstream-Commit: //p' | head -n1)"

if [[ -n "$prev" && "$prev_upstream" == "$upstream_sha" ]]; then
    snapshot="$prev"
    echo "$branch is already at $upstream_sha"
else
    [[ -n "$prev" ]] && ucache fetch --quiet --no-tags -- "$top" "refs/heads/$branch:refs/snap/prev"
    tree="$(snapshot_tree "$upstream_sha")"
    msg="$(printf 'Upstream snapshot of %s at %s\n\nInto: %s/\nPaths: %s\nUpstream-Repo: %s\nUpstream-Commit: %s\nUpstream-Date: %s\n' \
        "$project" "${upstream_sha:0:12}" "$dest" "${paths[*]}" "$repo_url" "$upstream_sha" "$upstream_date")"
    parent_args=()
    [[ -n "$prev" ]] && parent_args=(-p "$prev")
    snapshot="$(GIT_AUTHOR_DATE="$upstream_date" GIT_COMMITTER_DATE="$upstream_date" \
        ucache "${bot[@]}" commit-tree "$tree" "${parent_args[@]}" -m "$msg")"
    ucache update-ref refs/snap/new "$snapshot"
    git fetch --quiet --no-tags -- "$cache" "+refs/snap/new:refs/heads/$branch"
    echo "Updated $branch -> ${snapshot:0:12}"
fi

if [[ $do_merge -eq 0 ]]; then
    echo "--no-merge: $branch is updated; run the script again without --no-merge to merge it."
    exit 0
fi

# The base: the snapshot of the commit imported last, rebuilt from the upstream (works after a squash merge).
marker="$dest/UPSTREAM"
base_sha=""
if git cat-file -e "HEAD:$marker" 2>/dev/null; then
    base_sha="$(git show "HEAD:$marker" | sed -n 's/^commit //p' | head -n1)"
    [[ "$base_sha" =~ ^[0-9a-f]{40}$ ]] || die "$marker names no commit"
fi
if [[ "$base_sha" == "$upstream_sha" ]]; then
    echo "$dest is already at $upstream_sha; nothing to do."
    exit 0
fi

merge_args=(--write-tree --no-messages)
if [[ -n "$base_sha" ]]; then
    base_commit="$(ucache "${bot[@]}" commit-tree "$(snapshot_tree "$(fetch_commit "$base_sha")")" -m base)"
    ucache update-ref refs/snap/base "$base_commit"
    git fetch --quiet --no-tags -- "$cache" "+refs/snap/base:refs/upstream-base/$project"
    merge_args+=(--merge-base="refs/upstream-base/$project")
elif git cat-file -e "HEAD:$dest" 2>/dev/null; then
    die "$dest exists but $marker does not say which upstream commit it holds; write 'commit <sha>' into it"
else
    echo "First import into $dest/."
    merge_args+=(--allow-unrelated-histories)
fi

set +e
out="$(git merge-tree "${merge_args[@]}" HEAD "$snapshot")"
merge_status=$?
set -e
[[ -n "$base_sha" ]] && git update-ref -d "refs/upstream-base/$project"
[[ $merge_status -le 1 ]] || die "git merge-tree failed: $out"
result_tree="$(printf '%s\n' "$out" | head -n1)"
# after the tree: one "<mode> <object> <stage><TAB><path>" line per side of each conflicted file
stages="$(printf '%s\n' "$out" | tail -n +2 | sed '/^$/,$d')"
mapfile -t conflicts < <(printf '%s\n' "$stages" | sed '/^$/d' | cut -f2- | sort -u)

if [[ ${#conflicts[@]} -gt 0 && $abort_on_conflict -eq 1 ]]; then
    printf 'Conflicts merging upstream %s (nothing was changed):\n' "${upstream_sha:0:12}"
    printf '    %s\n' "${conflicts[@]}"
    exit 1
fi

# Apply the merged tree, name the upstream commit, and leave a merge in progress (git commit finishes it).
git read-tree -u -m HEAD "$result_tree"
printf '# The upstream commit %s/ holds (written by scripts/upstream-merge.sh).\ncommit %s\n' \
    "$dest" "$upstream_sha" >"$marker"
git add -- "$marker"
printf '%s\n' "$snapshot" >"$(git rev-parse --git-path MERGE_HEAD)"
printf 'Merge upstream %s %s into %s/\n\nUpstream-Commit: %s\n' \
    "$project" "${upstream_sha:0:12}" "$dest" "$upstream_sha" >"$(git rev-parse --git-path MERGE_MSG)"
printf "no-ff" >"$(git rev-parse --git-path MERGE_MODE)"

if [[ ${#conflicts[@]} -gt 0 ]]; then
    # the files with conflict markers become unmerged entries, as after git merge: git status lists them,
    # git commit waits until they are added, git merge --abort puts them back
    {
        for f in "${conflicts[@]}"; do printf '0 0000000000000000000000000000000000000000\t%s\n' "$f"; done
        printf '%s\n' "$stages"
    } | git update-index --index-info
    printf '\nConflicts merging upstream %s, in %d file(s):\n' "${upstream_sha:0:12}" "${#conflicts[@]}"
    printf '    %s\n' "${conflicts[@]}"
    echo
    echo "Resolve them, then: git add <files> && git commit --no-edit"
    echo "Or give up with: git merge --abort"
    exit 1
fi

git commit --quiet --no-edit
echo "Merged upstream $project ${upstream_sha:0:12} into $(git rev-parse --abbrev-ref HEAD) ($(git rev-parse --short HEAD))."
