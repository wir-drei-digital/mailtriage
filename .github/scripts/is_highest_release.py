#!/usr/bin/env python3
"""Whether a stable release tag may be marked Latest on GitHub.

Usage: is_highest_release.py TAG RELEASES_JSON

RELEASES_JSON is the output of
`gh release list --exclude-drafts --exclude-pre-releases --json tagName`.
Exits 0 when TAG is an exact vX.Y.Z tag at least as high as every
published stable release there, else 1. Other tags in the list are ignored,
as the updater ignores them.
"""
import json
import re
import sys

STABLE = re.compile(r'v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)')


def version(tag):
    match = STABLE.fullmatch(tag)
    return tuple(int(part) for part in match.groups()) if match else None


def main(tag, path):
    mine = version(tag)
    if mine is None:
        return 1
    with open(path) as releases:
        published = [version(r['tagName']) for r in json.load(releases)]
    return 0 if all(mine >= other for other in published if other) else 1


if __name__ == '__main__':
    sys.exit(main(sys.argv[1], sys.argv[2]))
