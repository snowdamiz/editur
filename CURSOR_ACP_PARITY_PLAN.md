# Cursor ACP parity plan

## 1. Purpose

Close the gaps between what Cursor's ACP surface (`agent acp`, [docs](https://cursor.com/docs/cli/acp)) can deliver and what Editur's agent sidebar captures and renders. This follows an audit of `src/agent/controller.rs`, `src/agent/state.rs`, and the agent UI in `src/app.rs` against the official Cursor ACP documentation (verified 2026-08-11).

## 2. Audit result: what already works

Everything below is implemented and verified in code; no action needed.

| Capability | Status |
| --- | --- |
| Core flow: `initialize`, `authenticate`, `session/new`, `session/load`, `session/list`, `session/prompt`, `session/cancel`, `session/set_mode`, `session/set_config_option` | Handled |
| `session/update`: user/agent/thought chunks, `plan`, `tool_call`, `tool_call_update`, `usage_update`, `current_mode_update`, `config_option_update`, `available_commands_update`, `session_info_update` (title) | Handled, rendered |
| `session/request_permission` with allow-once / allow-always / reject options, Run Everything auto-allow | Handled, rendered, one-decision enforcement |
| `cursor/ask_question` (blocking) incl. multi-select questions, answered/skipped/cancelled outcomes | Handled, rendered |
| `cursor/create_plan` (blocking) incl. name, overview, todos, phases, `isProject`; accept/reject/cancelled | Handled, rendered |
| `cursor/update_todos` incl. `merge` flag | Handled, rendered |
| `cursor/task` (subagent) description, prompt, string `subagentType`, model, agentId, durationMs | Handled, rendered as a tool card with metadata |
| `cursor/generate_image` with `filePath` present | Handled, rendered as text card |
| Modes, config options (select + boolean), slash commands, usage/cost, session titles, session history with local hide list | Handled, rendered |
| Diff tool content with inline diff view; syntax-highlighted text output; markdown assistant text; thought collapse; attachments (image/audio/context per advertised prompt capabilities) | Rendered |
| Transcript bounds, truncation marker, tool finalization on turn end, reconnect-after-transport-drop resume | Implemented |

## 3. Gaps

### 3.1 Correctness bugs against the documented Cursor schema

These break capabilities Cursor already ships:

- **G1 — custom subagent types fail to parse.** Docs define `subagentType` as `"unspecified" | "explore" | ... | { custom: string }`. `CursorTaskUpdate.subagent_type` is `String` (`controller.rs:2638`), so a `{ "custom": "my_type" }` payload fails serde and the whole `cursor/task` notification degrades into an error card instead of a subagent card.
- **G2 — `cursor/generate_image` without `filePath` fails to parse.** Docs mark `filePath` optional; `CursorImageUpdate.file_path` is a required `PathBuf` (`controller.rs:2652`). A payload without it becomes a parse-error card.
- **G3 — no automated coverage for `cursor/task` or `cursor/generate_image`.** The fake agent only exercises `cursor/ask_question` and `cursor/update_todos`; `cursor/create_plan` has one unit test. G1/G2 shipped because nothing exercises these paths.

### 3.2 Subagent visibility (the "click into the subagent" ask)

Protocol reality: `cursor/task` is a fire-and-forget notification tied to a `toolCallId`; **Cursor does not stream a nested subagent transcript over ACP**. Live progress exists only as the parent Task tool call's `tool_call_update` status stream, which Editur already merges into the same card. Within that constraint:

- **G4 — the subagent card is flat and buried.** Description, prompt, type, model, agentId, and duration render as plain labels inside the parent tool card (`app.rs:10082–10111`). The prompt (often multi-paragraph) is always fully expanded, there is no dedicated collapsed "Subagent" presentation, no running/finished distinction beyond the generic tool status, and `agent_id` is inert text.
- **G5 — no drill-down experiment.** The docs say `agentId` can "resume a previously created subagent". Whether `session/load` accepts a subagent `agentId` (which would let a user open the subagent's transcript read-only) is undocumented and unverified.

### 3.3 Data the controller discards that the UI could use

- **G6 — image/audio/blob payloads stripped.** `normalize_display_content` keeps only byte counts (`controller.rs:2762–2793`), so `DisplayContent::Image` renders as "Image · png · N bytes" and can never show pixels.
- **G7 — generated images not previewed.** `GeneratedImage.file_path` points at a real local file, but the UI shows the path as text (`app.rs:10113–10142`).
- **G8 — `ToolCall.kind` dropped.** Never mapped into `ToolActivity`, so the UI cannot show per-kind icons (read/edit/execute/search/task) or detect Task tool calls independently of the `cursor/task` notification.
- **G9 — `ToolCallLocation.line` dropped.** Paths are captured, line numbers are not, so "jump to the exact spot the agent touched" is impossible.
- **G10 — embedded terminal content is an id label.** `ToolCallContent::Terminal` keeps only the terminal id; `ClientCapabilities.terminal` is not advertised and no `terminal/*` methods exist, so live command output can never appear. UI shows `Terminal {id}` (`app.rs:10067`).
- **G11 — `PlanEntry.priority` dropped** (minor; affects plan rendering fidelity only).

### 3.4 UI interaction stubs

- **G12 — no path is clickable.** Tool paths, diff paths, `GeneratedImage` paths, and `ResourceLink` URIs are display-only; opening the touched file in the editor requires manual navigation.
- **G13 — changed files are invisible.** `changed_paths` drives tree/search refresh but there is no "files changed this session" list a user can review or click.
- **G14 — permission cards are mouse-only** and their `tool_call_id` is never linked back to the originating tool card.

### 3.5 Explicit non-gaps (deliberate, keep as-is)

- `fs/read_text_file` / `fs/write_text_file` are not advertised: Editur requires a saved buffer before every prompt, so the agent always sees disk truth. Revisit only if unsaved-buffer awareness becomes a product goal.
- Unknown `session/update` variants and unknown `cursor/*` methods are silently ignored — required by the additive-protocol rule in `ACP_AGENT_PLAN.md` §5.
- MCP-over-ACP, session delete/fork, ACP v2 draft features: out of scope per `ACP_AGENT_PLAN.md` §2.

## 4. Implementation phases (TDD, smallest first)

### Phase 1: Schema correctness (G1, G2, G3)

1. Failing tests first: unit tests for `normalize_cursor_notification` with (a) `subagentType: { "custom": "reviewer" }`, (b) each documented string variant, (c) `cursor/generate_image` without `filePath`; extend `editur-fake-agent` with `cursor-task` and `cursor-image` prompt scenarios and controller integration tests mirroring the existing `cursor-notification` test.
2. Fix: `subagent_type` becomes an untagged enum (`Named(String) | Custom { custom: String }`) normalized to a display string; `file_path` becomes `Option<PathBuf>`; `ToolOutput::GeneratedImage.file_path` becomes optional with a "no path supplied" UI fallback.

Exit: all five Cursor extension methods have fake-agent round-trip coverage and no documented payload shape produces an error card.

### Phase 2: Subagent presentation (G4, G8)

1. Map `ToolCall.kind` into `ToolActivity` (state test: kind survives updates that omit it).
2. Render Task cards distinctly: subagent icon/badge from kind or `cursor/task`, collapsed-by-default prompt (`CollapsingHeader`, like Thinking), metadata line keeps type · model · duration; show live "Running" while the parent tool call is in progress and duration once finished.
3. UI shape-level test in the existing `app.rs` test style (assert the subagent header text and collapsed prompt exist).

Exit: a subagent is visually distinct, its prompt is one click away, and its running/finished state is obvious.

### Phase 3: Subagent drill-down spike (G5)

Manual spike against a real `agent acp` (no CI dependency, same rules as `ACP_AGENT_PLAN.md` §4): record whether `session/load` with a `cursor/task` `agentId` returns that subagent's transcript.

- If yes: add a "View transcript" button on the Task card that opens the subagent read-only through the existing session-load path (tests via fake agent replaying a nested session).
- If no: document the limitation in this file and stop; do not build speculative UI.

**Spike result (2026-08-11, `cursor-agent 2026.08.04-aaa8809`): NO.** A scripted ACP session forced one Task subagent; `cursor/task` arrived with `agentId: "83634e95-…"` and `durationMs`, but `session/load` with that agentId returned `-32602 Invalid params: Session "83634e95-…" not found`, while `session/load` with the parent session id succeeded as a control. Subagent transcripts are not addressable over ACP; no drill-down UI is built.

Additional findings from the same spike:

- The live agent sends `subagentType` as `{"custom": {"unspecified": {}}}` — `custom` carries an **object**, not the documented string. The parser now accepts string variants, `{custom: string}`, and `{custom: {name: {}}}` (normalized to the object's key).
- The parent Task tool call arrives with `kind: "other"`, so Task detection must rely on the `cursor/task` notification content (which Editur does).

### Phase 4: Local artifacts made visible (G7, G12, G13)

1. Clickable paths: factor one helper that turns a workspace path label into an open-in-editor action; apply to tool paths, diff headers, and generated-image paths. Test the action routing, not egui layout.
2. Inline preview for `GeneratedImage` when the file exists (bounded size, load off the UI thread or on first expand; egui image support already ships for attachments thumbnails).
3. A compact "Changed files" section (collapsed list fed by `changed_paths`, each row clickable via the same helper).

Exit: everything the agent produced on disk is reachable in one click from the transcript.

### Phase 5: Rich content payloads (G6) — measure first

Carrying image bytes through `DisplayContent` violates the current "counts only" bounding on purpose. Before implementing: cap per-image bytes (e.g. 2 MiB) and total retained media per transcript, drop media first in `trim()`, then render inline images. Skip audio playback; keep the placeholder. If Cursor never sends inline image content in practice (verify during the Phase 3 spike), close this as not-needed instead of building it.

**Decision (2026-08-11): closed as not-needed.** During the Phase 3 spike every `session/update` content block was `type: "text"`; no inline image or audio content appeared. Generated images arrive as `cursor/generate_image` file paths, which Phase 4 previews from disk. Revisit only if a real transcript ever shows a non-text content block (the byte-count placeholder makes that visible).

### Phase 6: Terminal visibility (G10) — decide, don't drift

Advertising `terminal: true` means implementing `terminal/create`, `terminal/output`, `terminal/wait_for_exit`, `terminal/kill`, `terminal/release` and owning real PTYs on three platforms. During the Phase 3 spike, record whether Cursor actually uses client terminals when advertised and whether embedded `Terminal` content appears without it. Only if it does, and command output is otherwise invisible, plan a separate `ACP_TERMINAL_PLAN.md`; until then the id label plus `rawOutput` text remains acceptable.

**Decision (2026-08-11): keep as-is, no terminal plan.** With `terminal: false` advertised, the spike session produced no embedded `terminal` content blocks; command output continues to arrive as `rawOutput` text, which Editur already renders. Do not advertise `terminal: true` until a real transcript shows output that is otherwise invisible.

### Deferred without a trigger

- G9 (location line numbers): fold into Phase 4's clickable-path helper if the ACP crate exposes lines; otherwise skip.
- G11 (plan priority), G14 (permission keyboard shortcuts, tool-card linking): batch into a later polish pass; none block a Cursor capability.

## 5. Verification checklist

- [x] `{ custom: … }` subagent types and missing image paths render as cards, not errors.
- [x] Fake-agent scenarios exist for all five `cursor/*` extension methods.
- [x] Subagent cards are distinct, collapsed by default, and show live status.
- [x] `session/load`-on-`agentId` spike result recorded in this file.
- [x] Generated images preview inline and open on click.
- [x] Tool/diff/changed paths open the file in the editor (tool paths jump to the reported line).
- [x] `cargo test`, `cargo fmt --check`, `cargo clippy -- -D warnings` pass; incremental build artifacts cleaned if disk usage grew.
