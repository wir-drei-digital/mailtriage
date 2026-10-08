#!/bin/sh
# Renders the Homebrew formula of release X.Y.Z from its SHA256SUMS:
#
#   packaging/homebrew/render.sh X.Y.Z SHA256SUMS > mailtriage.rb
#
# Fails, printing nothing on stdout, when one of the four archives the
# formula names (the CLI for macos-arm64, linux-amd64 and linux-arm64, the
# tray for macos-arm64) is missing from SHA256SUMS, listed twice, or listed
# with a malformed line.
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: $0 X.Y.Z SHA256SUMS" >&2
  exit 2
fi
version=$1
sums=$2
here=$(cd "$(dirname "$0")" && pwd)
if ! printf '%s\n' "$version" | grep -Eqx '(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'; then
  echo "render: $version is not X.Y.Z" >&2
  exit 2
fi
[ -f "$sums" ] || {
  echo "render: no $sums" >&2
  exit 1
}

# The SHA-256 SHA256SUMS lists for the archive "$1-v$version-$2.tar.gz".
digest() {
  name="$1-v$version-$2.tar.gz"
  count=$(awk -v name="$name" '$2 == name' "$sums" | wc -l | tr -d ' ')
  if [ "$count" != 1 ]; then
    echo "render: SHA256SUMS has $count lines for $name" >&2
    exit 1
  fi
  found=$(awk -v name="$name" 'length($1) == 64 && $1 ~ /^[0-9a-f]+$/ && $0 == $1 "  " name { print $1 }' "$sums")
  if [ -z "$found" ]; then
    echo "render: SHA256SUMS has a malformed line for $name" >&2
    exit 1
  fi
  printf '%s' "$found"
}

macos=$(digest mailtriage macos-arm64)
tray=$(digest mailtriage-tray macos-arm64)
amd64=$(digest mailtriage linux-amd64)
arm64=$(digest mailtriage linux-arm64)
sed -e "s/@VERSION@/$version/g" \
  -e "s/@SHA256_MACOS_ARM64@/$macos/g" \
  -e "s/@SHA256_TRAY_MACOS_ARM64@/$tray/g" \
  -e "s/@SHA256_LINUX_AMD64@/$amd64/g" \
  -e "s/@SHA256_LINUX_ARM64@/$arm64/g" \
  "$here/mailtriage.rb.in"
