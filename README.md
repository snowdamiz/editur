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

Both installers download from the [continuous release](https://github.com/snowdamiz/editur/releases/tag/release) and verify its SHA-256 checksum before installation. macOS installs the native `Editur.app` bundle plus a CLI symlink; Linux and Windows install the native executable. Set `EDITUR_INSTALL_DIR` to override the default install directory.

The installers also download the pinned proprietary Cursor Agent package directly from Cursor. Editur verifies the archive and every installed file, keeps it private to Editur, disables Cursor's own auto-updater, and never adds Cursor Agent to `PATH` or changes a global Cursor installation. Cursor Agent is subject to [Cursor's terms](https://cursor.com/terms-of-service).

`editur update` migrates older bare-binary macOS installs into the icon-preserving app bundle. Very old builds may need the command twice—the first update installs the migration-capable CLI—or you can run the installer once.

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

Release builds publish a `.zip` app bundle for macOS, a `.tar.gz` archive for Linux, and a `.zip` archive for Windows. Every archive includes the native platform icon. macOS release builds require a precompiled Metal shader library, while local builds fall back to runtime compilation when the optional Apple Metal toolchain is absent. Release assets carry GitHub artifact attestations.

## Use

```text
editur [PATH]
editur update
editur syntax list
editur syntax install typescript
editur syntax install ./language.editur-syntax
editur syntax remove typescript
```

`PATH` may be an existing file, a directory, or a new filename whose parent exists. While Editur is open, later `editur PATH` commands forward the target to that process and return immediately. Closing the window exits the editor completely after the normal unsaved-change check.

The editor wraps long lines and scrolls vertically without a horizontal scrollbar. It preserves LF/CRLF line endings and file permissions. If the file changes externally, saving stops and offers Reload, Save As, or Cancel. Invalid UTF-8 and binary input are rejected.

Core shortcuts follow platform conventions: search the current file (`Cmd/Ctrl+F`), search project files and contents (`Cmd/Ctrl+Shift+F`), save (`Cmd/Ctrl+S`), toggle sidebar (`Cmd/Ctrl+B`), focus tree/editor (`Cmd/Ctrl+1/2`), and close (`Cmd/Ctrl+W`). In-file matches highlight live with Enter/Shift+Enter navigation. Project results are grouped into filename and content matches; recursive indexing does not start until the first non-empty project query.

`editur update` is intentionally terminal-only. It downloads the matching build from the continuous `release`, verifies its SHA-256 checksum, asks a clean resident editor to exit, and replaces the installation after verification. An update refuses to discard unsaved work. The install directory must be writable. CI release builds embed the update URL; local source builds can opt in by setting `EDITUR_UPDATE_BASE` to an HTTPS release directory at compile time or when running the command.

## Cursor Agent sidebar

Use the sidebar icon at the right of the titlebar to toggle the independent Cursor Agent sidebar for the current project; the Files explorer remains visible on the left and the collapsed Agent sidebar consumes no workspace. Nothing agent-related is launched, inspected, or downloaded until the sidebar is first opened. The Agent sidebar supports streamed replies, plans, tool activity and supplied diffs, follow-ups in one session, advertised model/mode controls, exact permission choices, cancellation, reconnect, and in-memory transcript truncation. A dirty open file must be saved before a prompt; external edits reload a clean buffer but never overwrite a dirty one.

Official release builds embed a tested per-platform Cursor manifest. Plain local source builds intentionally omit it unless `EDITUR_AGENT_MANIFEST` points to a generated manifest, and the Agent view reports that clearly instead of resolving a mutable package at runtime.

Cursor authentication is handled by Cursor Agent. Choose its advertised login method in the sidebar to open the browser flow; Editur does not ask for, print, or store Cursor credentials. A Cursor account is required. Agent use consumes the limits or usage-based billing of that account and selected model; check [Cursor's current pricing](https://cursor.com/pricing) before use.

Prompts, relevant project code, tool results, and conversation context may be sent by Cursor Agent to Cursor and its model providers. Editur does not add its own telemetry or persist the agent transcript, but Cursor's retention and training behavior depends on the account's Privacy Mode and provider choices. Review [Cursor's data-use policy](https://cursor.com/data-use), use `.cursorignore` for files Cursor should avoid, and do not submit regulated or third-party data unless your agreements permit it.

Permission cards reduce accidental execution but are not an operating-system sandbox. Review the exact proposed action and choice; agents can make incorrect changes or run risky commands. This integration uses the beta Cursor CLI/ACP surface and currently provides one local process, one active session, and one active turn. It has no cloud agents, parallel chats, persistent history, Editur-owned allowlists, worktrees, automatic Git operations, or ACP v2 draft features.

## Continuous releases

Push the commit to the dedicated delivery branch:

```sh
git push origin HEAD:release
```

The workflow tests and builds Linux x86_64, macOS Apple Silicon and Intel, and Windows x86_64. A successful run moves the `release` tag and refreshes the continuous prerelease archives, updater binaries, checksums, build attestations, and syntax catalog. Version tags matching `v*` still publish versioned application releases.

## Syntax packages

Rust and Plain Text are the only embedded syntaxes. Additional highlighting stays out of the editor binary and is installed only when requested:

```sh
editur syntax list
editur syntax install typescript
editur syntax remove typescript
```

Bare names such as `dockerfile` always refer to catalog packages, even when the current project contains a `Dockerfile`. Prefix local archives with a path, such as `./language.editur-syntax`.

The published catalog currently includes C/C++, C#, CSS, Dockerfile, dotenv, Go, GraphQL, HTML, Java, JavaScript, JSON, Kotlin, Lua, Makefile, Markdown, PHP, Python, Ruby, Shell, SQL, Swift, TOML, TypeScript, XML, and YAML. CI builds deterministic data-only archives from `syntax-packages/` and publishes them to the `syntax-v1` release.

Build that catalog locally with:

```sh
cargo run --release --locked --example build_syntax_catalog -- dist/syntax BASE_URL
```

Set `EDITUR_SYNTAX_CATALOG` to test another HTTPS catalog. `EDITUR_GPU_DEVICE` selects a native adapter by a case-insensitive name fragment, `EDITUR_GPU_VALIDATION=1` requests available validation layers, and `EDITUR_LOG=debug` prints startup timings.

See [PERFORMANCE.md](PERFORMANCE.md) for the current release baseline, [PLAN.md](PLAN.md) for the v1 product contract, and [ACP_AGENT_PLAN.md](ACP_AGENT_PLAN.md) for the agent-sidebar implementation plan.
