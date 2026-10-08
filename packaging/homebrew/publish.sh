#!/usr/bin/env bash
# Puts a rendered formula into a checkout of the tap and pushes it, never
# moving the tap backwards:
#
#   packaging/homebrew/publish.sh TAP_DIR FORMULA X.Y.Z
#
# - The tap's Formula/mailtriage.rb has a higher version: nothing changes.
# - It has this version and the same file: nothing to commit.
# - Otherwise it commits "mailtriage X.Y.Z" and pushes. When the push is
#   refused because the tap moved meanwhile, it fetches, decides again and
#   tries once more. Any other failed step fails the script at once.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $0 TAP_DIR FORMULA X.Y.Z" >&2
  exit 2
fi
tap=$1
formula=$2
version=$3
target="$tap/Formula/mailtriage.rb"
branch="$(git -C "$tap" rev-parse --abbrev-ref HEAD)"

# Whether version $1 is higher than version $2 (X.Y.Z, compared as numbers).
higher() {
  python3 - "$1" "$2" <<'PY'
import sys
def parts(v):
    return tuple(int(p) for p in v.split('.'))
sys.exit(0 if parts(sys.argv[1]) > parts(sys.argv[2]) else 1)
PY
}

# The tap's formula version, empty without one.
current() {
  if [ -f "$target" ]; then
    sed -n 's/^  version "\(.*\)"$/\1/p' "$target" | head -n 1
  fi
}

# Commits and pushes when needed. Returns 0 when the tap is up to date, 3
# when the push failed (as it does when the tap moved meanwhile), and 1
# when any other step failed. Every step returns explicitly: errexit does
# not apply inside a function called from `||`.
attempt() {
  local now
  now="$(current)" || return 1
  if [ -n "$now" ] && higher "$now" "$version"; then
    echo "::notice::the tap has mailtriage $now, higher than $version; it keeps it"
    return 0
  fi
  if [ "$now" = "$version" ] && cmp -s "$formula" "$target"; then
    echo "the tap already has this formula for mailtriage $version"
    return 0
  fi
  mkdir -p "$tap/Formula" || return 1
  cp "$formula" "$target" || return 1
  git -C "$tap" add Formula/mailtriage.rb || return 1
  git -C "$tap" -c user.name='github-actions[bot]' \
    -c user.email='41898282+github-actions[bot]@users.noreply.github.com' \
    commit -q -m "mailtriage $version" || return 1
  git -C "$tap" push -q origin "HEAD:$branch" || return 3
}

status=0
attempt || status=$?
if [ "$status" -eq 3 ]; then
  echo "the tap moved meanwhile; fetching and trying once more"
  git -C "$tap" fetch -q origin "$branch"
  git -C "$tap" reset -q --hard "origin/$branch"
  attempt
elif [ "$status" -ne 0 ]; then
  exit "$status"
fi
