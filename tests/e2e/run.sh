#!/usr/bin/env bash
# Runs the Dovecot end-to-end filing test (tests/e2e_dovecot.rs) in one
# namespace layout against a throwaway Dovecot 2.3 container:
#
#   MT_E2E_HIMALAYA=/path/to/himalaya bash tests/e2e/run.sh flat|prefix PORT
#
# Needs Docker, python3, cargo and a Himalaya v2.1.0 binary. The container is
# reachable on 127.0.0.1:PORT only and is always stopped when this script exits.
set -euo pipefail

fail() {
  echo "e2e: $*" >&2
  exit 1
}
usage() {
  echo "usage: MT_E2E_HIMALAYA=/path/to/himalaya $0 flat|prefix PORT" >&2
  exit 2
}

[ "$#" -eq 2 ] || usage
layout=$1
port=$2
case "$layout" in flat | prefix) ;; *) usage ;; esac
case "$port" in '' | *[!0-9]*) usage ;; esac

# Resolve the Himalaya binary before changing directory; the test runs
# mailtriage from a temporary directory, so the path must be absolute.
himalaya=${MT_E2E_HIMALAYA:-}
[ -n "$himalaya" ] || fail "set MT_E2E_HIMALAYA to a Himalaya v2.1.0 binary"
case "$himalaya" in
  */*) ;;
  *) himalaya=$(command -v "$himalaya") || fail "MT_E2E_HIMALAYA not found on PATH: $MT_E2E_HIMALAYA" ;;
esac
[ -f "$himalaya" ] && [ -x "$himalaya" ] || fail "MT_E2E_HIMALAYA is not an executable file: $himalaya"
himalaya="$(cd "$(dirname "$himalaya")" && pwd)/$(basename "$himalaya")"
version=$("$himalaya" --version) || fail "$himalaya --version failed"
version=${version%%$'\n'*}
case "$version" in
  "himalaya v2.1.0 "*) ;;
  *) fail "expected Himalaya v2.1.0, got: $version" ;;
esac

command -v python3 >/dev/null 2>&1 || fail "python3 is required"
command -v cargo >/dev/null 2>&1 || fail "cargo is required"
command -v docker >/dev/null 2>&1 || fail "the docker CLI is required for the Dovecot container"
docker info >/dev/null 2>&1 ||
  fail "the Docker daemon is not available ('docker info' failed); start Docker and retry"

cd "$(dirname "$0")/../.."
root=$PWD
config="$root/tests/e2e/dovecot-$layout.conf"
[ -f "$config" ] || fail "missing $config"
container="mt-e2e-$layout"

# Always stop and remove the container; on failure show Dovecot's log first.
# (No `docker run --rm`: the log must survive a container that exits early.)
cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if [ "$status" -ne 0 ]; then
    echo "e2e: $layout layout failed (exit $status); last Dovecot log lines:" >&2
    docker logs --tail 100 "$container" >&2 2>&1 || true
  fi
  docker stop "$container" >/dev/null 2>&1 || true
  docker rm -f "$container" >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# A container left over from an interrupted run would hold the name and port.
docker rm -f "$container" >/dev/null 2>&1 || true
docker run -d --platform linux/amd64 --name "$container" \
  -p "127.0.0.1:$port:143" \
  -v "$config:/etc/dovecot/dovecot.conf:ro" \
  dovecot/dovecot:2.3.21 >/dev/null

deadline=$((SECONDS + 30))
until python3 tests/e2e/imap_client.py --port "$port" ping >/dev/null 2>&1; do
  if [ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null)" != true ]; then
    fail "the Dovecot container stopped during startup"
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    fail "Dovecot did not accept a login on 127.0.0.1:$port within 30 s"
  fi
  sleep 1
done
echo "e2e: Dovecot ($layout layout) is up on 127.0.0.1:$port; running the test" >&2

MT_E2E_HIMALAYA="$himalaya" MT_E2E_LAYOUT="$layout" MT_E2E_PORT="$port" \
  cargo test --locked --test e2e_dovecot -- --ignored --test-threads=1
echo "e2e: $layout layout passed" >&2
