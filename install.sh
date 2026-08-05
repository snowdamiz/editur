#!/bin/sh
set -eu

release_base=https://github.com/snowdamiz/editur/releases/download/release
install_dir=${EDITUR_INSTALL_DIR:-"$HOME/.local/bin"}

case "$(uname -s):$(uname -m)" in
  Darwin:arm64) asset=editur-macos-aarch64 ;;
  Darwin:x86_64) asset=editur-macos-x86_64 ;;
  Linux:x86_64 | Linux:amd64) asset=editur-linux-x86_64 ;;
  *)
    printf 'editur: no release build for %s/%s\n' "$(uname -s)" "$(uname -m)" >&2
    exit 1
    ;;
esac

download_dir=$(mktemp -d "${TMPDIR:-/tmp}/editur-install.XXXXXX")
cleanup() {
  if [ -d "$download_dir" ] && [ ! -L "$download_dir" ]; then
    rm -r -- "$download_dir"
  fi
}
trap cleanup EXIT

curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  "$release_base/$asset" --output "$download_dir/editur"
curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  "$release_base/$asset.sha256" --output "$download_dir/editur.sha256"

expected_hash=$(tr -d '[:space:]' < "$download_dir/editur.sha256" | tr 'A-F' 'a-f')
case "$expected_hash" in
  *[!0-9a-f]* | "")
    printf 'editur: release checksum is invalid\n' >&2
    exit 1
    ;;
esac
if [ "${#expected_hash}" -ne 64 ]; then
  printf 'editur: release checksum is invalid\n' >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  actual_hash=$(sha256sum "$download_dir/editur" | awk '{print $1}')
else
  actual_hash=$(shasum -a 256 "$download_dir/editur" | awk '{print $1}')
fi
if [ "$actual_hash" != "$expected_hash" ]; then
  printf 'editur: downloaded binary failed SHA-256 verification\n' >&2
  exit 1
fi

mkdir -p "$install_dir"
install -m 0755 "$download_dir/editur" "$install_dir/editur"
printf 'Installed Editur to %s/editur\n' "$install_dir"
case ":${PATH:-}:" in
  *":$install_dir:"*) ;;
  *) printf "Add it to PATH: export PATH=\"%s:\$PATH\"\n" "$install_dir" ;;
esac
