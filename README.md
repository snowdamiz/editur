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

Both installers download from the [continuous release](https://github.com/snowdamiz/editur/releases/tag/release) and verify its SHA-256 checksum before installation. macOS installs the native `Editur.app` bundle to `~/Applications` plus a CLI symlink; Windows installs the icon-bearing native executable and adds Editur to the Start menu; Linux installs the native executable. Set `EDITUR_INSTALL_DIR` to override the CLI directory, `EDITUR_APP_DIR` to override the macOS application directory, or `EDITUR_START_MENU_DIR` to override the Windows shortcut directory.

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

Release builds publish a launchable `.app` bundle for macOS, a native executable archive for Linux, and an icon-bearing `.exe` archive for Windows. CI validates the application structure before uploading each artifact. On macOS, `./dev.sh .` also launches from `target/editur-dev/Editur.app`, so local development uses the real Dock icon without any runtime icon decoding or LaunchServices delay. macOS release builds require a precompiled Metal shader library, while local builds fall back to runtime compilation when the optional Apple Metal toolchain is absent. Release assets carry GitHub artifact attestations.

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

## ACP Agent sidebar

Use the sidebar icon at the right of the titlebar to open an ACP coding agent for the current project; the Files explorer remains visible on the left and the collapsed Agent sidebar consumes no workspace. Nothing provider-related is inspected, provisioned, or launched until Agent is first opened. Release builds offer Cursor by default and Codex from the provider selector. Provider switches stop the old process before starting the replacement, keep the unsent composer draft, and isolate sessions and managed files by provider.

The shared Agent UI supports streamed replies, plans, tool activity and supplied diffs, follow-ups, advertised model/mode controls, ACP image, audio, and resource attachments, exact permission choices, cancellation, reconnect, and bounded in-memory transcripts. History and provider-specific permission controls appear only when the connected agent advertises them. A dirty open file must be saved before a prompt; external edits reload a clean buffer but never overwrite a dirty one.

Official release builds embed one attested provider bundle. Cursor `2026.07.23-e383d2b` is provisioned during installation. Codex uses the canonical `@agentclientprotocol/codex-acp` `1.1.14` adapter, its locked `@openai/codex` `0.147.0` dependency, and a private Node.js `22.22.0` runtime; it is downloaded lazily only after its first-use license and provider-terms notice is accepted. Editur never invokes `npx`, a global Node installation, or a mutable package tag. Claude remains an unavailable catalog entry until its canonical distribution and licensing can meet the same pinned private-package policy.

To run the Cursor-only local development flow, use `./dev.sh .`; it generates and caches the current platform manifest under `target/`. A plain `cargo run` intentionally omits provider metadata.

Authentication is owned by the selected agent. Editur renders agent-launched login methods separately from terminal or environment setup methods and does not ask for, print, or store provider credentials. Provider use consumes that account's limits or usage-based billing; review [Cursor pricing](https://cursor.com/pricing) or [OpenAI API pricing](https://openai.com/api/pricing/) before use.

Prompts, relevant project code, tool results, and conversation context may be sent to the selected provider and its model providers. Editur does not add telemetry or persist the transcript. Review [Cursor's data-use policy](https://cursor.com/data-use) or [OpenAI's data controls](https://platform.openai.com/docs/guides/your-data), use provider ignore controls where available, and do not submit regulated or third-party data unless your agreements permit it.

Permission cards reduce accidental execution but are not an operating-system sandbox. Review the exact proposed action and choice; agents can make incorrect changes or run risky commands. Editur retains one selected local provider process and one active turn. It has no cloud agents, parallel chats, persisted transcripts, Editur-owned allowlists, worktrees, automatic Git operations, or ACP v2 draft features.

## Continuous releases

Push the commit to the dedicated delivery branch:

```sh
git push origin HEAD:release
```

The workflow tests and builds Linux x86_64, macOS Apple Silicon and Intel, and Windows x86_64. It builds the pinned Codex adapter from its locked upstream commit, packages the exact private runtime deterministically, verifies its version probe, and attests both package and provider bundle. A successful run moves the `release` tag and refreshes the continuous prerelease archives, updater binaries, checksums, provider packages, build attestations, and syntax catalog. Version tags matching `v*` still publish versioned application releases.

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
