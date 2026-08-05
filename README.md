# Editur

Editur is a small native editor for quick, focused file changes. It opens one file beside a lazy, keyboard-navigable tree, searches the project from a floating palette, and saves through conflict-checked atomic replacement.

It uses the host graphics API directly: Metal on macOS, Direct3D 12 on Windows 10+, and Vulkan 1.1 on Linux. There is no `wgpu` renderer or runtime graphics fallback.

## Quick start

Install on macOS or Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/snowdamiz/editur/release/install.sh | sh
```

Install on Windows from PowerShell:

```powershell
irm https://raw.githubusercontent.com/snowdamiz/editur/release/install.ps1 | iex
```

Open a new terminal if instructed, then run:

```sh
editur .
```

Update that installed binary at any time from the terminal:

```sh
editur update
```

Both installers download the native binary from the [continuous release](https://github.com/snowdamiz/editur/releases/tag/release) and verify its SHA-256 checksum before installation. Set `EDITUR_INSTALL_DIR` to override the default install directory.

## Build and install

Install stable Rust, then run:

```sh
cargo build --release --locked
cargo install --path . --locked
```

On Ubuntu/Debian, install the native window headers and Vulkan loader first:

```sh
sudo apt-get install libwayland-dev libxkbcommon-dev libvulkan1
```

Release builds publish `.tar.gz` archives for macOS/Linux and a `.zip` archive for Windows. Each archive includes the native platform icon. Release assets carry GitHub artifact attestations.

## Use

```text
editur [PATH]
editur update
editur syntax list
editur syntax install python
editur syntax install ./language.editur-syntax
editur syntax remove python
```

`PATH` may be an existing file, a directory, or a new filename whose parent exists. The process remains attached to the terminal until the window closes.

The editor wraps long lines and scrolls vertically without a horizontal scrollbar. It preserves LF/CRLF line endings and file permissions. If the file changes externally, saving stops and offers Reload, Save As, or Cancel. Invalid UTF-8 and binary input are rejected.

Core shortcuts follow platform conventions: search the current file (`Cmd/Ctrl+F`), search project files and contents (`Cmd/Ctrl+Shift+F`), save (`Cmd/Ctrl+S`), toggle sidebar (`Cmd/Ctrl+B`), focus tree/editor (`Cmd/Ctrl+1/2`), and close (`Cmd/Ctrl+W`). In-file matches highlight live with Enter/Shift+Enter navigation. Project results are grouped into filename and content matches as the background index becomes ready.

`editur update` is intentionally terminal-only. It downloads the matching build from the continuous `release`, verifies its SHA-256 checksum, and replaces the running installation after verification. The install directory must be writable. CI release builds embed the update URL; local source builds can opt in by setting `EDITUR_UPDATE_BASE` to an HTTPS release directory at compile time or when running the command.

## Continuous releases

Push the commit to the dedicated delivery branch:

```sh
git push origin HEAD:release
```

The workflow tests and builds Linux x86_64, macOS Apple Silicon and Intel, and Windows x86_64. A successful run moves the `release` tag and refreshes the continuous prerelease archives, updater binaries, checksums, build attestations, and syntax catalog. Version tags matching `v*` still publish versioned application releases.

## Syntax packages

Rust and Plain Text are embedded. Python and Markdown are shipped as data-only sample packages in `syntax-packages/`; tagged CI builds publish their deterministic archives and catalog to the `syntax-v1` release.

Build that catalog locally with:

```sh
cargo run --release --locked --example build_syntax_catalog -- dist/syntax BASE_URL
```

Set `EDITUR_SYNTAX_CATALOG` to test another HTTPS catalog. `EDITUR_GPU_DEVICE` selects a native adapter by a case-insensitive name fragment, `EDITUR_GPU_VALIDATION=1` requests available validation layers, and `EDITUR_LOG=debug` prints startup timings.

See [PERFORMANCE.md](PERFORMANCE.md) for the current release baseline and [PLAN.md](PLAN.md) for the v1 product contract.
