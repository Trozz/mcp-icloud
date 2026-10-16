#!/usr/bin/env bash
# Tests for next-version.sh. Run: bash scripts/test-next-version.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/next-version.sh
source "$here/next-version.sh"

fail=0
check() {
  local name="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    echo "ok   $name"
  else
    echo "FAIL $name: expected '$expected', got '$actual'"
    fail=1
  fi
}

field() {
  grep "^$1=" <<<"$2" | head -n1 | cut -d= -f2-
}

check "labels: highest wins" major "$(resolve_bump 'minor,major,patch' '')"
check "labels: case insensitive" minor "$(resolve_bump 'Minor' '')"
check "label beats title" minor "$(resolve_bump 'minor' 'Release [major]')"
check "title brackets" major "$(resolve_bump '' 'Release [major]')"
check "title highest wins" minor "$(resolve_bump '' '[patch] and [minor] release')"
check "title case insensitive" minor "$(resolve_bump '' 'Release [MINOR]')"
check "unrelated labels fall back to title" major "$(resolve_bump 'bug,docs' '[major] x')"
check "default patch" patch "$(resolve_bump 'bug,docs' 'Release v2')"
check "apply patch from zero" 0.0.1 "$(apply_bump 0.0.0 patch)"
check "apply minor resets patch" 1.3.0 "$(apply_bump 1.2.7 minor)"
check "apply major resets minor and patch" 2.0.0 "$(apply_bump 1.2.7 major)"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
git init -q -b main
git config user.email test@example.com
git config user.name test
mkdir -p crates/mcp-icloud-mail docs
echo a > crates/mcp-icloud-mail/a.rs
git add .
git commit -qm "feat: initial"

check "no tags: no stable tag" "" "$(latest_stable_tag mail)"
out="$(BRANCH=main PR_TITLE='' PR_LABELS='' GITHUB_OUTPUT=/dev/null main)"
check "no tags: version 0.0.1" 0.0.1 "$(field version "$out")"
check "no tags: release" true "$(field release "$out")"
check "no tags: empty range" "" "$(field range "$out")"
check "no tags: tag name" mail-v0.0.1 "$(field tag "$out")"

git tag mail-v1.4.2
git tag mail-v1.5.0-dev.7
git tag calendar-v9.0.0
check "stable tag excludes dev and other prefixes" mail-v1.4.2 "$(latest_stable_tag mail)"

echo docs > docs/readme.md
git add docs
git commit -qm "docs: update"
out="$(BRANCH=main PR_TITLE='' PR_LABELS='' GITHUB_OUTPUT=/dev/null main)"
check "docs-only change: no release" false "$(field release "$out")"

echo b > crates/mcp-icloud-mail/a.rs
git add crates
git commit -qm "fix: change"
out="$(BRANCH=main PR_TITLE='Release' PR_LABELS='minor,major' GITHUB_OUTPUT=/dev/null main)"
check "conflicting labels pick major" 2.0.0 "$(field version "$out")"
check "crate change: release" true "$(field release "$out")"
check "range from last stable" "mail-v1.4.2..HEAD" "$(field range "$out")"
check "previous stable" mail-v1.4.2 "$(field previous "$out")"

out="$(BRANCH=develop RUN_NUMBER=12 PR_TITLE='' PR_LABELS='' GITHUB_OUTPUT=/dev/null main)"
check "develop pre-release" 1.4.3-dev.12 "$(field version "$out")"
check "develop tag" mail-v1.4.3-dev.12 "$(field tag "$out")"

git tag mail-v1.4.3
out="$(BRANCH=main PR_TITLE='' PR_LABELS='' GITHUB_OUTPUT=/dev/null main)"
check "existing tag: no release" false "$(field release "$out")"

exit "$fail"
