# Editur implementation plan

## 1. Purpose

Editur is a small native code editor launched from a terminal for quick, focused file changes:

```text
editur src/main.rs
```

It opens one lightweight desktop window with a navigable file tree and one editor buffer. The first release is written in Rust, ships Rust syntax highlighting only, and lets users install other language grammars with a command instead of editing configuration files.

### Audience and outcome

This plan is for the engineer implementing Editur. After reading it, they should be able to build and release the first useful version without inventing product scope or architecture along the way.

### Assumption

“Opened from the terminal” means the `editur` command launches a native desktop window and remains attached until that window closes. It does not mean the editor itself is a terminal UI. If a terminal-only UI is the actual goal, make that decision before implementation because it changes the UI framework.

## 2. Product contract

The first release must:

- Start from a terminal with zero project configuration.
- Open an existing UTF-8 file, a directory, or a new file path.
- Show a collapsible, keyboard-navigable file tree on the left.
- Show one editable buffer on the right.
- Search project filenames and UTF-8 file contents from a floating `Cmd/Ctrl+F` palette, updating results while the user types.
- Support selection, copy/paste, undo/redo, wrapped lines, vertical scrolling, and standard text input without horizontal scrollbars.
- Highlight Rust by default and use plain text for unknown file types.
- Install another language with `editur syntax install <language>`.
- Update a release installation from the terminal with `editur update`.
- Save safely without silently overwriting a file changed by another process.
- Warn before discarding unsaved work.
- Run on macOS, Linux, and Windows 10+ from one Rust codebase.
- Use the Editur logo for the window/application icon on every supported OS.

The first release will not include:

- Tabs, split panes, sessions, or project workspaces.
- LSP, completion, diagnostics, formatting, Git integration, or an embedded terminal.
- A general command palette, settings screen, themes marketplace, or account system.
- Arbitrary native, WASM, or script plugins.
- Non-UTF-8 editing or huge-file optimization.

Those features belong only after the quick single-file workflow is proven.

## 3. Terminal experience

### Commands

```text
editur [PATH]
editur syntax list
editur syntax install <LANGUAGE>
editur syntax remove <LANGUAGE>
editur update
editur --help
editur --version
```

Use `std::env::args_os` for this small grammar. Do not add a CLI framework unless the command surface becomes materially more complex.

### Path behavior

| Input | Behavior |
| --- | --- |
| No path | Use the current directory as the tree root; show no buffer until a file is selected. |
| Existing file inside the current directory | Use the current directory as root and open the file. |
| Existing file outside the current directory | Use the file’s parent as root and open the file. |
| Existing directory | Use it as root and wait for file selection. |
| Missing path with an existing parent | Open an empty dirty buffer and create the file on save. |
| Invalid or unreadable path | Print a concise terminal error and return a nonzero exit code. |

The process stays in the foreground. This makes shell scripts and callers naturally wait for the edit to finish without a separate `--wait` protocol.

## 4. Window and interaction design

Use one window with three regions:

1. A resizable and collapsible sidebar.
2. A code editor that takes the remaining space.
3. A one-line status bar showing the path, dirty state, syntax, and cursor line/column.

On launch, focus the editor when a file was supplied. Otherwise focus the tree.

Minimum shortcuts:

| Action | macOS | Linux/Windows |
| --- | --- | --- |
| Save | `Cmd+S` | `Ctrl+S` |
| Search files and contents | `Cmd+F` | `Ctrl+F` |
| Undo/redo | Platform convention | Platform convention |
| Toggle sidebar | `Cmd+B` | `Ctrl+B` |
| Focus tree | `Cmd+1` | `Ctrl+1` |
| Focus editor | `Cmd+2` | `Ctrl+2` |
| Close | `Cmd+W` | `Ctrl+W` |

When the current buffer is dirty, selecting another file or closing the window presents Save, Discard, and Cancel. Errors remain visible in the window and are also written to stderr; a failed save never clears the dirty flag.

The tree reads a directory only when the user expands it, sorts directories before files, shows dotfiles, hides `.git`, and does not follow directory symlinks. A separate background search index recursively reads the project without blocking the editable window, skips build/vendor metadata directories, ignores binary and invalid UTF-8 content, and never follows symlinks.

## 5. Technical approach

### Chosen stack

| Need | Choice | Reason |
| --- | --- | --- |
| Native window and input | `winit`, `egui-winit`, and text-only `arboard` | One cross-platform event loop with IME, keyboard, pointer, DPI, accessibility, and a small explicit system-clipboard bridge. |
| Widgets | `egui` | Built-in multiline text editing, panels, scrolling, selection, undo/redo, and accessibility semantics without coupling the app to a renderer framework. |
| Renderer | Direct Metal, Direct3D 12, and Vulkan modules | Each release contains only its platform API. A small in-repo egui mesh/texture renderer avoids `wgpu`, translation layers, and a second rendering abstraction. |
| Highlighting | `syntect` with its `fancy-regex` backend | Mature Sublime-compatible grammar support, a pure-Rust regex path, and precompiled syntax dumps for fast startup. |
| Syntax manifests | `serde` and JSON | A small, versioned package format with broad tooling support. |
| Syntax downloads | `ureq` with Rustls | A small blocking, pure-Rust HTTP client fits a short-lived CLI operation; no async runtime is needed. |
| Package extraction and checksums | `zip` and `sha2` | Standard package transport plus bounded extraction and SHA-256 verification. |
| User data location | `directories` | Correct per-platform application data paths without custom OS branches. |
| Safe temporary files | `tempfile` | Same-directory temporary writes before replacement. |

Keep dependency features narrow. In particular, do not enable image loaders, web support, persistence, alternate graphics APIs, or every bundled `syntect` syntax. Use `egui` with default fonts, `egui-winit` with accessibility, a text-only clipboard backend, and target-gated graphics bindings so macOS never compiles Vulkan or D3D12, Windows never compiles Metal or Vulkan, and Linux never compiles Metal or D3D12. Disable `syntect` default features and select the `regex-fancy`, parsing, YAML-load, dump-load, and dump-create features explicitly; otherwise Cargo can pull in the native Oniguruma library and violate the Rust-only requirement.

### Renderer configuration

Build exactly one direct graphics module per release target:

| Target | Target-gated bindings | Runtime backend |
| --- | --- | --- |
| macOS | `metal` | Metal and `CAMetalLayer` |
| Windows 10+ | `windows` Win32 Graphics features | DXGI and Direct3D 12 |
| Linux | `ash` plus `ash-window` | Vulkan 1.1+ and `VK_KHR_swapchain` |

Linux release builds also enable the required Wayland and X11 `winit` features. Other platforms do not compile those dependencies. Each backend implements only the operations egui needs: create a device and presentation surface, upload RGBA textures and vertex/index buffers, set a clipped viewport, draw premultiplied-alpha triangles, present, resize, and recreate a lost or out-of-date swapchain. No general renderer abstraction, shader graph, render graph, or public graphics API is part of v1.

Prefer an integrated or low-power device because a text editor does not need a discrete GPU. Metal selects a non-headless low-power `MTLDevice` when available; D3D12 uses `IDXGIFactory6::EnumAdapterByGpuPreference` with `DXGI_GPU_PREFERENCE_MINIMUM_POWER`; Vulkan prefers an integrated device with graphics and present support. `EDITUR_GPU_DEVICE` may select an adapter by name for diagnostics, and `EDITUR_GPU_VALIDATION=1` enables available validation layers in development. If the required API, device, queue, surface, or swapchain cannot be created, print the backend and contextual native error to the attached terminal and exit nonzero. Do not silently fall back to another graphics API.

### Project shape

Start with one binary crate, not a workspace. Keep the implementation in a few concrete modules:

- `cli`: parse arguments and route editor or syntax-package commands.
- `app`: own UI state and draw the three window regions.
- `buffer`: text, cursor-facing metadata, dirty state, line endings, and disk fingerprint.
- `file_io`: open, conflict-check, and safe save.
- `renderer`: expose the same small lifecycle from target-gated Metal, D3D12, and Vulkan modules.
- `search`: build and query the background filename/content index.
- `tree`: lazily list and sort directory entries.
- `syntax`: detect languages, load the syntax cache, highlight text, and manage packages.

Do not create traits for these modules in advance. Add an interface only when a second real implementation exists.

### Runtime model

- Keep UI and editor state on the main thread.
- Do not add Tokio or another async runtime.
- Start the recursive search index in a background thread; never wait for it before drawing the editor.
- Load one immutable compiled syntax set at startup.
- Request repaint only after input or state changes; the editor should do no work while idle.
- Create one native graphics device, queue, presentation layer/swapchain, and egui renderer for the window and reuse them for the process lifetime.
- Hold one `String` buffer. It matches the underlying text widget and avoids converting a rope on every frame.
- Cache the highlighted layout by buffer revision, syntax, theme, and available width.

The deliberate v1 ceiling is normal source files up to 1 MiB. Files above 5 MiB open as plain text with a warning. If real usage demands larger files, replace the editor widget and `String` together with a virtualized rope-backed implementation; adding a rope alone would only add conversions.

## 6. File safety

Opening a file records its size, modification time, and content hash. Saving follows this sequence:

1. Compare the current disk fingerprint with the one recorded at open or last save.
2. If it changed, stop and offer Reload, Save As, or Cancel. Never overwrite silently.
3. Encode the buffer as UTF-8 while preserving the file’s detected LF or CRLF style.
4. Write to a temporary file in the destination directory.
5. Preserve the existing file permissions, flush the write, and replace the destination.
6. Update the fingerprint and clear the dirty flag only after replacement succeeds.

Reject binary and invalid UTF-8 input with a clear message. Do not perform lossy decoding because a quick editor must not corrupt a file it does not understand.

## 7. Syntax highlighting and extensions

### Default behavior

Embed only two syntax definitions in the base binary:

- Rust, selected for `.rs` files.
- Plain Text, used as the safe fallback.

Compile them into a `syntect` syntax dump during the release build and load the dump directly at startup. Do not parse a catalog of YAML grammars every time the editor opens.

### User workflow

```text
$ editur syntax list
Installed:
  rust (built in)

Available:
  javascript
  markdown
  python

$ editur syntax install python
Installed python 1.0.0

$ editur notes.py
```

The next editor launch discovers `.py` automatically. No config file, restart command, or language mapping is required.

Also allow `editur syntax install ./python.editur-syntax` for package authors and offline use. This is an installation command, not a folder the user must maintain.

### Package format

An `.editur-syntax` package is a ZIP-formatted, data-only archive:

```text
manifest.json
syntaxes/*.sublime-syntax
LICENSES/*
```

The manifest contains only:

- Format version.
- Stable language ID and display name.
- Package version and minimum compatible Editur version.
- Filename extensions and exact filenames.
- Included grammar files and any syntax-package dependencies.

It contains no executable hooks. The installer rejects absolute paths, `..` traversal, symlinks, duplicate language IDs, unknown manifest versions, downloads above 2 MiB, unpacked content above 8 MiB or 128 entries, invalid checksums, and grammars that fail to compile.

### Official catalog

Host a static versioned index and package files with the project’s release artifacts. `syntax install <language>` performs a blocking HTTPS download, validates the advertised SHA-256 checksum, installs into the platform application-data directory, and rebuilds one combined syntax dump atomically.

The GUI only reads that combined dump. Network access, package extraction, YAML parsing, and grammar linking happen in the explicit install command, never on the editor startup path.

The initial registry needs only a few common languages. Adding a generic plugin API is intentionally out of scope; syntax data solves the stated extension need with a much smaller security and maintenance surface.

## 8. Performance contract

Record the reference machine and measure release builds. Initial targets are:

| Metric | Target |
| --- | --- |
| Warm command-to-editable-window | ≤150 ms median |
| Cold command-to-editable-window | ≤300 ms at p95 |
| Input-to-painted-frame for a 1 MiB Rust file | <16 ms at p95 |
| Search update after a keystroke once indexed | <50 ms at p95 |
| Idle CPU after the window settles | <1% |
| Base resident memory with a small file | ≤60 MiB |
| Stripped release binary | ≤30 MiB per architecture before packaging |

Milestone 0 establishes the real baseline before further optimization. Use release builds and profile any missed budget; do not add a custom allocator, rope, background worker pool, render graph, or renderer framework based on assumption alone.

Start with full-buffer re-highlighting when the buffer revision changes and reuse the cached layout at all other times. If the typing budget fails on the 1 MiB fixture, change only the highlighter to cache parser state per line and reparse from the first changed line until state converges.

## 9. Error handling and observability

- Fallible operations return `Result`; production paths do not use `unwrap` or `expect`.
- Terminal subcommands print one contextual error and exit nonzero.
- GUI failures become a visible non-blocking error banner or modal, depending on whether user action is required.
- Development builds can log startup phase timings when `EDITUR_LOG=debug` is set.
- Release builds contain no telemetry and make no network request except an explicit syntax catalog/install or update command.

Use standard error types first. Add a small derived error enum only when repeated manual conversions become noisy; do not introduce an error hierarchy for six modules.

## 10. Implementation milestones

### Milestone 0: prove the shell

- Create the single binary crate, `winit` event loop, and minimal native window.
- Parse `editur [PATH]`, `--help`, and `--version` with the standard library.
- Build with only the target's direct graphics bindings and required window-system features.
- Select the low-power native device, log its name/API in debug mode, and surface initialization failures in the attached terminal.
- Verify Metal on macOS, D3D12 on Windows, and Vulkan on Linux. Verify that a Linux host without Vulkan receives a useful terminal error rather than another graphics backend.
- Measure command-to-window time by phase: process start, path loading, native device creation, presentation-layer/swapchain creation, and first editable frame.
- Measure binary size, idle CPU, and memory on one reference machine per operating system.

Exit condition: `editur` reliably opens and closes a blank window through the platform's direct Metal, D3D12, or Vulkan backend, reports a useful terminal error when graphics initialization is impossible, and has a recorded startup baseline for each operating system.

### Milestone 1: safe single-file editing

- Implement path resolution, UTF-8 loading, one wrapping multiline editor, dirty state, and status bar.
- Implement LF/CRLF preservation, external-change detection, safe replacement, and unsaved-change prompts.
- Wire standard save, undo/redo, and close behavior.
- Add focused tests for path resolution, line-ending round trips, conflict detection, and failed-save dirty state.

Exit condition: opening, editing, saving, and creating a file cannot silently discard or corrupt data in the covered cases.

### Milestone 2: file tree

- Add the resizable/collapsible sidebar.
- Read directories lazily, sort entries, hide `.git`, and avoid following directory symlinks.
- Add mouse and keyboard navigation.
- Route file changes through the same unsaved-buffer prompt used on close.
- Add the floating project search palette with separately labeled filename and content results.
- Index in the background, cap indexed file size, skip generated/vendor directories, and route opened results through the same unsaved-buffer prompt.

Exit condition: a user can open a directory, navigate or search to a file without a mouse, edit it, save it, and switch files safely.

### Milestone 3: Rust highlighting

- Embed the precompiled Rust and Plain Text syntax set.
- Detect syntax from the selected file.
- Cache layout so idle frames never re-highlight unchanged text.
- Add a representative Rust fixture and measure typing latency at small, medium, and 1 MiB sizes.

Exit condition: Rust highlighting is correct enough for comments, strings, raw strings, macros, keywords, and types while meeting the measured interaction budget.

### Milestone 4: syntax packages

- Define and validate the versioned package manifest.
- Implement list, local install, remove, and combined-cache rebuild.
- Add official catalog resolution, HTTPS download, size limits, and checksum validation.
- Publish Python and Markdown as end-to-end sample packages.
- Test corrupt archives, traversal attempts, invalid manifests, failed grammar compilation, and interrupted cache replacement.

Exit condition: a clean install can run `editur syntax install python` and immediately receive Python highlighting without editing configuration.

### Milestone 5: release hardening

- Run formatting, Clippy with warnings denied, unit tests, and CLI integration tests.
- Exercise accessibility labels and keyboard-only operation.
- Test permission errors, deleted files, renamed directories, read-only files, Unicode paths, and simultaneous external edits.
- Test integrated-GPU selection, Vulkan/DXGI swapchain recreation, Metal drawable unavailability, suspend/resume, and monitor/DPI changes.
- Benchmark release artifacts on each supported OS and profile missed targets.
- Produce signed archives with the native platform icon and installation instructions that place `editur` on `PATH`.
- Publish checksum-verifying shell and PowerShell bootstrap installers for a one-command initial installation.
- Publish checksum-verified native updater assets whenever the `release` branch is pushed, while retaining versioned `v*` releases.

Exit condition: every product requirement and performance gate has reproducible evidence, or a documented platform-specific exception approved before release.

## 11. Minimal test strategy

Prefer small tests around behavior that can lose data or break the extension boundary:

- Unit tests for argument parsing, path-root selection, syntax detection, manifest validation, line-ending handling, and disk fingerprint comparison.
- Temporary-directory integration tests for create, save, conflict, permissions, syntax install/remove, and atomic cache replacement.
- One GUI smoke test per supported OS for launch, search, type, save, and close; do not build a large screenshot suite.
- One benchmark fixture set: small Rust, 10,000-line Rust, 1 MiB Rust, and a file above the 5 MiB ceiling.

Required local release checks:

```text
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --locked
cargo build --release --locked
```

## 12. Release definition of done

Editur v1 is done when a new user can:

1. Install one native binary on `PATH`.
2. Run `editur src/main.rs` from a terminal and begin typing in the focused editor.
3. Navigate the surrounding project from the sidebar.
4. Search surrounding filenames and file contents from `Cmd/Ctrl+F` with live categorized results.
5. Save without line-ending damage or silent external-change overwrite.
6. Install Python highlighting with one command and have it selected automatically for `.py` files.
7. Use the core workflow entirely by keyboard.
8. Observe the documented startup, latency, idle, memory, and binary-size results on the reference systems.
9. Run `editur update` to install the latest successful build from the `release` branch without opening the UI.

Anything beyond that is a candidate for a later release, not a prerequisite for shipping the focused editor described here.

## 13. Technical references

- [egui repository and integration overview](https://github.com/emilk/egui)
- [`egui-winit` input and window integration](https://docs.rs/egui-winit/latest/egui_winit/)
- [`winit` cross-platform windowing](https://docs.rs/winit/latest/winit/)
- [`metal` Rust bindings](https://docs.rs/metal/latest/metal/)
- [`windows` Direct3D 12 bindings](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/Graphics/Direct3D12/)
- [`ash` Vulkan bindings](https://docs.rs/ash/latest/ash/)
- [Vulkan swapchain extension](https://registry.khronos.org/vulkan/specs/latest/man/html/VK_KHR_swapchain.html)
- [egui multiline `TextEdit`](https://docs.rs/egui/latest/egui/widgets/text_edit/struct.TextEdit.html)
- [`syntect` syntax-set builder](https://docs.rs/syntect/latest/syntect/parsing/struct.SyntaxSetBuilder.html)
- [`syntect` binary syntax dumps](https://docs.rs/syntect/latest/syntect/dumps/)
- [`directories` project-data locations](https://docs.rs/directories/latest/directories/struct.ProjectDirs.html)
