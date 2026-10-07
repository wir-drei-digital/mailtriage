#!/usr/bin/env bash
# Adds a Himalaya version to src/engine/himalaya-versions.json:
#
#   scripts/add-himalaya-version.sh X.Y.Z
#
# The digests come from the asset `digest` fields of pimalaya/himalaya's
# release vX.Y.Z (one per platform mailtriage needs; a missing one fails and
# changes nothing). `roles` is copied from the newest listed version: read
# the new version's `--mailbox` resolver and correct it by hand when it
# changed. The version must be a stable release higher than every listed
# one.
#
# Environment: GH_TOKEN or GITHUB_TOKEN is sent to the GitHub API when set.
# For tests, HIMALAYA_VERSIONS_FILE replaces the data file and
# HIMALAYA_RELEASE_JSON names a file with the release JSON instead of the API.
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 X.Y.Z" >&2
  exit 2
fi
version=$1
if ! printf '%s\n' "$version" | grep -Eqx '(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'; then
  echo "add-himalaya-version: $version is not X.Y.Z" >&2
  exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
data="${HIMALAYA_VERSIONS_FILE:-$root/src/engine/himalaya-versions.json}"
release_file="$(mktemp)"
trap 'rm -f "$release_file"' EXIT

if [ -n "${HIMALAYA_RELEASE_JSON:-}" ]; then
  cp "$HIMALAYA_RELEASE_JSON" "$release_file"
else
  token="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
  auth=()
  if [ -n "$token" ]; then
    auth=(-H "Authorization: Bearer $token")
  fi
  curl --proto '=https' --tlsv1.2 -fsSL --max-time 60 \
    -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2022-11-28' \
    ${auth[@]+"${auth[@]}"} \
    -o "$release_file" \
    "https://api.github.com/repos/pimalaya/himalaya/releases/tags/v$version"
fi

python3 - "$data" "$version" "$release_file" <<'PY'
import json
import re
import sys

PLATFORMS = ['aarch64-darwin', 'x86_64-linux', 'aarch64-linux']
DIGEST = re.compile(r'sha256:[0-9a-f]{64}')

data_path, version, release_path = sys.argv[1:]


def fail(message):
    print(f'add-himalaya-version: {message}', file=sys.stderr)
    sys.exit(1)


def parts(v):
    return tuple(int(p) for p in v.split('.'))


def render(entries):
    lines = []
    for e in entries:
        roles = json.dumps(e['roles'], separators=(',', ':'))
        assets = json.dumps({p: e['assets'][p] for p in PLATFORMS}, separators=(',', ':'))
        lines.append(f'  {{"version":{json.dumps(e["version"])},"roles":{roles},\n   "assets":{assets}}}')
    return '{"versions":[\n' + ',\n'.join(lines) + '\n]}\n'


with open(data_path) as f:
    entries = json.load(f)['versions']
with open(release_path) as f:
    release = json.load(f)

if any(e['version'] == version for e in entries):
    fail(f'{version} is already listed')
if parts(version) <= parts(entries[-1]['version']):
    fail(f'{version} is not higher than {entries[-1]["version"]}')
if release.get('tag_name') != f'v{version}':
    fail(f'the release is {release.get("tag_name")!r}, not v{version}')
if release.get('draft') or release.get('prerelease'):
    fail(f'v{version} is a draft or a prerelease')

assets = {}
for platform in PLATFORMS:
    name = f'himalaya.{platform}.tgz'
    found = [a for a in release.get('assets', []) if a.get('name') == name]
    if len(found) != 1:
        fail(f'the release has no single {name}')
    digest = found[0].get('digest') or ''
    if not DIGEST.fullmatch(digest):
        fail(f'the release has no sha256 digest for {name}')
    assets[platform] = digest

entries.append({'version': version, 'roles': dict(entries[-1]['roles']), 'assets': assets})
with open(data_path, 'w') as f:
    f.write(render(entries))
print(f'Added Himalaya {version}; roles copied from {entries[-2]["version"]}: {json.dumps(entries[-1]["roles"])}')
PY
