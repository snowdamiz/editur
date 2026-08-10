# LSP support implementation plan

## 1. Purpose

Add useful Language Server Protocol support without turning Editur into a language-server installer or a second extension platform. Editur remains a small native editor; user-installed language servers run as child processes and provide language intelligence over standard input/output.

### Audience and outcome

This plan is for the engineer implementing the feature. After reading it, they should be able to add the first useful LSP release, including its required Settings UI, without reopening product scope, protocol, process-lifecycle, or editor-integration decisions.

### Decision summary

- A new UI is required because executable discovery alone cannot handle nonstandard installations, custom arguments, failures, or intentional disablement.
- Settings are an in-window route, not another native window.
- The first release supports diagnostics, completion, hover, and go to definition.
- Editur uses user-installed servers and never invokes a shell to launch them.
- Protocol work stays off the UI thread and starts only for an open supported document.
- LSP semantic tokens do not replace the existing built-in syntax highlighting.

## 2. Current-state findings

The existing editor already provides most of the integration points LSP needs:

- Each open tab owns one `Buffer`; split panes reference tabs rather than duplicating document state.
- All normal edits converge where the buffer revision and line indexes are updated.
- Save, Save As, external reload, tab close, and project replacement already have explicit lifecycle points.
- The main loop is event-driven and existing background controllers communicate through bounded commands and normalized events.
- The renderer already composes syntax, find-match, and bracket presentation layers.
- Markdown layout can render server-supplied hover documentation.
- Application data already has a platform-correct location, but only window geometry and agent-provider choice are persisted today.

There is no general Settings screen, no LSP process, and no editor status bar or Problems panel to reuse.

## 3. First-release product contract

The first useful LSP release must:

- Discover a supported language server on `PATH`, or use an explicit executable and argument list from Settings.
- Start a server lazily when the first matching document is opened and stop it when its last document closes or the project changes.
- Run one server process per server preset and Editur project root. TypeScript and JavaScript documents share one process.
- Complete `initialize`/`initialized`, capability discovery, `shutdown`/`exit`, and document synchronization correctly.
- Send `didOpen`, debounced `didChange`, `didSave`, and `didClose` for each unique open buffer path.
- Keep split panes from opening or changing the same protocol document twice.
- Render diagnostics against the current document version and provide next/previous diagnostic navigation.
- Offer capability-backed completions from automatic trigger characters and `Ctrl+Space`.
- Show bounded plain-text or Markdown hover information after a short stationary-pointer delay.
- Navigate to one definition directly and show a small chooser when the server returns several.
- Ignore stale completion, hover, definition, and diagnostic results after the buffer or cursor context changes.
- Keep all process I/O, framing, JSON parsing, and response dispatch off the UI thread.
- Wake the event loop when an LSP event arrives; never add idle polling or a permanent repaint loop.
- Surface starting, ready, missing, stopped, and failed states in Settings without blocking normal editing.
- Preserve normal undo, dirty-state, conflict detection, and safe-save behavior for completion edits.
- Shut down the server process and its descendants on normal exit, failure, settings changes, and project replacement.
- Add no network request and no measurable work for projects that never open a supported document.

### Initial server presets

Ship a small tested catalog rather than a generic configuration language:

| Languages | Preset | Auto-discovered command | Default arguments |
| --- | --- | --- | --- |
| Rust | rust-analyzer | `rust-analyzer` | none |
| TypeScript, TSX, JavaScript, JSX | TypeScript Language Server | `typescript-language-server` | `--stdio` |
| Python | Pyright | `pyright-langserver` | `--stdio` |
| Go | gopls | `gopls` | none |
| C and C++ | clangd | `clangd` | none |

A custom executable and arguments may replace a preset's command, but the preset continues to own its language IDs and file mapping. Add another preset only with a fake-server test and a real native smoke test.

### Deliberately deferred

The first release will not include:

- Downloading, updating, or licensing language servers.
- Arbitrary user-defined language IDs, extension mappings, root markers, or server catalogs.
- Per-project Editur settings or checked-in editor configuration.
- Server environment-variable editing or shell command strings.
- Formatting, rename, references, code actions, workspace symbols, inlay hints, code lenses, signature help, call hierarchy, or execute-command support.
- `workspace/applyEdit`, file-operation requests, or edits to unopened files.
- Snippet expansion, completion-item resolve, or completion additional edits.
- Semantic tokens; Syntect remains the syntax-highlighting source.
- A Problems panel, outline view, breadcrumb bar, minimap, or new status bar.
- TCP, socket, remote, or WebSocket LSP transports.
- Multiple roots per Editur window or automatic nested-project root detection.
- Automatic process restart loops.

These are follow-ups only when the basic local stdio path is reliable and a concrete user workflow requires them.

## 4. Settings UI decision and layout

### Why Settings is required

Automatic discovery covers the happy path but not these normal cases:

- A server is installed outside the inherited `PATH`.
- A server requires `--stdio` or another platform-specific argument.
- A user wants LSP disabled globally or for one language.
- A launch succeeds but initialization fails and the user needs actionable status.
- A changed executable or argument list must restart only the affected process.

Environment variables and project-local initialization JSON are not required for the first release.

### Route and navigation

Add one full-window Settings route inside the existing window. It preserves tabs, panes, terminal state, Agent state, and unsaved buffers behind it.

- Open it from a gear button in the title bar or `Cmd/Ctrl+,`.
- `Back to app`, `Escape`, or `Cmd/Ctrl+,` returns to the prior editor or Agent view.
- Closing the application from Settings follows the existing unsaved-buffer flow.
- Settings never creates a second renderer, window, or event loop.

Use the supplied screenshot's layout proportions and hierarchy:

```text
+--------------------------+-----------------------------------------------+
|  ← Back to app           |  Language Servers                             |
|                          |                                               |
|  [ Search settings... ]  |  General                                      |
|                          |  +-----------------------------------------+  |
|  Editor                  |  | Enable language servers          [on] |  |
|    Language Servers      |  | Start only for supported open files    |  |
|                          |  +-----------------------------------------+  |
|                          |                                               |
|                          |  Servers                                      |
|                          |  +-----------------------------------------+  |
|                          |  | Rust      Ready          [ Auto      v] |  |
|                          |  | TypeScript Not found     [ Auto      v] |  |
|                          |  | Python    Stopped        [ Off       v] |  |
|                          |  | Go        Custom         [ Custom    v] |  |
|                          |  | C / C++   Ready          [ Auto      v] |  |
|                          |  +-----------------------------------------+  |
+--------------------------+-----------------------------------------------+
```

The navigation rail is about 270 logical pixels wide. The content column is centered, capped near 780 pixels, and scrolls independently. Reuse the current dark palette, border, typography, control radii, and hover behavior instead of copying the reference application's exact colors.

Do not add empty General, Appearance, Account, or Plugins pages merely to fill the rail. The initial rail contains one `Editor` group and one selected `Language Servers` item. Search filters the page and server rows and becomes useful as more real settings are added.

### Controls and behavior

The `General` card contains one global `Enable language servers` toggle. Turning it off stops every LSP process after saving the setting.

The `Servers` card contains one row per preset:

- Language and preset name.
- Runtime status: `Not started`, `Starting`, `Ready`, `Not found`, `Failed`, or `Stopped`.
- Mode selector: `Auto`, `Custom`, or `Off`.
- A concise error below a failed row, capped to a few lines.

Selecting `Custom` expands the row with:

- `Executable`: an absolute path or bare executable name.
- `Arguments`: one argument per line so spaces remain unambiguous on every OS.
- `Apply and restart` and `Reset to auto` actions.

The first switch to `Custom` pre-fills the discovered command and preset arguments. The top of the server card also provides `Rescan`. It repeats executable discovery but does not install anything. Toggle and mode changes save immediately; custom text fields save only through `Apply and restart` so typing does not repeatedly restart a process.

All controls need keyboard focus, accessible labels, visible focus treatment, and the same minimum hit sizes as existing editor controls.

## 5. Settings data contract

Persist a small global `settings.json` in the existing application-data directory. Store only deviations from defaults:

```json
{
  "languageServers": {
    "enabled": true,
    "servers": {
      "rust-analyzer": {
        "mode": "custom",
        "command": "/opt/tools/rust-analyzer",
        "args": []
      },
      "pyright": {
        "mode": "off"
      }
    }
  }
}
```

Requirements:

- Missing file and missing fields use defaults.
- Bound the file at 64 KiB before reading it.
- Reject symlinks, invalid JSON, unknown modes, empty custom commands, NUL characters, more than 64 arguments, an argument above 4 KiB, or a total custom command above 32 KiB.
- Show a load error in Settings and keep the invalid file untouched. Do not silently replace it with defaults.
- Write through a same-directory temporary file and atomically replace the destination.
- Persist configuration only, never detected paths, process IDs, statuses, diagnostics, or protocol responses.
- A bare custom command uses the inherited `PATH`; an absolute command is used exactly as entered.
- Never perform shell expansion, quote parsing, environment interpolation, or command concatenation.

Keep the model concrete. It needs a settings struct, a language-server section, and one override struct; it does not need a generic preference registry, schema engine, observer bus, or migration framework.

## 6. Runtime architecture

```text
Editor and Settings UI on main thread
        |
        | bounded commands / normalized events
        v
LSP state: documents, requests, diagnostics, popup state
        |
        | one controller per active preset and project root
        v
controller thread + bounded stdout/stderr reader threads
        |
        | Content-Length framed JSON-RPC 2.0 over stdio
        v
user-installed language server process
```

### Concrete code shape

Add only two product modules initially:

- `settings`: typed defaults, bounded load/validation, and atomic save.
- `lsp`: preset catalog, process lifecycle, stdio framing, controller commands/events, request tracking, and LSP-to-editor normalization.

Keep Settings drawing and editor integration with the application state. Extend the retained editor output only with caret geometry, the hovered character, and the last inserted scalar needed for completion triggers; keep LSP types out of the editor surface. Add the fake LSP binary and its integration test alongside the existing fake-agent testing pattern.

Start with one `lsp` module. Split transport or controller code into submodules only if the implemented file becomes difficult to navigate. Do not add an LSP trait, server factory, async runtime, or general subprocess framework.

One small child-process guard may be shared with the existing Agent process code now that a second feature needs descendant-safe teardown. On Unix it owns a process group; on Windows it owns a kill-on-close Job Object. Keep provider-specific behavior out of that helper.

### Dependency choice

Use the latest pinned compatible `lsp-types` release for stable request, response, capability, diagnostic, completion, hover, and location types. At planning time, `0.97.0` is the current release and covers the stable LSP 3.16 type surface required here. Do not enable proposed features.

Do not add `lsp-server`: it is a server scaffold and Editur is the client. Implement the small client transport with the standard library, existing `serde`/`serde_json`, and bounded channels. Do not add Tokio.

Before retaining the dependency, measure compile time, stripped release size, and transitive crates. Fall back to local types only if the measured cost breaks the existing product budget.

## 7. Process discovery and lifecycle

### Discovery

For `Auto`, search inherited `PATH` entries in order with the standard library. On Windows, honor executable suffixes supported by the process API. Accept a regular file or a symlink whose final target is a regular file; reject loops and directories. Never run `which`, `where`, a shell, or a package manager.

For `Custom`, resolve an absolute path directly or search a bare name through the same path routine. Launch with an argument vector and the Editur project root as the working directory.

Discovery and process start occur on the controller thread. A missing server updates status to `Not found`; it does not show a modal every time a matching file opens.

### Lifecycle

1. The first supported document requests its preset.
2. The controller discovers and spawns the process with piped stdin, stdout, and stderr.
3. Editur sends `initialize` with one workspace folder and the capabilities it truly supports.
4. After a successful response, Editur sends `initialized` and opens every waiting document for that preset.
5. Further tabs reuse that process.
6. When the last matching tab closes, the controller sends `shutdown`, waits for its response, sends `exit`, and joins the process.
7. If graceful shutdown exceeds one second, terminate the owned process tree.

Changing a server mode, executable, or arguments restarts only that preset. Replacing the project stops all old-project servers before the new project starts any.

On unexpected exit, keep diagnostics already visible but mark them stale, report bounded stderr in Settings, and wait for `Rescan`, `Apply and restart`, or another explicit open after settings change. Do not create an automatic restart loop.

## 8. Protocol transport and capability contract

### Framing and bounds

Read and write standard LSP `Content-Length` frames over byte streams.

- Parse headers case-insensitively until the empty line.
- Require exactly one valid decimal `Content-Length`.
- Reject a header block above 16 KiB and a body above 16 MiB.
- Read the exact body length before JSON parsing.
- Handle partial reads and multiple queued frames.
- Bound retained stderr to the newest 64 KiB of complete UTF-8 lines.
- Treat malformed stdout, invalid JSON-RPC envelopes, duplicate response IDs, and premature EOF as a failed connection.
- Ignore unknown notifications after optional debug logging.
- Reply `Method not found` to unknown server requests so the server never waits forever.

When `EDITUR_LOG=debug` is set, log method names and IDs, not full source text, completion payloads, hover contents, environment values, or settings.

### Client capabilities

Advertise only:

- Full and incremental text synchronization handling.
- Publish diagnostics with version support.
- Completion with plain-text insertion and supported trigger characters.
- Hover with plain text and Markdown.
- Definition locations and location links.
- Workspace folders with one folder.

Do not advertise dynamic registration, snippets, completion resolve, apply-edit, configuration, progress UI, semantic tokens, formatting, rename, code actions, or execute-command support.

Use UTF-16 positions, which is the protocol fallback when no alternate position encoding is negotiated. This avoids proposed capability fields and still requires correct surrogate-pair accounting.

### Required server requests

Even conservative servers can send requests. Handle them explicitly:

| Request | Response |
| --- | --- |
| `workspace/configuration` | An array of `null` entries because first-release server-specific initialization settings are absent. |
| `workspace/workspaceFolders` | The one active Editur project folder. |
| `window/showMessageRequest` | `null`; mirror the message into bounded server status detail. |
| `workspace/applyEdit` | `applied: false` with a concise reason. |
| Unknown request | JSON-RPC `Method not found`. |

Never execute a command supplied by a server.

## 9. Document synchronization

### Document identity

Map canonical local paths to `file` URIs and a preset-owned language ID. Reject non-file URIs received for navigation. Keep one protocol document per unique buffer path regardless of tab placement or split-pane presentation.

Maintain an LSP document version independent of the wrapping buffer revision. Increment it for every sent change and tag all feature requests with path, version, cursor offset, and request ID.

### Open, change, save, close

- `didOpen`: send the normalized in-memory UTF-8 text, language ID, and version after initialization.
- `didChange`: coalesce edits for 50 ms, then send one update using the server's advertised sync kind.
- `didSave`: send only after conflict-checked safe save succeeds and only when advertised; include text only when requested by the server.
- `didClose`: send after the last tab for that path closes.
- Save As: close the old URI, then open the new URI with a fresh protocol version.
- Clean external reload: send the reloaded text as a change.
- Dirty external conflict: keep the current in-memory document open and preserve the existing conflict flow.
- Files above the editor's 5 MiB large-file threshold do not start or join LSP.

Treat an absent or `None` text-sync capability as unsupported for this release and show that in the preset's Settings status. Honor full versus incremental changes. Send open/close for explicit support and for the legacy sync-kind form used by older servers.

For an incremental-sync server, retain the last-sent snapshot and compute one valid replacement with longest common UTF-8-boundary prefix and suffix scans. Convert the replaced old range to zero-based UTF-16 line/character positions and send the inserted middle text. Any old and new string can be represented by this single replacement, so it is correct for paste, undo, redo, indent, and IME commits without restructuring editor input.

```rust
// ponytail: one bounded diff scan per synced change; emit editor-native deltas if the 1 MiB typing benchmark misses budget.
```

For a full-sync server, send the current text without the range. Replace the retained snapshot only after the notification has been queued successfully.

Clear completion and hover immediately on a local edit. Mark diagnostics stale until a matching or newer publish notification arrives.

## 10. Feature behavior

### Diagnostics

Store diagnostics by document URI and optional server version. Drop results older than the latest sent document version. For versionless diagnostics, accept the newest notification but mark it stale after the next local edit until another notification arrives.

Convert UTF-16 ranges to clamped byte spans against the matching buffer text. Invalid or reversed ranges are ignored and counted in debug diagnostics; they must not panic rendering.

Compose diagnostics in the existing presentation pass so find and bracket highlights do not erase diagnostic underlines:

- Error: red underline.
- Warning: amber underline.
- Information and hint: muted blue underline.
- Empty ranges: a small marker beside the affected line number.

Hovering an underline or marker shows severity, source/code when present, and the message. A compact error/warning count appears in the pane header. `F8` jumps to the next diagnostic in the active file and `Shift+F8` moves backward, wrapping at the ends. Do not add a Problems panel yet.

Cap retained diagnostics at 5,000 per document and surface truncation in Settings debug detail.

### Completion

Request completion when:

- The user presses `Ctrl+Space`.
- A typed character matches a trigger advertised by the server.

The popup is anchored to the retained editor's caret rectangle and shows at most 12 visible rows with label, kind, and one-line detail. Retain no more than 500 items and preserve the server order.

Up/Down changes selection, Enter or Tab accepts, and Escape closes. Consume these keys before normal editor handling so acceptance does not also insert a newline or indentation.

Prefer a same-document `textEdit`; otherwise replace the current word range with `insertText` or the label. Apply it through the editor surface's existing selection-replacement path so undo records one edit and the buffer remains dirty but unsaved. Advertise no snippets, omit items explicitly marked as snippets, ignore server commands and additional edits, and discard a response if its path, version, or cursor context is stale.

### Hover

Expose the character under the stationary pointer from the retained editor surface. After 400 ms without pointer movement, scrolling, key input, or document change, request hover if the server advertises it.

Render plain text directly and Markdown through the existing compact Markdown layout. Bound source content at 256 KiB and the popup to the current pane. Dismiss it on movement, edit, scroll, click away, Escape, tab change, or a newer request.

### Go to definition

Request definition on `F12` or primary click while the platform command modifier is held. Accept `Location`, `Location[]`, or `LocationLink[]`.

- One valid local file location opens or activates the tab and moves the cursor to the clamped target position.
- Several locations open a bounded chooser using the existing search-result visual language.
- Missing files, directories, invalid ranges, non-file URIs, and unparseable paths produce one recoverable error.
- A target outside the current project may open in a tab but does not change the file-tree root.

Cap the chooser at 200 locations. Do not implement references through this chooser in the first release.

## 11. UI-thread integration

The application owns renderable LSP state and polls only already-buffered events during a frame. Each event is normalized before it reaches UI code:

| Controller event | UI effect |
| --- | --- |
| StateChanged | Update the corresponding Settings row. |
| Diagnostics | Replace diagnostics for one path if current. |
| Completion | Open or update the caret popup if current. |
| Hover | Open the pointer popup if current. |
| Definitions | Navigate or open the location chooser if current. |
| ServerMessage | Retain bounded status detail in Settings. |
| ProcessExited | Mark the preset failed and dismiss its transient popups. |

The UI sends concrete commands:

| Command | Meaning |
| --- | --- |
| Open | Start/reuse a preset and open one document. |
| Change | Synchronize a debounced full snapshot; the controller chooses full or incremental wire form. |
| Save | Notify after a successful disk save. |
| Close | Close one protocol document. |
| Complete | Request completion for a tagged cursor context. |
| Hover | Request hover for a tagged pointer context. |
| Definition | Request definitions for a tagged cursor context. |
| Restart | Apply changed settings to one preset. |
| Rescan | Repeat discovery for configured presets. |
| Shutdown | Gracefully stop the controller and process tree. |

Bound command queues. Coalesce pending changes by document and discard superseded hover/completion requests rather than blocking typing behind them.

## 12. Data safety and trust boundaries

- Treat the configured executable and every server message as untrusted input.
- Launch one executable plus an argument vector directly; never invoke a shell.
- Do not send environment dumps, credentials, unrelated files, or the user's home directory through initialization options.
- Send only text for documents the user opened in Editur.
- Accept navigation only to local `file` URIs and load them through existing UTF-8/binary checks.
- Do not apply workspace edits, server commands, unopened-file edits, or disk writes in the first release.
- Completion changes only the current in-memory buffer and uses existing undo and safe-save behavior.
- Bound frames, collections, messages, strings, stderr, and chooser rows before retaining or drawing them.
- Unknown and invalid protocol data becomes a connection error or ignored item, never a panic.
- On shutdown, close stdin, stop reader threads, and terminate the owned process tree if graceful exit fails.

## 13. TDD and verification strategy

Build vertical slices. Each controller or conversion behavior starts with one failing test and the smallest implementation that passes.

### Fake server

Add a deterministic fake LSP process that speaks framed JSON-RPC over stdio. It must be able to:

- Split headers and bodies across writes.
- Return static and missing capabilities.
- Record initialize, open, change, save, close, shutdown, and exit messages.
- Publish versioned and versionless diagnostics.
- Return completion, hover, one definition, and several definitions.
- Send unknown notifications and server requests.
- Emit malformed frames, oversized lengths, bounded stderr, delayed stale responses, and unexpected exit.
- Spawn a test descendant for process-tree teardown checks.

Automated tests must not require installed language servers, network access, user settings, or project credentials.

### Focused automated checks

Settings tests:

- Missing settings use defaults.
- Valid overrides round-trip.
- Invalid, oversized, symlinked, or truncated files are rejected without replacement.
- Atomic-save failure leaves the old file intact.
- Argument and command bounds are enforced.

Protocol tests:

- Partial and back-to-back frames parse correctly.
- Invalid headers, body lengths, JSON, response IDs, and EOF fail clearly.
- Initialization advertises only implemented capabilities.
- Unknown requests receive `Method not found`.
- Shutdown sends `shutdown`, then `exit`, and leaves no descendant.

Document tests:

- One buffer shown in two panes produces one `didOpen` and one change stream.
- Full and incremental synchronization match server capability.
- Prefix/suffix replacement handles insert, delete, replace, undo, paste, newline, and emoji.
- UTF-16 positions handle non-BMP characters and clamp malformed server ranges.
- Save As closes the old URI and opens the new one.
- External reload synchronizes; dirty conflict preserves the user's text.
- Stale diagnostic, completion, hover, and definition responses are ignored.

Feature tests:

- Diagnostic overlays preserve find and bracket formatting.
- Completion keys are consumed before editor newline/tab behavior.
- Accepted completion is one undoable dirty edit.
- Hover Markdown is bounded and cached.
- One definition navigates directly; several use the chooser.
- Large and unsupported files start no server.

Keep UI tests state- and shape-focused. Do not create a broad screenshot suite or test `egui`, `serde_json`, or `lsp-types` internals.

### Native smoke checks

Before release, run the same short manual scenario on macOS, Linux, and Windows for each preset:

1. Auto-discover the real server.
2. Open a small project and receive one diagnostic.
3. Accept one completion and undo it.
4. Show hover documentation.
5. Navigate to a definition.
6. Change to a custom executable, restart, disable, and re-enable.
7. Close Editur and confirm no language-server descendant remains.

Record tested server versions because the servers are not shipped or pinned by Editur.

## 14. Performance contract

Measure in release mode against the existing baseline:

| Metric | Acceptance target |
| --- | ---: |
| Startup with no supported document | At most 2 ms regression |
| First editable frame with supported document | No wait for discovery, spawn, or initialize |
| Idle CPU after LSP settles | No permanent repaint or polling regression |
| Typing p95 in the existing 1 MiB fixture while synced | Under 16 ms |
| Controller event to visible UI | Under one frame after receipt |
| Change synchronization delay | 50-100 ms after the latest edit |
| Release binary growth | Remain within the existing 30 MiB product target |

Track server process memory separately from Editur. One retained last-sent snapshot per synchronized document is the initial memory ceiling. Do not add a rope, incremental diff library, worker pool, or cache database unless profiling shows the bounded scan is the missed budget.

Check this project's build-directory size before and after adding `lsp-types`, and remove only this project's Rust incremental artifacts when space becomes excessive.

## 15. Implementation phases

### Phase 0: protocol and dependency spike

- Add `lsp-types` temporarily and measure dependency, build, and release-size impact.
- Connect a small harness to rust-analyzer through stdio.
- Prove framing, initialize, open, incremental change, diagnostics, completion, hover, definition, shutdown, and descendant cleanup.
- Record rust-analyzer capabilities and any unsolicited server requests.

Exit condition: the stable subset works without an async runtime, and the dependency remains within the product budget.

### Phase 1: Settings plus Rust diagnostics slice

- Write settings default/validation/atomic-save tests first.
- Add the in-window Settings route and screenshot-derived Language Servers page.
- Add the Rust preset, discovery, process lifecycle, initialization, document open/change/save/close, and normalized status events.
- Render Rust diagnostics and F8 navigation.
- Use the fake server for all failure paths.

Exit condition: a user can configure or auto-discover rust-analyzer, edit without UI blocking, see current diagnostics, disable it, and exit without an orphan process.

### Phase 2: completion

- Add stale-response and Unicode-range tests first.
- Add trigger-character and `Ctrl+Space` requests.
- Add the bounded caret popup and keyboard routing.
- Apply one plain-text completion through normal undo and dirty state.

Exit condition: a completion can be requested, accepted, undone, saved safely, and ignored when stale.

### Phase 3: hover and definition

- Expose hovered-character and caret geometry from the retained surface.
- Reuse compact Markdown layout for hover.
- Add F12/command-click definition requests, direct navigation, and the multiple-location chooser.

Exit condition: hover and definition work without blocking typing or accepting unsafe URI schemes.

### Phase 4: remaining presets

- Add TypeScript/JavaScript, Python, Go, and C/C++ presets one at a time.
- Give each preset a fake-server mapping test and a real smoke record on all supported platforms.
- Add no server-specific code unless capability negotiation cannot express a tested requirement.

Exit condition: all five presets pass the same editor workflow and failure behavior.

### Phase 5: hardening

- Run formatting, Clippy with warnings denied, all tests, and native release builds.
- Exercise malformed/oversized output, crashes, process-tree teardown, project replacement, Save As, external reload/conflict, and settings corruption.
- Measure startup, typing, idle CPU, memory, binary size, synchronization delay, and event-to-paint latency.
- Update user documentation with server-install responsibility, Settings behavior, supported presets, and shortcuts.

Exit condition: every acceptance item has automated or recorded native evidence and the normal non-LSP editor remains within its performance contract.

## 16. Acceptance checklist

- [ ] Settings opens in the existing window from the gear button and `Cmd/Ctrl+,` and returns without losing editor state.
- [ ] The Settings layout has a left navigation rail and grouped content cards matching the supplied reference's hierarchy.
- [ ] Missing or invalid settings cannot crash startup or silently overwrite the settings file.
- [ ] Unsupported files and files above 5 MiB start no LSP work.
- [ ] A supported open document starts only its configured preset in the background.
- [ ] Auto, Custom, Off, global disable, Rescan, and Apply and restart work as described.
- [ ] Processes are launched without a shell and receive only explicit arguments.
- [ ] Split panes do not duplicate protocol documents.
- [ ] Full and incremental sync, save, close, Save As, and external reload are correct.
- [ ] UTF-16 positions are correct for emoji and other non-BMP text.
- [ ] Diagnostics are current, bounded, navigable, and do not erase existing presentation overlays.
- [ ] Completion is bounded, keyboard accessible, undoable, and rejected when stale.
- [ ] Hover is delayed, bounded, and dismissed on context change.
- [ ] Definition handles one or many local file locations and rejects unsafe URIs.
- [ ] Unknown server requests receive a response and never deadlock the server.
- [ ] Failure remains isolated to the affected preset and never freezes or crashes the editor.
- [ ] Normal exit, project replacement, disable, and restart leave no child or descendant process.
- [ ] Fake-server tests require no network or installed language server.
- [ ] Real smoke records exist for every supported preset on macOS, Linux, and Windows.
- [ ] Startup, typing, idle, memory, and binary-size budgets pass.

## 17. Risks and upgrade triggers

| Risk | Initial response | Revisit when |
| --- | --- | --- |
| A server behaves outside advertised capabilities | Fail only that preset, retain bounded details, and add a compatibility change only with a regression test. | A supported preset cannot complete the first-release workflow. |
| UTF-16 conversion or stale versions place UI at the wrong text | Tag every request and test non-BMP text, reload, undo, and rapid edits. | Never relax version checks. |
| Full snapshots and one-range diff scans become expensive | Debounce and measure at the existing 1 MiB ceiling. | Typing p95 misses budget; then emit native editor deltas. |
| Five presets create excessive resident memory | Start lazily and stop after the last matching document closes. | Real mixed-language projects prove churn worse than memory cost. |
| Monorepos need nested roots | Use the explicit Editur project root first. | A tested preset fails materially in common monorepos; then add preset-specific marker discovery. |
| Users need a different server for an existing language | Allow custom executable and arguments within the preset. | A real language needs custom IDs/extensions; then add a bounded custom-preset editor. |
| Users expect server installation | Clearly label `Not found` and custom configuration. | A safe, licensed, cross-platform managed distribution is selected explicitly. |
| Server-initiated edits are requested | Return `applied: false`. | Formatting, rename, or code actions are separately approved with file-safety and undo design. |
| Process descendants survive shutdown | Own a process group or Job Object and test it. | Never accept orphaning as a platform exception. |

## 18. References

- [Language Server Protocol overview](https://microsoft.github.io/language-server-protocol/)
- [LSP 3.17 specification](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/)
- [`lsp-types` documentation](https://docs.rs/lsp-types/latest/lsp_types/)
- [`lsp-server` documentation](https://docs.rs/lsp-server/latest/lsp_server/) — evaluated and intentionally not selected because it scaffolds language servers rather than clients.
- [rust-analyzer](https://github.com/rust-lang/rust-analyzer)
