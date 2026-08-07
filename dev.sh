#!/bin/sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
cd "$script_dir"

case "$(uname -s):$(uname -m)" in
  Darwin:arm64) agent_os=macos; agent_arch=aarch64 ;;
  Darwin:x86_64) agent_os=macos; agent_arch=x86_64 ;;
  Linux:x86_64 | Linux:amd64) agent_os=linux; agent_arch=x86_64 ;;
  *)
    printf 'editur: Cursor Agent local development is unsupported on %s/%s\n' \
      "$(uname -s)" "$(uname -m)" >&2
    exit 1
    ;;
esac

manifest="target/editur-dev/cursor-agent-$agent_os-$agent_arch.json"
binary="target/editur-dev/bin/editur"
if [ ! -s "$manifest" ] || [ assets/agent/cursor-release.json -nt "$manifest" ]; then
  printf '%s\n' 'Generating the pinned Cursor Agent development manifest…'
  cargo run --locked --example build_agent_manifest -- \
    assets/agent/cursor-release.json "$agent_os" "$agent_arch" "$manifest"
fi

EDITUR_AGENT_MANIFEST="$manifest" cargo build --locked --bin editur
target/debug/editur --quit-running
sleep 0.1
mkdir -p "$(dirname -- "$binary")"
cp target/debug/editur "$binary.new"
mv "$binary.new" "$binary"
if [ "$#" -eq 0 ]; then
  set -- .
fi
exec "$binary" "$@"
