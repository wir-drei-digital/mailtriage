#!/usr/bin/env python3
"""The Dovecot job's matrix: every tested Himalaya version and the SHA-256
of its x86_64-linux release archive, from src/engine/himalaya-versions.json.

Usage: himalaya_matrix.py DATA_FILE

Prints one line of JSON: [{"version": "2.1.0", "sha256": "<64 hex>"}, ...].
"""
import json
import sys


def main(path):
    with open(path) as data:
        versions = json.load(data)['versions']
    matrix = []
    for entry in versions:
        digest = entry['assets']['x86_64-linux']
        if not digest.startswith('sha256:'):
            raise SystemExit(f"{entry['version']}: no sha256 digest for x86_64-linux")
        matrix.append({'version': entry['version'], 'sha256': digest[len('sha256:'):]})
    print(json.dumps(matrix, separators=(',', ':')))


if __name__ == '__main__':
    main(sys.argv[1])
