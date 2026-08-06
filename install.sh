#!/bin/sh
set -eu

release_base=https://github.com/snowdamiz/editur/releases/download/release
install_dir=${EDITUR_INSTALL_DIR:-"$HOME/.local/bin"}

case "$(uname -s):$(uname -m)" in
  Darwin:arm64) asset=editur-macos-aarch64.zip; package=app ;;
  Darwin:x86_64) asset=editur-macos-x86_64.zip; package=app ;;
  Linux:x86_64 | Linux:amd64) asset=editur-linux-x86_64; package=binary ;;
  *)
    printf 'editur: no release build for %s/%s\n' "$(uname -s)" "$(uname -m)" >&2
    exit 1
    ;;
esac

download_dir=$(mktemp -d "${TMPDIR:-/tmp}/editur-install.XXXXXX")
staged_app=
cleanup() {
  if [ -n "$staged_app" ] && [ -d "$staged_app" ] && [ ! -L "$staged_app" ]; then
    rm -r -- "$staged_app"
  fi
  if [ -d "$download_dir" ] && [ ! -L "$download_dir" ]; then
    rm -r -- "$download_dir"
  fi
}
trap cleanup EXIT

curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  "$release_base/$asset" --output "$download_dir/$asset"
curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
  "$release_base/$asset.sha256" --output "$download_dir/$asset.sha256"

expected_hash=$(tr -d '[:space:]' < "$download_dir/$asset.sha256" | tr 'A-F' 'a-f')
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
  actual_hash=$(sha256sum "$download_dir/$asset" | awk '{print $1}')
else
  actual_hash=$(shasum -a 256 "$download_dir/$asset" | awk '{print $1}')
fi
if [ "$actual_hash" != "$expected_hash" ]; then
  printf 'editur: downloaded binary failed SHA-256 verification\n' >&2
  exit 1
fi

printf '%s\n' 'Editur includes Cursor Agent as a proprietary third-party dependency.'
printf '%s\n' 'It is downloaded directly from Cursor and is subject to https://cursor.com/terms-of-service.'

mkdir -p "$install_dir"
if [ "$package" = app ]; then
  mkdir "$download_dir/package"
  /usr/bin/ditto -x -k "$download_dir/$asset" "$download_dir/package"
  app_source="$download_dir/package/Editur.app"
  app_destination="$install_dir/Editur.app"
  executable="$app_source/Contents/MacOS/editur"
  if [ ! -d "$app_source" ] || [ -L "$app_source" ] || [ ! -f "$executable" ] || [ -L "$executable" ]; then
    printf 'editur: release archive does not contain a valid Editur.app\n' >&2
    exit 1
  fi
  "$executable" --provision-agent
  staged_app="$install_dir/.Editur.app.new.$$"
  if [ -e "$staged_app" ] || [ -L "$staged_app" ]; then
    printf 'editur: staging path already exists: %s\n' "$staged_app" >&2
    exit 1
  fi
  cp -R "$app_source" "$staged_app"
  if [ -e "$app_destination" ] || [ -L "$app_destination" ]; then
    if [ ! -d "$app_destination" ] || [ -L "$app_destination" ]; then
      printf 'editur: refusing to replace non-directory %s\n' "$app_destination" >&2
      exit 1
    fi
    backup_app="$install_dir/.Editur.app.old.$$"
    if [ -e "$backup_app" ] || [ -L "$backup_app" ]; then
      printf 'editur: backup path already exists: %s\n' "$backup_app" >&2
      exit 1
    fi
    mv "$app_destination" "$backup_app"
    if ! mv "$staged_app" "$app_destination"; then
      mv "$backup_app" "$app_destination"
      exit 1
    fi
    rm -r -- "$backup_app"
  else
    mv "$staged_app" "$app_destination"
  fi
  staged_app=
  rm -f -- "$install_dir/editur"
  ln -s "$app_destination/Contents/MacOS/editur" "$install_dir/editur"
else
  chmod 0755 "$download_dir/$asset"
  "$download_dir/$asset" --provision-agent
  install -m 0755 "$download_dir/$asset" "$install_dir/editur"
fi
printf 'Installed Editur to %s/editur\n' "$install_dir"
case ":${PATH:-}:" in
  *":$install_dir:"*) ;;
  *) printf "Add it to PATH: export PATH=\"%s:\$PATH\"\n" "$install_dir" ;;
esac
