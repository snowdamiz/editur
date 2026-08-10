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

cursor_manifest="target/editur-dev/cursor-agent-$agent_os-$agent_arch.json"
codex_manifest="target/editur-dev/codex-agent-$agent_os-$agent_arch.json"
claude_manifest="target/editur-dev/claude-agent-$agent_os-$agent_arch.json"
provider_bundle="target/editur-dev/agent-bundle-$agent_os-$agent_arch.json"
codex_commit=5faefec5d55ded33c54b68ffec93def4f6c547f5
codex_source="target/editur-dev/codex-acp-$codex_commit"
codex_stage="target/editur-dev/codex-stage-$agent_os-$agent_arch"
codex_archive="$script_dir/target/editur-dev/editur-provider-codex-$agent_os-$agent_arch.zip"
claude_commit=6b405138fc82be947964612fac04e56654827b66
claude_source="target/editur-dev/claude-agent-acp-$claude_commit"
claude_stage="target/editur-dev/claude-stage-$agent_os-$agent_arch"
claude_archive="$script_dir/target/editur-dev/editur-provider-claude-$agent_os-$agent_arch.zip"
binary="target/editur-dev/bin/editur"
if [ ! -s "$cursor_manifest" ] || [ assets/agent/cursor-release.json -nt "$cursor_manifest" ]; then
  printf '%s\n' 'Generating the pinned Cursor Agent development manifest…'
  cargo run --locked --example build_agent_manifest -- \
    assets/agent/cursor-release.json "$agent_os" "$agent_arch" "$cursor_manifest"
fi

case "$agent_os:$agent_arch" in
  macos:aarch64)
    node_platform=darwin-arm64
    node_sha=5ed4db0fcf1eaf84d91ad12462631d73bf4576c1377e192d222e48026a902640
    ;;
  macos:x86_64)
    node_platform=darwin-x64
    node_sha=5ea50c9d6dea3dfa3abb66b2656f7a4e1c8cef23432b558d45fb538c7b5dedce
    ;;
  linux:x86_64)
    node_platform=linux-x64
    node_sha=c33c39ed9c80deddde77c960d00119918b9e352426fd604ba41638d6526a4744
    ;;
esac
node_archive="target/editur-dev/node-v22.22.0-$node_platform.tar.gz"
node_root="target/editur-dev/node-v22.22.0-$node_platform"
mkdir -p target/editur-dev
if [ ! -s "$node_archive" ]; then
  printf '%s\n' 'Downloading the pinned private Node.js runtime…'
  curl --fail --silent --show-error --location \
    "https://nodejs.org/dist/v22.22.0/node-v22.22.0-$node_platform.tar.gz" \
    --output "$node_archive.new"
  mv "$node_archive.new" "$node_archive"
fi
if [ "$agent_os" = macos ]; then
  actual_node_sha=$(shasum -a 256 "$node_archive" | awk '{print $1}')
else
  actual_node_sha=$(sha256sum "$node_archive" | awk '{print $1}')
fi
if [ "$actual_node_sha" != "$node_sha" ]; then
  printf '%s\n' 'editur: cached Node.js runtime failed checksum verification' >&2
  exit 1
fi
if [ ! -x "$node_root/bin/node" ]; then
  tar -xzf "$node_archive" -C target/editur-dev
fi

if [ ! -s "$codex_archive" ] \
  || [ assets/agent/codex-release.json -nt "$codex_archive" ] \
  || [ examples/package_provider_runtime.rs -nt "$codex_archive" ]; then
  if [ ! -d "$codex_source/.git" ]; then
    printf '%s\n' 'Fetching the pinned Codex ACP adapter…'
    git init -q "$codex_source"
    git -C "$codex_source" remote add origin https://github.com/agentclientprotocol/codex-acp.git
    git -C "$codex_source" fetch --depth 1 origin "$codex_commit"
    git -C "$codex_source" checkout -q --detach FETCH_HEAD
  fi
  if [ "$(git -C "$codex_source" rev-parse HEAD)" != "$codex_commit" ]; then
    printf '%s\n' 'editur: cached Codex ACP source does not match the release pin' >&2
    exit 1
  fi
  printf '%s\n' 'Building the pinned Codex ACP development package…'
  PATH="$script_dir/$node_root/bin:$PATH" npm --prefix "$codex_source" ci --ignore-scripts
  PATH="$script_dir/$node_root/bin:$PATH" npm --prefix "$codex_source" run build
  PATH="$script_dir/$node_root/bin:$PATH" npm --prefix "$codex_source" prune --omit=dev --ignore-scripts
  PATH="$script_dir/$node_root/bin:$PATH" cargo run --locked --example verify_codex_source -- "$codex_source"
  mkdir -p "$codex_stage/package/dist" "$codex_stage/package/node_modules" "$codex_stage/runtime/bin"
  cp "$codex_source/dist/index.js" "$codex_stage/package/dist/index.js"
  cp "$codex_source/package.json" "$codex_source/README.md" "$codex_source/LICENSE" "$codex_stage/package/"
  cp -R "$codex_source/node_modules/@openai" "$codex_stage/package/node_modules/"
  cp "$node_root/bin/node" "$codex_stage/runtime/bin/node"
  cp "$node_root/LICENSE" "$codex_stage/runtime/LICENSE"
  cargo run --locked --example package_provider_runtime -- "$codex_stage" "$codex_archive.new"
  mv "$codex_archive.new" "$codex_archive"
fi

if [ ! -s "$codex_manifest" ] \
  || [ assets/agent/codex-release.json -nt "$codex_manifest" ] \
  || [ "$codex_archive" -nt "$codex_manifest" ]; then
  cargo run --locked --example build_agent_manifest -- \
    assets/agent/codex-release.json "$agent_os" "$agent_arch" "$codex_manifest" "$codex_archive"
fi

if [ ! -s "$claude_archive" ] \
  || [ assets/agent/claude-release.json -nt "$claude_archive" ] \
  || [ examples/package_provider_runtime.rs -nt "$claude_archive" ] \
  || [ examples/verify_claude_source.rs -nt "$claude_archive" ]; then
  if [ ! -d "$claude_source/.git" ]; then
    printf '%s\n' 'Fetching the pinned Claude ACP adapter…'
    git init -q "$claude_source"
    git -C "$claude_source" remote add origin https://github.com/agentclientprotocol/claude-agent-acp.git
    git -C "$claude_source" fetch --depth 1 origin "$claude_commit"
    git -C "$claude_source" checkout -q --detach FETCH_HEAD
  fi
  if [ "$(git -C "$claude_source" rev-parse HEAD)" != "$claude_commit" ]; then
    printf '%s\n' 'editur: cached Claude ACP source does not match the release pin' >&2
    exit 1
  fi
  printf '%s\n' 'Building the pinned Claude ACP development package…'
  PATH="$script_dir/$node_root/bin:$PATH" npm --prefix "$claude_source" ci --ignore-scripts
  PATH="$script_dir/$node_root/bin:$PATH" npm --prefix "$claude_source" run build
  PATH="$script_dir/$node_root/bin:$PATH" npm --prefix "$claude_source" prune --omit=dev --ignore-scripts
  PATH="$script_dir/$node_root/bin:$PATH" cargo run --locked --example verify_claude_source -- "$claude_source"
  mkdir -p "$claude_stage/package/node_modules" "$claude_stage/runtime/bin"
  cp -R "$claude_source/dist" "$claude_stage/package/"
  cp "$claude_source/package.json" "$claude_source/README.md" "$claude_source/LICENSE" "$claude_stage/package/"
  cp -R "$claude_source/node_modules/"* "$claude_stage/package/node_modules/"
  cp "$node_root/bin/node" "$claude_stage/runtime/bin/node"
  cp "$node_root/LICENSE" "$claude_stage/runtime/LICENSE"
  cargo run --locked --example package_provider_runtime -- "$claude_stage" "$claude_archive.new"
  mv "$claude_archive.new" "$claude_archive"
fi

if [ ! -s "$claude_manifest" ] \
  || [ assets/agent/claude-release.json -nt "$claude_manifest" ] \
  || [ "$claude_archive" -nt "$claude_manifest" ]; then
  cargo run --locked --example build_agent_manifest -- \
    assets/agent/claude-release.json "$agent_os" "$agent_arch" "$claude_manifest" "$claude_archive"
fi
cargo run --locked --example build_provider_bundle -- \
  "$provider_bundle" "$cursor_manifest" "$codex_manifest" "$claude_manifest"

EDITUR_PROVIDER_BUNDLE="$provider_bundle" cargo build --locked --bin editur
target/debug/editur --quit-running
sleep 0.1
if [ "$agent_os" = macos ]; then
  app="target/editur-dev/Editur.app"
  binary="$app/Contents/MacOS/editur"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp assets/macos/Info.plist "$app/Contents/Info.plist"
  cp assets/icons/editur.icns "$app/Contents/Resources/Editur.icns"
fi
mkdir -p "$(dirname -- "$binary")"
cp target/debug/editur "$binary.new"
mv "$binary.new" "$binary"
if [ "$#" -eq 0 ]; then
  set -- .
fi
EDITUR_CODEX_ARCHIVE="$codex_archive" EDITUR_CLAUDE_ARCHIVE="$claude_archive" exec "$binary" "$@"
