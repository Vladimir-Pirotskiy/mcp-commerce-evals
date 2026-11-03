#!/bin/sh
# Smoke test of a release tarball: unpack it in an empty temporary directory, start the bundled
# store on a free port, run the bundled evals binary from another empty directory against it and
# require exit 0. Nothing from the source tree is reachable.
# Usage: smoke-release.sh <tarball>
set -eu

tarball=$(cd "$(dirname "${1:?usage: smoke-release.sh <tarball>}")" && pwd)/$(basename "$1")
work=$(mktemp -d)
store_pid=
cleanup() {
  [ -z "$store_pid" ] || kill "$store_pid" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

mkdir "$work/unpack" "$work/run"
tar xzf "$tarball" -C "$work/unpack"
bundle=$(find "$work/unpack" -mindepth 1 -maxdepth 1 -type d | head -n 1)

# Port 0 lets the OS pick a free port; the store prints the address it got.
"$bundle/reference-store" --bind 127.0.0.1:0 >"$work/store.log" 2>&1 &
store_pid=$!
url=
for _ in $(seq 1 100); do
  url=$(sed -n 's/.*MCP endpoint \(http[^ ]*\).*/\1/p' "$work/store.log" | head -n 1)
  [ -z "$url" ] || break
  kill -0 "$store_pid" 2>/dev/null || { cat "$work/store.log" >&2; exit 1; }
  sleep 0.1
done
[ -n "$url" ] || { echo "smoke: the store never printed its address" >&2; exit 1; }

cd "$work/run"
"$bundle/mcp-commerce-evals" --url "$url" --out report.json --min-hit5 0.92 --min-mrr 0.91
echo "smoke: ok ($url)"
