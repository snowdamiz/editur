# Editur

Editur is a small native editor for quick, focused file changes. It opens one file beside a lazy, keyboard-navigable tree, searches the project from a floating palette, and saves through conflict-checked atomic replacement.

It uses the host graphics API directly: Metal on macOS, Direct3D 12 on Windows 10+, and Vulkan 1.1 on Linux. There is no `wgpu` renderer or runtime graphics fallback.

## Quick start

Install on macOS or Linux:

```sh
curl --proto '=https' --tlsv1.2 --retry 5 --retry-all-errors -LsSf https://raw.githubusercontent.com/snowdamiz/editur/release/install.sh | sh
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
```

`PATH` may be an existing file, a directory, or a new filename whose parent exists. While Editur is open, later `editur PATH` commands forward the target to that process and return immediately. Closing the window exits the editor completely after the normal unsaved-change check.

The editor wraps long lines and scrolls vertically without a horizontal scrollbar. It preserves LF/CRLF line endings and file permissions. If the file changes externally, saving stops and offers Reload, Save As, or Cancel. Invalid UTF-8 and binary input are rejected.

Core shortcuts follow VS Code platform conventions: search the current file (`Cmd/Ctrl+F`), search project files and contents (`Cmd/Ctrl+Shift+F`), save (`Cmd/Ctrl+S`), toggle the sidebar (`Cmd/Ctrl+B`), focus editor panes (`Cmd/Ctrl+1` through `9`), and close the active editor (`Cmd+W` on macOS, `Ctrl+F4` on Windows, or `Ctrl+W` on Linux). In-file matches highlight live with Enter/Shift+Enter navigation. Project results are grouped into filename and content matches; recursive indexing does not start until the first non-empty project query.

## Keyboard profiles

Open Settings with the titlebar gear or `Cmd/Ctrl+,`; use `Cmd/Ctrl+K Cmd/Ctrl+S` to open Keybindings directly. The immutable VS Code and Vim profiles are always available. **Customize** derives an editable profile without copying the preset, while **New profile** can start from VS Code, Vim, or an empty Standard/Vim behavior. The command list searches labels, IDs, keys, and scopes and can add, change, remove, disable, replace conflicts, or reset bindings. The recorder accepts one-to-four-stroke logical chords and can opt a stroke into physical-key-position matching. Profile changes are saved atomically and do not change buffers or undo history.

The VS Code profile includes only actions Editur implements; command palettes, multi-cursor editing, folding, refactoring, editor history, extension commands, and arbitrary `when` expressions are intentionally absent. Built-in terminal control keys fall through to the PTY; an explicit custom terminal/global rule can override one. Standard text fields retain printable and IME input, and `Ctrl+Alt` printable bindings require an explicit physical key so AltGr remains usable.

The Vim profile is practical modal editing, not a Vim runtime. It supports Normal, Insert, Replace, Visual character/line, and Operator-pending modes; counts; basic, word, paragraph, file, and character-find motions; `d/c/y/>/<`; common text objects; `x/X/D/C/r/J/~/p/P`; grouped undo/redo and `.`, unnamed and `"+` registers, find reuse, `Ctrl+W` pane commands, and the safe Ex subset `:w`, `:q`, `:q!`, `:wq`, `:x`, `:e {path}`, `:noh`, and `:{line}`. Visual block, named registers, marks, macros, substitutions, shell commands, `.vimrc`, plugins, and recursive mappings are not supported.

<details>
<summary>Stable command IDs</summary>

These IDs are persisted and are also searchable in Settings:

```text
Application
application.closeWindow
application.openKeyboardShortcuts
application.openSettings
application.toggleAgentSidebar
application.toggleAgenticView
application.toggleDevinSidebar

Files and workbench
file.closeActiveEditor
file.focusLeftPane
file.focusNextPane
file.focusPane1 … file.focusPane9
file.focusPreviousPane
file.focusRightPane
file.save
file.saveAndClose
file.splitEditor
workbench.focusExplorer
workbench.toggleMarkdownPreview
workbench.toggleSidebar
workbench.toggleTerminal

Search and tree
search.close
search.findInFile
search.findNext
search.findPrevious
search.searchProject
tree.collapse
tree.expand
tree.moveDown
tree.moveUp
tree.open

Editor
editor.copy
editor.cut
editor.paste
editor.undo
editor.redo
editor.selectAll
editor.deleteLeft
editor.deleteRight
editor.insertLineBreak
editor.indent
editor.outdent
editor.cursorLeft
editor.cursorRight
editor.cursorUp
editor.cursorDown
editor.cursorWordLeft
editor.cursorWordRight
editor.cursorLineStart
editor.cursorLineEnd
editor.cursorPageUp
editor.cursorPageDown
editor.cursorDocumentStart
editor.cursorDocumentEnd
editor.selectLeft
editor.selectRight
editor.selectUp
editor.selectDown
editor.selectWordLeft
editor.selectWordRight
editor.selectLineStart
editor.selectLineEnd
editor.selectPageUp
editor.selectPageDown
editor.selectDocumentStart
editor.selectDocumentEnd
editor.triggerSuggest
editor.goToDefinition
editor.nextDiagnostic
editor.previousDiagnostic

Vim modes and changes
vim.mode.normal
vim.mode.insert
vim.mode.insertLineStart
vim.mode.append
vim.mode.appendLineEnd
vim.mode.openAbove
vim.mode.openBelow
vim.mode.replace
vim.mode.visualCharacter
vim.mode.visualLine
vim.change.deleteCharacter
vim.change.deleteCharacterLeft
vim.change.deleteToLineEnd
vim.change.joinLines
vim.change.putAfter
vim.change.putBefore
vim.change.replaceCharacter
vim.change.substituteCharacter
vim.change.substituteLine
vim.change.toLineEnd
vim.change.toggleCase

Vim motions, operators, and text objects
vim.motion.left
vim.motion.right
vim.motion.up
vim.motion.down
vim.motion.lineStart
vim.motion.firstNonBlank
vim.motion.lineEnd
vim.motion.fileStart
vim.motion.fileEnd
vim.motion.wordForward
vim.motion.bigWordForward
vim.motion.wordEnd
vim.motion.bigWordEnd
vim.motion.wordBack
vim.motion.bigWordBack
vim.motion.paragraphBack
vim.motion.paragraphForward
vim.motion.findForward
vim.motion.findBackward
vim.motion.tillForward
vim.motion.tillBackward
vim.motion.repeatFind
vim.motion.reverseFind
vim.operator.delete
vim.operator.change
vim.operator.yank
vim.operator.indent
vim.operator.outdent
vim.textObject.inner
vim.textObject.around

Vim history, search, register, Ex, and counts
vim.history.undo
vim.history.redo
vim.history.repeat
vim.search.forward
vim.search.backward
vim.search.next
vim.search.previous
vim.search.wordForward
vim.search.wordBackward
vim.register.system
vim.ex.open
vim.count.0 … vim.count.9
```

</details>

`editur update` is intentionally terminal-only. It downloads the matching build from the continuous `release`, verifies its SHA-256 checksum, asks a clean resident editor to exit, and replaces the installation after verification. An update refuses to discard unsaved work. The install directory must be writable. CI release builds embed the update URL; local source builds can opt in by setting `EDITUR_UPDATE_BASE` to an HTTPS release directory at compile time or when running the command.

## ACP Agent sidebar

Use the sidebar icon at the right of the titlebar to open an ACP coding agent for the current project; the Files explorer remains visible on the left and the collapsed Agent sidebar consumes no workspace. Nothing provider-related is inspected, provisioned, or launched until Agent is first opened. Release builds offer Cursor by default and Codex from the provider selector. Provider switches stop the old process before starting the replacement, keep the unsent composer draft, and isolate sessions and managed files by provider.

The shared Agent UI supports streamed replies, plans, tool activity and supplied diffs, follow-ups, advertised model/mode controls, ACP image, audio, and resource attachments, exact permission choices, cancellation, reconnect, and bounded in-memory transcripts. History and provider-specific permission controls appear only when the connected agent advertises them. A dirty open file must be saved before a prompt; external edits reload a clean buffer but never overwrite a dirty one.

The full Agent view organizes recent project roots as workspaces, including Git worktrees opened as folders. Workspace buttons show only the project name. Selecting a workspace switches the agent working directory; ACP history is requested and filtered by that exact root, so each workspace shows only its own sessions.

Official release builds embed one attested provider bundle. Cursor `2026.07.23-e383d2b` is provisioned during installation. Codex uses the canonical `@agentclientprotocol/codex-acp` `1.1.14` adapter, its locked `@openai/codex` `0.147.0` dependency, and a private Node.js `22.22.0` runtime; it is downloaded lazily only after its first-use license and provider-terms notice is accepted. Editur never invokes `npx`, a global Node installation, or a mutable package tag. Claude remains an unavailable catalog entry until its canonical distribution and licensing can meet the same pinned private-package policy.

To run the Cursor-only local development flow, use `./dev.sh .`; it generates and caches the current platform manifest under `target/`. A plain `cargo run` intentionally omits provider metadata.

Authentication is owned by the selected agent. Editur renders agent-launched browser login separately from terminal or environment setup. Cursor users may optionally save a per-account Cursor Cloud API key during login or in **Settings → Providers**; Editur stores it in private application data and uses it only to stream Cloud Agent progress. Provider use consumes that account's limits or usage-based billing; review [Cursor pricing](https://cursor.com/pricing) or [OpenAI API pricing](https://openai.com/api/pricing/) before use.

To start a Cursor Cloud Agent, connect the repository in the [Cursor Dashboard](https://cursor.com/dashboard), choose **Cloud** from the **Local / Cloud** dropdown beneath the Cursor composer, and send a normal prompt. Editur hands the prompt to Cursor's own CLI using the selected account's existing sign-in; no API key or environment file is required to start the run. The current Git branch must be clean, committed, and pushed to its upstream. Editur always returns the Cloud Agent URL and, when that account has an optional API key saved, mirrors [Cursor's run stream](https://cursor.com/docs/cloud-agent/api/endpoints#stream-a-run) in the transcript. Local attachments are not supported for cloud prompts.

Prompts, relevant project code, tool results, and conversation context may be sent to the selected provider and its model providers. Editur does not add telemetry or persist the transcript. Review [Cursor's data-use policy](https://cursor.com/data-use) or [OpenAI's data controls](https://platform.openai.com/docs/guides/your-data), use provider ignore controls where available, and do not submit regulated or third-party data unless your agreements permit it.

Permission cards reduce accidental execution but are not an operating-system sandbox. Review the exact proposed action and choice; agents can make incorrect changes or run risky commands. Editur retains one selected local provider process and one active turn. Cursor's public streaming API requires its separate optional API key. The ACP surface has no parallel chats, persisted transcripts, Editur-owned allowlists, automatic worktree creation, automatic Git operations, or ACP v2 draft features.

## Devin cloud sidebar

Use the bolt icon at the right of the titlebar, or bind `application.toggleDevinSidebar`, to supervise cloud Devin separately from the local ACP Agent. The two assistant sidebars share one right-hand slot but retain independent state and drafts. Sessions support server filters, advanced and batch creation, messages and attachments, event search/detail, tags, insights, lineage, pull requests, and lifecycle controls. The sidebar section switcher also exposes repository docs and indexing, knowledge, playbooks, schedules and event automations, integrations, Devin Review, blueprints/builds, and write-only organization secrets. Every resource is permission-gated independently.

Connect with a Devin personal access token or service-user key beginning with `cog_`. User-entered credentials are stored only in the operating-system credential store. Developer builds can instead inherit `DEVIN_API_KEY` and optional `DEVIN_ORG_ID`; environment values take precedence and are never copied into Editur settings. Editur discovers the organization when the credential identifies one, and asks you to choose when a PAT can access several. It uses Devin's documented MCP endpoint plus organization-scoped v3/v3beta1 endpoints; a `403` disables only the affected feature. See [Devin authentication](https://docs.devin.ai/api-reference/authentication), [Devin MCP](https://docs.devin.ai/work-with-devin/devin-mcp), and the [v3 permission model](https://docs.devin.ai/api-reference/v3/overview).

Devin works from its remote clone: it cannot see unsaved buffers, uncommitted changes, or commits that have not been pushed. Editur never uploads a local diff, turns remote paths into local file links, checks out a Devin pull request, or adds remote activity to local changed-file state. Polling runs only while the Devin sidebar is visible, resumes immediately when reopened, and stopping the polling does not stop the remote session.

Sleep and archive are reversible; sending to a sleeping session wakes it, and archived sessions can be unarchived from the Archived scope. Termination permanently stops remote work and always requires the danger confirmation dialog. Disconnect removes only the stored local token after confirmation and does not alter remote sessions. Authentication, transport, rate-limit, offline, and lifecycle errors shown by the UI are sanitized; raw response bodies, authorization headers, and credential-bearing URL parameters are never written to preferences, diagnostics, or logs.

## Language servers

Editur uses language servers already installed by the user; it never downloads a server or makes an LSP network request. Open the in-window Settings page with the titlebar gear or `Cmd/Ctrl+,`, then leave a preset on Auto, choose a custom executable plus one argument per line, or turn it Off. The initial presets are rust-analyzer for Rust, TypeScript Language Server for TypeScript/JavaScript, Pyright for Python, gopls for Go, and clangd for C/C++.

Supported language servers provide diagnostics, plain-text completion, and go to definition. Use `Ctrl+Space` for completion, `F8`/`Shift+F8` for next/previous diagnostics, and `F12` or command-click for definitions. Files above 5 MiB and unsupported file types do not start a server. Editur does not apply server workspace edits, commands, snippets, formatting, rename, or code actions.

Servers run lazily per project and preset, receive only open document text, and stop when their last document closes. `Not found` means the executable is absent from Editur's inherited `PATH`; install it through the language's normal tooling or select an absolute custom path. `Rescan` repeats discovery but installs nothing.

## Continuous releases

Push the commit to the dedicated delivery branch:

```sh
git push origin HEAD:release
```

The workflow tests and builds Linux x86_64, macOS Apple Silicon and Intel, and Windows x86_64. It builds the pinned Codex adapter from its locked upstream commit, packages the exact private runtime deterministically, verifies its version probe, and attests both package and provider bundle. A successful run moves the `release` tag and refreshes the continuous prerelease archives, updater binaries, checksums, provider packages, and build attestations. Version tags matching `v*` still publish versioned application releases.

## Syntax highlighting

Syntax highlighting is fully built in and selected automatically from the file name or extension; unknown formats fall back to Plain Text. Editur embeds Syntect's full default syntax set plus C/C++, C#, CSS/SCSS/Less, Dockerfile, dotenv, Go, GraphQL, HTML and HTML-like Astro/Vue/Svelte files, Java, JavaScript/JSX, JSON/JSONC, Kotlin, Lua, Makefile, Markdown/MDX, PHP, PowerShell, Python, Ruby, Shell, SQL, Swift, TOML, TypeScript/TSX, XML, and YAML grammars. No syntax package download, configuration, or separate CLI command is required.

`EDITUR_GPU_DEVICE` selects a native adapter by a case-insensitive name fragment, `EDITUR_GPU_VALIDATION=1` requests available validation layers, and `EDITUR_LOG=debug` prints startup timings.

See [PERFORMANCE.md](PERFORMANCE.md) for the current release baseline, [PLAN.md](PLAN.md) for the v1 product contract, [AURA_INTERNAL_EDITOR_PLAN.md](AURA_INTERNAL_EDITOR_PLAN.md) for the company workbench architecture and roadmap, [ACP_AGENT_PLAN.md](ACP_AGENT_PLAN.md) for the agent-sidebar implementation plan, [DEVIN_SIDEBAR_PLAN.md](DEVIN_SIDEBAR_PLAN.md) for the dedicated Devin cloud sidebar, [LSP_PLAN.md](LSP_PLAN.md) for the language-server contract and Settings UI, and [KEYBINDINGS_PLAN.md](KEYBINDINGS_PLAN.md) for configurable VS Code, Vim, and custom keyboard profiles.
