# Source control sidebar implementation plan

## 1. Purpose and reader

This plan is for the engineer adding a version control (Git) sidebar to Editur. After reading it, they should be able to build the Git backend controller and status model, wire a new left-slot pane that replaces the file tree when active, add the titlebar toggle button beside the terminal button, and implement the interface design recorded in Section 10.

The sidebar covers the daily loop: see what changed, stage it, review the diff, commit it. It is a local-repository surface. It is not a Git client, not a merge tool, and not a remote-hosting integration.

## 2. Product contract

The feature must:

- Add a Source Control pane that occupies the existing left sidebar slot, replacing the file tree while active. The two panes share one slot, one width, and one divider; exactly one renders at a time.
- Add a titlebar toggle icon button placed directly to the right of the terminal button, following the existing rect-helper + interact pattern.
- Add a stable toggle command bindable through the existing keybindings system.
- Show the current branch with ahead/behind counts, the working-tree and index status as grouped file lists, and a commit composer.
- Stage, unstage, and discard changes per file and per group, and commit staged changes with a message.
- Open a read-only unified diff tab for any listed file, reusing the agent diff renderer.
- Run every `git` invocation off the UI thread through a long-lived controller, following the Devin/LSP controller pattern.
- Refresh on deterministic triggers (visibility, window focus, buffer save, agent turn end, after every mutation) — no filesystem watcher and no continuous polling.
- Keep destructive actions (discard) behind explicit danger confirmation.
- Never mix Git status into the agent session's `changed_paths` / `FileChange` state; those are agent-turn semantics, not repository semantics.

## 3. Non-goals

The first release will not include:

- Push, pull, fetch, sync, or any network Git operation.
- Branch creation, switching, merging, or rebasing. The branch row is display-only.
- Hunk or line staging; staging is whole-file only.
- A three-way merge editor. Conflicted files are listed and open as plain files with conflict markers.
- Commit history, log, blame, or stash surfaces.
- Editor gutter change decorations (added/modified/deleted markers next to line numbers).
- Multi-root or submodule enumeration. One repository per workspace root.
- GitHub/`gh` features beyond the existing `inspect_git_workspace` branch/PR peek, which stays where it is (agentic rail).
- Amend, sign-off, GPG, or commit-template support.

Several of these are natural follow-ons (Section 12); none belong in the first slice.

## 4. Existing infrastructure survey

Verified against the current tree; the implementation should reuse these rather than reinvent.

| Need | Exists today | Where |
| --- | --- | --- |
| Left sidebar slot, width, divider, resize | Yes — `sidebar: bool`, `sidebar_width: f32` (default 248), `SIDEBAR_MIN_WIDTH`, max 500, 5px divider | `src/app.rs`, `src/app/layout.rs` (`split_workspace_with_devin`) |
| Sidebar chrome (frosted fill, macOS titlebar inset, settings row) | Yes — `draw_sidebar`, `theme::state::sidebar_material()`, `sidebar_settings_rect` | `src/app/workspace_view.rs` |
| Titlebar toggle buttons | Yes — `file_tree_toggle_rect` → `terminal_toggle_rect` → `agentic_toggle_rect` chain; `draw_terminal_toggle` pattern (interact + hover text + `icons::paint`) | `src/app.rs` ~2391–2404, ~5928–5951 |
| Command catalog and default chords | Yes — `Command` enum + `info!` catalog + `vscode_bindings()` | `src/keybindings.rs`, handled in `src/app/commands.rs` |
| Background controller pattern | Yes — `DevinController` / LSP `Controller`: `sync_channel` commands/events, named worker thread, `wake` repaint closure, UI-side `try_recv` drain | `src/devin/controller.rs`, `src/lsp/controller.rs` |
| Shelling out to `git` | Yes — `git_text` helper (bounded 64 KiB stdout, UTF-8) and `inspect_git_workspace` | `src/devin/controller.rs`, `src/projects.rs` |
| Unified diff model + renderer | Yes — `AgentDiff` / `build_agent_diff` (bounded LCS), `draw_agent_diff_view` / `draw_agent_diff_body`, `theme::diff::*` tokens | `src/app/agent_diff.rs` |
| Buffer reconcile after external change | Yes — `reconcile_buffer` fingerprint compare, invoked on window focus | `src/file_io.rs`, `src/app/runtime.rs` |
| Dialogs, toasts, rows, icon buttons, theme tokens | Yes — `Dialog`, `Toasts`, `selectable_content_row`, `icons::button_sized`, `theme::*` | `src/dialog.rs`, `src/toast.rs`, `src/components.rs`, `src/icons.rs`, `src/theme/` |

What does **not** exist: any `git2`/`gix` dependency (keep it that way — subprocess `git` only), porcelain status parsing, staging/commit invocations, a git-toplevel discovery, git-sourced diffs, or a branch display outside agentic mode.

## 5. Architecture

```text
Titlebar toggle / keybinding command
        |
        v
  EditorApp (sidebar slot: SidebarPane::Files | SidebarPane::SourceControl)
        |
        +-- draw_sidebar ── Files ────────▶ TreeSurface (unchanged)
        |                └─ SourceControl ▶ SourceControlView
        |                                        |
        v                                        v
    GitState  ◀───── GitEvent ───────  GitController (worker "editur-git")
        |                                        |
        └────────── GitCommand ─────────────────┘
                                                 |
                                                 v
                                     std::process::Command::new("git")
```

### 5.1 Ownership boundary

Three concrete pieces, in a new `src/git/` module (`controller.rs`, `status.rs`):

- `GitController` owns the worker thread, command/event channels, `git` subprocess invocation, output bounding, generation counters, and the `wake` repaint closure. Mirror `DevinController` exactly: `mpsc::sync_channel` in both directions, named thread `"editur-git"`, `send()` / `events().try_recv()`.
- `GitState` (on `EditorApp`, like `self.devin`) owns the parsed repository snapshot, commit-message draft, in-flight operation flags, selection, and last refresh error.
- `SourceControlView` (`src/app/source_control_view.rs`, sibling of `devin_view.rs`) renders the pane and translates clicks into controller commands. It never runs `git` itself.

Do not add a VCS trait, a provider abstraction, or a second diff model. There is one VCS and one diff renderer.

The controller starts lazily the first time the Source Control pane becomes visible and stays alive for the app's lifetime (like LSP), since refresh triggers keep arriving while the pane is hidden-but-warm. All mutations are serialized on the single worker thread, which makes stage → refresh races impossible by construction.

### 5.2 Controller commands and events

```text
GitCommand                          GitEvent
  Refresh                             Status(GitStatusSnapshot { generation, .. })
  LoadDiff { path, area }             Diff { path, area, old: Option<String>, new: Option<String>, generation }
  Stage(Vec<PathBuf>)                 OperationFailed { op: &'static str, message: String }
  Unstage(Vec<PathBuf>)               Committed { short_hash: String, subject: String }
  Discard(Vec<PathBuf>)               WorktreeMutated { paths: Vec<PathBuf> }
  Commit { message: String }
  Init
  Shutdown
```

Rules:

- Every mutating command (`Stage`, `Unstage`, `Discard`, `Commit`, `Init`) is automatically followed by a `Refresh` on the worker before the next command is taken.
- `Discard` additionally emits `WorktreeMutated` so the app can run `reconcile_buffer` on affected open tabs immediately instead of waiting for the next window-focus reconcile.
- Each `Refresh` carries a monotonically increasing `generation`. The reducer drops any `Status`/`Diff` event older than the latest applied generation (same stale-response rejection the Devin plan uses).
- Command output is bounded (reuse the 64 KiB `git_text` discipline; raise the diff-content bound to 4 MiB per file). Oversized or non-UTF-8 content degrades to a placeholder, never a panic.

### 5.3 Repository discovery and status model

On first refresh, resolve the repository root with `git -C <workspace_root> rev-parse --show-toplevel`. If it fails, the state is `NoRepository` (the workspace root is remembered so `Init` can run `git init` there). If `git` itself cannot be spawned, the state is `GitUnavailable`. All subsequent commands run with `-C <repo_root>`.

One status call gathers everything:

```text
git status --porcelain=v2 --branch -z
```

Parse into:

- `RepoInfo { branch: BranchInfo, last_commit_subject: Option<String> }` where `BranchInfo` covers a named branch with optional upstream and ahead/behind counts (`# branch.ab`), a detached HEAD (short OID), and the unborn-branch case (`branch.head` `(initial)` — no commits yet). `last_commit_subject` comes from one extra `git log -1 --format=%h%x00%s` call and is skipped on unborn branches.
- `Vec<GitEntry>` where `GitEntry { path, orig_path: Option<PathBuf>, index: Option<ChangeKind>, worktree: Option<ChangeKind> }` and `ChangeKind` is `Modified | Added | Deleted | Renamed | Untracked | Conflicted | TypeChange`. A file staged *and* re-modified appears in both the staged and unstaged groups, as two rows — this matches Git's own model and VS Code's presentation.

Display paths relative to the workspace root; when the repo root is above the workspace root, entries outside the workspace still display (relative to repo root, prefixed `../`) so status never silently hides changes.

### 5.4 Diff sourcing

Diff tabs are read-only and always show exactly what Git sees (disk and index, never unsaved buffer text). The mapping:

| Row | Old text | New text |
| --- | --- | --- |
| Unstaged modified | index: `git show :0:<path>` | worktree file from disk |
| Staged modified/added | `git show HEAD:<path>` (absent for added → `None`) | index: `git show :0:<path>` |
| Untracked | `None` (created-file semantics `AgentDiff` already supports) | worktree file from disk |
| Deleted (unstaged) | index content | empty |
| Deleted (staged) | HEAD content | empty |
| Renamed | old path content on the relevant side | new path content |

Feed the pair into `build_agent_diff` unchanged. Binary or oversized content on either side renders a single placeholder row ("Binary file" / "File too large to diff") instead of a diff body.

If an open tab has unsaved edits to the same path, the diff header shows one muted note: "Unsaved editor changes are not shown." Nothing else changes; keeping the diff consistent with `git status` is worth more than chasing buffer text.

### 5.5 Refresh policy

No filesystem watcher exists and this plan does not add one. Refresh is event-driven:

| Trigger | Hook point |
| --- | --- |
| Source Control pane becomes visible | toggle/command handlers |
| Window regains focus | the existing `WindowEvent::Focused(true)` path in `runtime.rs`, next to `reconcile_open_buffer` |
| Any buffer saved | the save path in `file_io` / commands |
| Agent turn finishes | `refresh_after_agent` (which already resets `git_workspace_status`) |
| Any mutation completes | automatic, on the worker (5.2) |
| Manual refresh button | header |

Triggers coalesce with a 200 ms debounce on the UI side before sending `Refresh`, so a save burst produces one status call. Triggers fire even while the pane is hidden **only if** the controller has already started; before first open, nothing runs. A failed refresh keeps the previous snapshot on screen and sets a footer error line (10.10) — it never clears the lists.

## 6. Application integration

### 6.1 Left-slot pane model

Add to `EditorApp`:

```rust
enum SidebarPane { Files, SourceControl }
sidebar_pane: SidebarPane,   // default Files
```

`sidebar: bool` keeps its meaning: "the left slot is open." `sidebar_width`, the divider, drag-resize clamps, and `sidebar_settings_rect` are shared by both panes untouched. `draw_sidebar` branches on `sidebar_pane` between `draw_tree` and the new `draw_source_control`, keeping the frosted material, macOS titlebar inset, and settings row common.

Agentic mode is unaffected: the left rail there is the sessions rail, not this pane. Toggling Source Control while the agentic view is active first returns to the IDE layout (`set_agentic_mode(false)`), then opens the pane — the same rule the Devin plan set for its sidebar.

### 6.2 Commands

| Command variant | Id | Behavior |
| --- | --- | --- |
| `ViewToggleSourceControl` (new) | `workbench.toggleSourceControl` | Open with `sidebar_pane = SourceControl` if closed or showing Files; close the slot if already showing Source Control. Default chord Cmd/Ctrl+Shift+G via a `builtin("vscode.view.scm", …)` entry. |
| `ViewToggleExplorer` (new) | `workbench.toggleExplorer` | Symmetric pane-aware toggle for Files. The titlebar file-tree button retargets from `ViewToggleSidebar` to this, preserving today's behavior exactly while `sidebar_pane == Files`. |
| `ViewToggleSidebar` (existing, Cmd/Ctrl+B) | `workbench.toggleSidebar` | Unchanged: opens/closes the slot, preserving whichever pane is active. |
| `ViewFocusExplorer` (existing, Cmd/Ctrl+Shift+E) | `workbench.focusExplorer` | Additionally forces `sidebar_pane = Files` before focusing the tree. |

Catalog entries follow the `info!` pattern (`View` category, `GLOBAL` scope). Opening Source Control by command or button also moves keyboard focus into the pane (10.8).

### 6.3 Titlebar button

Insert the button between the terminal and agentic toggles in both `draw_titlebar` and `draw_agentic_titlebar`:

```rust
fn source_control_toggle_rect(terminal_button: egui::Rect) -> egui::Rect {
    terminal_button.translate(egui::vec2(terminal_button.width(), 0.0))
}
```

- `agentic_toggle_rect` is now computed from the source-control button so the Agent/IDE toggle stays to its right.
- The titlebar drag-region math that anchors on `terminal_button.right()` (macOS `sidebar_drag_left`, and the agentic titlebar's equivalent) moves to `source_control_button.right()`.
- Draw with the `draw_terminal_toggle` pattern verbatim: `ui.interact(button, Id::new("source_control_toggle"), Sense::click())`, hover text "Show Source Control" / "Hide Source Control", `icons::paint(…, Icon::SourceControl, …)`, color `theme::text().primary` when hovered or active (active = slot open with the Source Control pane), else `theme::text().muted`.
- Click executes `ViewToggleSourceControl` through `execute_keybinding`, exactly as the neighbors do.

### 6.4 New icon

Add `Icon::SourceControl` to `src/icons.rs` as vector segments on the 16-unit grid: a branch glyph — two `Segment::Circle` node dots joined by a vertical `Segment::Line` on the left, a third node dot upper-right, and a curved/elbow `Segment::Line` connecting the upper-left node to it (the conventional fork shape). Register it in `segments()` and the `ALL` test list. It serves the titlebar toggle and the sidebar branch row. Row-level actions reuse existing icons: `Plus` (stage), `Minus` (unstage), `History` (discard), `File` (open file), `Refresh` (refresh).

### 6.5 Persistence

Match the current chrome policy: none of `sidebar`, `sidebar_width`, or pane choice is persisted today, so `sidebar_pane` is session-only too. Defaults: slot open, `Files`. Do not extend `Settings` for this.

## 7. TDD and verification strategy

Automated tests use fixture strings and temp-dir repositories; no network, no user configuration. Red-green slices:

1. **Porcelain v2 parser** (`src/git/status.rs`): table-driven over fixture byte strings — ordinary modified; staged + re-modified same file (two rows); staged added; deleted staged and unstaged; rename with score; untracked; conflicted (`u` lines); detached HEAD; unborn branch; ahead/behind counts; NUL-separated paths with spaces and non-ASCII.
2. **Snapshot reducer**: full replacement per generation, stale-generation rejection, stable sort (grouped, then path-ordered), pending-operation flag lifecycle around a mutation.
3. **Diff sourcing**: the Section 5.4 mapping from `GitEntry` shape to (old, new) source selection, including the `None` old-side for untracked/added and empty new-side for deleted.
4. **Toggle and pane semantics** (in `src/app/tests.rs`, the existing style): Source Control toggle opens/switches/closes correctly; Cmd+B preserves the active pane; `ViewFocusExplorer` forces Files; the layout gives both panes the identical left-slot rect; the titlebar button order is file-tree, terminal, source-control, agentic.
5. **Controller integration** (one test in `tests/`, real `git` in a `tempfile` repo): init → write file → status shows untracked → stage → commit → status clean, driven entirely through `GitCommand`/`GitEvent`. This is cheap, hermetic, and catches argument/parsing drift better than mocks.
6. **Mutation sequencing**: a mutation is always followed by a fresh snapshot event before any later command's result.

Do not write tests for passive structs, icon path data, or copies of Git's documented output formats beyond what the parser exercises.

Manual smoke on a real repository: stage/unstage/discard from rows and group headers, commit with a hook that fails (error toast, message preserved), diff for each row type in 5.4, discard reloads an open tab, no-repo → Initialize → working sidebar, and the pane at minimum sidebar width.

## 8. Implementation phases

### Phase 1: Git backend

- `src/git/status.rs`: porcelain v2 parser and snapshot types (TDD slice 1).
- `src/git/controller.rs`: worker thread, command/event loop, repo discovery, bounded output, generation counters, mutation → auto-refresh (slices 2, 3, 5, 6).
- Command implementations: `status --porcelain=v2 --branch -z`, `log -1`, `show :0:` / `show HEAD:`, `add --`, `restore --staged --` (with the unborn-branch fallback `rm --cached -r --`), `restore --`, untracked discard via file deletion, `commit --file=-` with the message on stdin, `init`.

Exit: the integration test drives init → stage → commit → clean through the controller with no UI.

### Phase 2: Application shell

- `SidebarPane` enum, `draw_sidebar` branch, `GitState` on `EditorApp`, `poll_git` drain next to `poll_agent`/`poll_devin`.
- `Icon::SourceControl`, titlebar rect chain and drag-region updates, `draw_source_control_toggle`.
- Catalog commands from 6.2 and their handlers, including the file-tree button retarget and agentic-mode exit rule.
- Refresh triggers and debounce from 5.5.
- A deliberately plain pane body: branch line and raw grouped file list, click does nothing yet.

Exit: slice-4 tests pass; the pane toggles from button and keybinding, shows live status, and survives repo/non-repo/git-missing states without panicking.

### Phase 3: Full pane UI

Implement Section 10: header and branch row, commit composer, group headers with bulk actions, styled rows with badges and hover actions, dialogs, toasts, empty states, footer error line.

Exit: Section 10.11 acceptance criteria that don't involve diff tabs pass.

### Phase 4: Diff tabs and reconciliation

- Extract the shared unified-diff tab body from the agent diff view so a Git-sourced tab reuses `draw_agent_diff_body` and `AgentDiffCache` without touching agent session state.
- New tab payload for Git diffs (path, area staged/unstaged, old/new text, read-only); deleted files open as diff-only tabs.
- Row click → `LoadDiff` → open/refresh the tab; re-request on refresh generation change while the tab is visible.
- `WorktreeMutated` → `reconcile_buffer` for affected open tabs (discard immediately reloads clean buffers, flags conflicts on dirty ones exactly like the window-focus path).

Exit: every row type opens the correct diff; discarding a file visibly reloads its open tab.

### Phase 5: Hardening

- Rename, conflict, type-change, and unborn-branch presentation checks.
- Binary/oversized bounding; repos with thousands of entries (single-pass parse, virtualized list if row count is large — the tree's `visible_rows` approach).
- Keyboard and accessibility pass (10.8), minimum-width pass (10.9), reduced-motion check (none needed — this surface has no animation).
- All-platform verification, since titlebar geometry differs on macOS.

Exit: definition of done (Section 11) holds.

## 9. Failure and error policy

- **Refresh failures** (git died, repo vanished): keep the last snapshot rendered, show the footer error line, retry on the next trigger. Never toast, never clear.
- **Mutation failures** (hook rejected commit, index lock, permission): danger `Toast` with the operation name and the first bounded stderr line, sanitized of absolute-home prefixes. State re-converges via the automatic refresh; optimistic UI is limited to disabling the affected row/button while in flight.
- **Commit message is never lost**: a failed commit re-enables the composer with the draft intact.
- Nothing from Git output is written to logs at a level users ship; stderr excerpts live only in the toast/footer.

## 10. UI design plan

### 10.1 The user's job, and the principle that follows

The file tree answers "where is everything"; this pane answers "what have I changed and am I ready to commit it." The user flips to it in short bursts between edits. Therefore the pane is **status-first**: branch identity and the changes list dominate; the commit composer sits directly above the list so the read-review-commit loop happens top to bottom without navigation. There are no views to push or pop — everything is one scrollable surface, matching the tree it replaces.

### 10.2 Placement and slot behavior

Same slot, same geometry as the file tree: left dock, shared `sidebar_width` (default 248), `SIDEBAR_MIN_WIDTH` floor, 500 max, existing 5px divider and resize cursor. The settings row stays pinned at the bottom of the slot in both panes. Switching panes preserves scroll position, selection, and the commit draft of the hidden pane for the session.

### 10.3 Pane anatomy (top to bottom)

1. **Header row** (height `chrome::HEADER`, below the macOS titlebar inset): leading `Icon::SourceControl` at grid size, then the branch name in `small_strong` primary. Trailing, right-aligned: ahead/behind counts as `↑2 ↓1` in `micro` muted (omitted when zero or no upstream), then a `Refresh` icon button (`icons::button_sized`, secondary tone). Hover text on the branch name gives the upstream ref ("main → origin/main"); detached HEAD shows the short OID with hover "Detached HEAD"; an unborn branch shows the branch name with hover "No commits yet". Branch name middle-truncates under pressure; the counts and refresh button never yield.
2. **Commit composer**: a frameless multiline `TextEdit` (id `git_commit_message`) inside a `surface().input` well with `radius::CONTROL` — the find-bar input treatment. Hint text "Commit message". Two rows tall, growing to six before scrolling internally. Beneath it a full-width Commit button at `control::STANDARD` height: accent fill with `on_accent` text when enabled; muted otherwise with hover text explaining why ("Stage changes to commit" / "Enter a commit message"). Enabled only when staged count > 0 and the message is non-blank. Enter inserts a newline (commit messages are multiline); **Cmd/Ctrl+Enter commits** from inside the field. While a commit is in flight the button shows "Committing…" and disables; success clears the draft and pushes a neutral toast "Committed a1b2c3d — subject", middle-truncated.
3. **Changes area** (egui `ScrollArea`, the Devin-list approach): groups in fixed order, each rendered only when non-empty —
   - `MERGE CHANGES (n)` — conflicted entries, pinned first.
   - `STAGED CHANGES (n)` — trailing group action: `Minus` icon button, "Unstage all".
   - `CHANGES (n)` — worktree entries including untracked; trailing actions: `Plus` ("Stage all") and `History` ("Discard all changes…", danger-toned on hover).
   Group labels are uppercase `micro` strong muted (the settings-section-label voice) with the count in plain muted; labels scroll with content.
4. **Footer error line** (only when the last refresh failed): one `micro` muted line above the settings row — "Couldn't refresh status — retrying". This is the only surface for refresh trouble (Section 9).

### 10.4 Row anatomy

One line, height `theme::control::row()`, hover/selection via `theme::state::fill(selected, focused, hovered, false)` with `radius::ROW` — identical feel to tree rows.

- **Leading badge**: a fixed 16px slot with a single `micro` strong letter, colored per 10.6. This replaces a file icon; the letter is the icon.
- **Name**: file name in `small` primary; deleted entries render the name with strikethrough. Renames show the new name with hover text "old → new".
- **Path**: the parent directory in `small` muted after the name, middle-truncated first when space runs out. Hover text always carries the full relative path.
- **Hover actions**, right-aligned, revealed on row hover or row focus, painted over a solid row-fill patch so they stay legible over truncated text:
  - Changes group: `File` (Open file) · `History` (Discard…) · `Plus` (Stage).
  - Staged group: `File` (Open file) · `Minus` (Unstage).
  - Merge group: `File` (Open file) · `Plus` (Mark resolved — stages the file).
  Each is an `icons::button_sized` at `GRID`-based size with hover text; discard uses the danger tone on hover.
- **Click** anywhere else on the row opens the diff tab (10.5). The whole row is the hit target; actions are the exception islands.
- A row with an operation in flight dims to `text_disabled()` and ignores input until the next snapshot.

### 10.5 Diff tabs

Row activation opens a read-only unified diff tab in the active pane, titled with the file name and a muted qualifier — "name (Working Tree)" or "name (Staged)". Rendering reuses the agent diff body: same line numbers, `+`/`−` signs, `theme::diff` washes, context compaction, and scroll behavior. The tab is keyed by (path, area) so re-clicking focuses the existing tab; a newer status generation re-requests content in place. Deleted files open as diff-only tabs (there is no file to edit). The unsaved-buffer note from 5.4 renders as one muted line under the tab header when applicable. No staging controls live inside the diff tab in this release.

### 10.6 Status vocabulary

| Badge | Meaning | Color token |
| --- | --- | --- |
| `M` | Modified | `semantic().warning` |
| `A` | Added (staged new file) | `semantic().success` |
| `U` | Untracked | `semantic().success` |
| `D` | Deleted | `semantic().danger` |
| `R` | Renamed | `semantic().info` |
| `T` | Type changed | `semantic().info` |
| `!` | Conflicted | `semantic().danger` |

Badge letters carry hover text with the long word. Color never carries meaning alone — the letter, hover text, and (for deletes) strikethrough convey the same state. No animation exists anywhere on this surface, so reduced-motion needs no special case.

### 10.7 Destructive actions

| Action | Confirmation | Wording rules |
| --- | --- | --- |
| Discard one tracked file | `Dialog`, danger severity, destructive button "Discard Changes" | Body names the file via `dialog.path(...)`; states changes are restored from the index and cannot be undone |
| Discard one untracked file | Same dialog, body switches to "This file is untracked. Discarding **deletes it permanently**." | Delete-styled wording is mandatory — this removes a file, not edits |
| Discard all (group header) | Same dialog; body gives counts: "Discard changes in 4 files? 2 untracked files will be deleted permanently." | Counts computed from the live snapshot |
| Stage / Unstage / Commit | None — reversible or intentional | Feedback via list update and (commit only) toast |

Escape cancels; Enter never triggers the destructive button (existing `Dialog` keyboard contract).

### 10.8 Keyboard, focus, and accessibility

- New `Scope::SourceControl`, active when focus is on `git_commit_message` or the changes list (a `scm_focused` flag mirroring `tree_focused`), registered in `active_keybinding_scopes` at the same priority tier as `FilesTree`.
- Opening the pane focuses the **commit message field** (the highest-intent target; the list is one Tab away). `ViewToggleSourceControl` on an already-open pane closes it, per 6.2.
- List navigation (direct-key handling like `tree_keyboard`): Up/Down move across group boundaries; Enter opens the diff; **Space** stages/unstages the focused row (context-dependent); Delete/Backspace opens the discard dialog for the focused Changes row. Escape from the list returns focus to the editor, matching tree behavior.
- Every interactive element sets accessible widget info: rows announce "status word, file name, directory"; group actions and hover actions carry their hover-text labels; the commit button announces its disabled reason.
- At `SIDEBAR_MIN_WIDTH`: the header keeps icon + truncated branch + refresh (counts drop first); the composer and button stay full-width; rows keep badge + truncated name and drop the directory; hover actions still fit because they overlay. No horizontal overflow at any width.

### 10.9 Empty and edge states

| State | Entered when | Visible UI |
| --- | --- | --- |
| Git unavailable | `git` cannot be spawned | Centered muted sentence: "Git isn't available on this machine." Nothing else |
| No repository | `rev-parse` fails | Centered empty state: one sentence + "Initialize Repository" button (runs `Init`, then refreshes into the normal pane) |
| Unborn branch | Repo with no commits | Normal pane; header hover "No commits yet"; commit works and creates the first commit |
| Clean | No entries | Header + composer remain; list area shows centered muted "No changes" with the last commit as a second `micro` line: "Last commit: a1b2c3d subject…" |
| Refreshing (first load) | Controller started, no snapshot yet | Header renders immediately from nothing ("…" branch placeholder), list area shows a centered muted "Loading…" — no skeletons for a local sub-second call |
| Refresh failed | Non-first refresh errored | Previous snapshot intact + footer error line |
| Operation in flight | Mutation sent | Affected row/button disabled-dimmed; everything else live |

Transitions never discard the commit draft, list scroll, or selection.

### 10.10 Component-reuse decision

| Reuse as-is | Adapt | New | Explicitly not reused |
| --- | --- | --- | --- |
| Theme tokens (`surface`, `semantic`, `state`, metrics, typography) | Left-slot chrome from `draw_sidebar` (shared branch point) | `Icon::SourceControl` glyph | `AgentState.changed_paths` / `FileChange` (agent-turn semantics) |
| `Dialog` (danger discard), `Toasts` (commit/mutation results) | Terminal-toggle titlebar pattern → source-control toggle | Status badge letters | `agent_path_link`-style linking of diff rows into agent state |
| `icons::button_sized`, `selectable_content_row`-style row fills | Agent diff view body → shared read-only diff tab | Commit composer + button block | Devin polling/freshness machinery (git is trigger-driven, not polled) |
| egui `ScrollArea` (Devin list precedent) | `tree_keyboard` pattern → list navigation | Porcelain v2 parser, `GitController` | `TreeSurface` file icons (badges replace them) |
| Find-bar input well treatment for the composer | `git_text` bounded-subprocess helper | Group headers with bulk actions | Any `git2`/`gix` dependency |

### 10.11 Wireframes

Pane at the default 248px width, changes present:

```text
┌──────────────────────────────┐
│ ⎇ main            ↑2 ↓1   ⟳ │  header: branch · counts · refresh
├──────────────────────────────┤
│ ┌──────────────────────────┐ │
│ │ Commit message…          │ │  composer (input well)
│ └──────────────────────────┘ │
│ [        Commit            ] │  accent when enabled
├──────────────────────────────┤
│ STAGED CHANGES (2)        ⊖ │  group + unstage-all
│  M  app.rs        src/       │
│  A  status.rs     src/git/   │
│ CHANGES (3)            ↺  ⊕ │  group + discard-all · stage-all
│  M  layout.rs     src/app/   │
│  D  o̶l̶d̶_̶v̶i̶e̶w̶.̶r̶s̶  src/app/   │
│  U  notes.md                 │
├──────────────────────────────┤
│ ⚙ Settings                   │  existing settings row
└──────────────────────────────┘
```

Row under hover, actions revealed over the trailing edge:

```text
│  M  layout.rs     s… 📄 ↺ ⊕ │  open · discard · stage
```

Clean repository:

```text
┌──────────────────────────────┐
│ ⎇ main                    ⟳ │
├──────────────────────────────┤
│ ┌──────────────────────────┐ │
│ │ Commit message…          │ │
│ └──────────────────────────┘ │
│ [        Commit            ] │  disabled, muted
├──────────────────────────────┤
│                              │
│         No changes           │
│  Last commit: a1b2c3d Fix …  │
│                              │
├──────────────────────────────┤
│ ⚙ Settings                   │
└──────────────────────────────┘
```

Titlebar cluster after the change (macOS, left of center):

```text
[▤ files] [>_ terminal] [⎇ source control] [◇ agent/ide]
```

### 10.12 UI acceptance criteria

1. The titlebar shows the source-control button immediately right of the terminal button in both the normal and agentic titlebars, with correct drag regions on macOS.
2. Toggling Source Control replaces the file tree in place — same rect, same width, same divider — and toggling Files restores the tree with its previous scroll and selection.
3. Cmd/Ctrl+B closes and reopens the slot preserving the active pane; Cmd/Ctrl+Shift+E always lands on the Files tree.
4. Cmd/Ctrl+Enter commits from the message field; plain Enter inserts a newline; the commit button is enabled only per 10.3 and its disabled state explains itself on hover.
5. Stage, unstage, and discard work from row hover actions, group headers, and the keyboard (Space / Delete); discard is impossible without the danger dialog, and untracked discard wording says the file will be deleted.
6. Each row type in Section 5.4 opens the correct diff pairing, rendered with the existing diff visual language, read-only, titled with its area qualifier.
7. Discarding a file that is open in a tab reloads that tab (or flags a conflict if the buffer was dirty) without requiring a window refocus.
8. A failed refresh changes only the footer line; a failed commit toasts and preserves the message draft.
9. A staged-and-re-modified file appears in both groups and each row diffs its own area.
10. The no-repo state offers Initialize Repository and transitions to a working pane without restart.
11. At `SIDEBAR_MIN_WIDTH` every element renders without horizontal overflow, and all rows/actions/dialogs are keyboard-reachable with accessible labels that convey status as text.
12. Agent-session changed-file cards and diffs behave exactly as before — no shared state was touched.

## 11. Definition of done

- The Source Control pane opens from the titlebar button beside the terminal button and from `workbench.toggleSourceControl`, replacing the file tree in the left slot.
- Status, staging, unstaging, discarding, and committing work against a real repository entirely off the UI thread.
- Diff tabs render every entry type correctly through the shared unified-diff body.
- The file tree, terminal, agent, Devin, and agentic surfaces are byte-for-byte unaffected when the pane is never opened.
- Refresh triggers fire per Section 5.5 with no watcher and no polling loop.
- All Section 7 automated slices and the manual smoke pass; Section 10.12 criteria hold.
- No new crate dependencies beyond what testing requires (`tempfile` if not already present).

## 12. Deferred follow-ons

Add only after real usage demands them:

- Branch menu: switch, create, and publish branches from the header row.
- Push/pull/sync actions with ahead/behind integration.
- Hunk-level staging and inline stage controls in the diff tab.
- Editor gutter change indicators sourced from the same controller.
- Commit history / file log view and stash management.
- A change-count badge on the titlebar toggle (requires background refresh policy decisions first, same reasoning as the Devin plan's hidden-state rule).
- Multi-root and submodule support.
