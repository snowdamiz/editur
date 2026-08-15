# Devin cloud sidebar implementation plan

## 1. Purpose and reader

This plan is for the engineer adding cloud Devin supervision to Editur. After reading it, they should be able to validate Devin's supported integration surface, implement the remote controller and state model, wire an independently toggled sidebar into the application, and hand the remaining interface design to a UI-focused contributor.

This is not a fourth ACP provider. Cursor, Codex, and Claude remain local provider choices inside the existing Agent experience. Devin cloud sessions have their own lifecycle, remote workspace, authentication, activity model, and dedicated sidebar.

## 2. Product contract

The integration must:

- Add a dedicated Devin sidebar that is separate from the existing Agent sidebar and provider selector.
- Add a dedicated show/hide action for the Devin sidebar.
- Keep Devin visibility, selected session, transcript, and controller state independent from the local Agent state.
- List cloud Devin sessions visible to the authenticated user, including sessions started from supported origins such as Slack.
- Create a Devin session for a selected repository and prompt.
- Load a session's messages, status, recent activity, attachments, pull requests, and child-session metadata when available.
- Let the user send follow-up messages and invoke supported lifecycle controls.
- Stop polling when the Devin surface is not in use without stopping the remote Devin session.
- Treat Devin file, shell, browser, and Git activity as remote activity. It must not enter Editur's local buffer reconciliation or changed-file state.
- Keep credentials out of source, preferences, transcripts, diagnostics, and logs.

The detailed interface design is recorded in Section 10.

## 3. Non-goals

The first release will not include:

- Devin as a `ProviderId` or a choice in the existing Agent provider selector.
- Slack API, Slack OAuth, or a dependency on Devin's Slack application.
- An embedded Devin desktop, IDE, browser, or interactive remote shell.
- Automatic checkout, merging, or application of a Devin pull request.
- Automatic upload of uncommitted local changes.
- Automatic synchronization between a Devin remote workspace and local Editur buffers.
- Service-user impersonation, organization administration, or broad enterprise permissions.
- Batch creation, a managed-agent tree, or a full child-session orchestration interface.
- The local `devin acp` CLI integration. That is a separate local-agent feature and does not supervise cloud Devin sessions.
- Undocumented or insider-only Devin endpoints and commands.

## 4. Verified Devin integration surface

The supported foundation is Devin's authenticated MCP server at `https://mcp.devin.ai/mcp`. It uses Streamable HTTP and provides tools for creating, searching, inspecting, and controlling cloud sessions. See the [Devin MCP documentation](https://docs.devin.ai/work-with-devin/devin-mcp).

The relevant MCP tools are:

| Tool | Editur use |
| --- | --- |
| `devin_session_search` | List and filter sessions, including origin, tags, repository, parent, user, and time filters. |
| `devin_session_create` | Start one or more sessions with a prompt, repository, tags, mode, playbook, and optional ACU limit. |
| `devin_session_interact` | Read session state, send a message, sleep, terminate, archive, unarchive, and retrieve messages or attachments. |
| `devin_session_events` | Read or search shell, file, browser, MCP, Git, message, status, todo, recording, and lifecycle events. |
| `devin_session_gather` | Wait for one or more sessions to settle. It is not required for the interactive sidebar loop. |

Devin's organization REST API also supports session listing, creation, messages, termination, and archival. It remains a fallback if MCP event retrieval proves unusable, not a second protocol to maintain in the first implementation. The current API is v3; v1 and v2 are legacy. See the [API overview](https://docs.devin.ai/api-reference/overview) and [common API flows](https://docs.devin.ai/api-reference/common-flows).

Devin's Slack application starts a cloud session when a linked user mentions `@Devin` and continues the conversation in the Slack thread. Editur should discover those sessions through Devin session search rather than integrate with Slack directly. See the [Slack integration documentation](https://docs.devin.ai/integrations/slack).

The public flows are request/response and polling based. No documented push feed is required by this plan. A reference to an ACP live WebSocket exists in authentication documentation, but no supported public endpoint contract was found; do not build against it.

## 5. Architecture

```text
Application command routing
        |
        +---- existing Agent sidebar and local ACP controller
        |
        +---- dedicated Devin sidebar
                    |
                    v
              DevinState
                    ^
                    |
              DevinController
                    |
                    v
          https://mcp.devin.ai/mcp
```

### 5.1 Ownership boundary

Add three concrete pieces:

- `DevinController` owns authentication headers, MCP requests, polling, cursor progression, backoff, cancellation, and background-thread lifetime.
- `DevinState` owns the session summary list, selected session, messages, activity, attachments, pull requests, child-session metadata, transient errors, and connection state.
- `DevinSidebar` renders the dedicated surface and translates user intent into controller commands.

Do not add a generic remote-agent trait or controller factory. There is one remote integration. Reuse the existing background command/event channel pattern, but keep the Devin command, event, and state types separate because cloud session semantics do not match ACP semantics.

The controller must never perform network work on the UI thread. It starts lazily when the Devin sidebar is first shown. Hiding the sidebar stops or idles polling; it does not send sleep, terminate, or archive to Devin.

### 5.2 Controller commands

The initial command set should cover:

- Refresh the session list.
- Select and load a session.
- Load more messages or events using returned cursors.
- Create a session.
- Send a message or attachment reference.
- Refresh session status.
- Sleep a session.
- Archive or unarchive a session.
- Terminate a session only after the UI has obtained explicit confirmation.
- Shut down the local controller thread.

Do not map the existing Agent `Cancel` action to Devin termination. Local turn cancellation and remote session termination have materially different consequences.

### 5.3 State and event normalization

Preserve upstream identifiers and timestamps so repeated polling is idempotent. The state reducer should deduplicate messages by event or message identity and activity by event identity before appending them in chronological order.

Keep Devin's raw status and `status_detail` available to the UI layer. The backend may expose stable semantic categories such as active, waiting, sleeping, completed, and failed, but the UI pass owns their wording and presentation.

Normalize remote activity into a small data model containing:

- Stable event identity and timestamp.
- Event category and summary.
- Optional bounded details.
- Optional associated path, command, URL, attachment, pull request, or child-session identifier.

Do not translate remote file events into local paths or local file-change records.

### 5.4 Transport

Use the existing synchronous HTTP dependency and JSON support from a background thread for the protocol spike. Implement only the MCP initialization and tool-call response handling required by the five documented Devin tools. Bound response sizes and handle both JSON and SSE-formatted Streamable HTTP responses.

If the spike proves that correct transport handling requires persistent MCP sessions, server requests, notifications, or substantial SSE machinery, replace the small transport with the official Rust MCP SDK. Do not add an async runtime and a second HTTP stack before that need is demonstrated.

Keep the production endpoint fixed to Devin's documented MCP URL. Do not add arbitrary endpoint configuration in the first release.

### 5.5 Polling

Polling is active only while the Devin sidebar is visible or another explicitly designed background-notification mode is enabled later.

Initial operating values:

- Refresh the selected active session every 3–5 seconds.
- Refresh the session summary list every 15–30 seconds.
- Stop frequent polling for terminal sessions.
- Use returned cursors or event identifiers to request and append only new material.
- Add exponential backoff with jitter after rate limits and transient failures.
- Respect server retry guidance when provided.
- Resume with an immediate refresh when the sidebar becomes visible again.

These values are calibration knobs, not UI behavior. Adjust them after measuring request volume and perceived staleness.

## 6. Authentication and authorization

For the protocol spike and developer builds, read `DEVIN_API_KEY` and, when required, `DEVIN_ORG_ID` from the process environment. Do not print either value. Before a user-facing release, store user-entered credentials in the operating system credential store rather than Editur preferences.

Both service-user keys and personal access tokens use the `cog_` prefix. Personal access tokens represent the human user and are the appropriate default for a local desktop tool; enterprise policy may disable or require approval for them. Organization-scoped service-user keys are appropriate for a centrally managed deployment. See [Devin authentication](https://docs.devin.ai/api-reference/authentication) and [personal access tokens](https://docs.devin.ai/api-reference/personal-access-tokens).

Request only the permissions required by the enabled features:

- `ViewOrgSessions` for listing, details, messages, and events.
- `ManageOrgSessions` for messages and lifecycle controls.
- `UseDevinSessions` for creation.

Do not request impersonation or administrative permissions by default. See the [v3 permission model](https://docs.devin.ai/api-reference/v3/overview).

Authentication errors exposed to the UI must be sanitized. Raw response bodies, request headers, attachment URLs containing credentials, and unbounded event details must not enter normal logs.

## 7. Repository and local-workspace boundary

When creating a session, derive the candidate repository from the current project's `origin` remote and normalize common HTTPS and SSH GitHub forms to `owner/repository`. If the remote is absent, ambiguous, or unavailable to Devin, return a state that lets the future UI request an explicit repository selection instead of guessing.

A cloud Devin works from its remote clone. It cannot see unsaved buffers, uncommitted files, or commits that have not been pushed. The first release must state this boundary wherever the future UI design determines it is necessary.

Do not send `git diff` automatically. Devin handoff can transmit uncommitted changes, but diffs may contain secrets and require a separate preview, size limit, redaction warning, and explicit confirmation flow. See the [Devin handoff documentation](https://docs.devin.ai/work-with-devin/devin-handoff).

Sessions created by Editur should receive one stable `editur` tag. Do not rely exclusively on that tag when listing sessions, because sessions created in Slack, the Devin web app, or another client still need to be discoverable.

## 8. TDD and verification strategy

Use one fake MCP transport or local fake HTTP server with sanitized response fixtures. No real Devin credentials or network access belong in automated tests.

Implement in red-green slices:

1. Search response parsing and session-list replacement.
2. Message and event pagination with identity-based deduplication.
3. Session selection and stale-response rejection.
4. Message sending and lifecycle command result handling.
5. Poll scheduling, visibility pause, backoff, and shutdown.
6. Credential redaction and bounded-response failures.
7. Dedicated sidebar command routing without changing local Agent state.

Prefer one table-driven or end-to-end reducer test per behavior cluster. Do not write tests for passive fields or external schema copies that contain no application logic.

A manual authenticated smoke test must prove:

- An Editur-created session appears and opens in Devin's web application.
- A Slack-origin session visible to the same user appears in search.
- New messages and representative activity events arrive without duplication.
- A message sent from Editur appears in the Devin session.
- Sleep, archive, unarchive, and terminate produce the expected remote state.
- Hiding the sidebar stops polling while the remote session continues.
- No token or authorization header appears in logs or diagnostics.

## 9. Implementation phases

### Phase 0: authenticated protocol spike

- Initialize against the official MCP endpoint.
- Record the exact sanitized schemas and responses returned by session search, interact, events, create, and gather.
- Exercise JSON and SSE response handling.
- Verify cursor behavior, status transitions, attachment and pull-request shapes, Slack origin filtering, and child-session metadata.
- Verify `401`, `403`, `429`, timeout, malformed, and oversized responses.
- Decide whether the existing HTTP dependency is sufficient or the official MCP SDK is required.

Exit condition: an internal command-line or test harness can list, inspect, message, create, sleep, and archive a real session without exposing credentials.

### Phase 1: controller and state vertical slice

- Implement the concrete Devin controller, commands, events, and state reducer.
- Add lazy startup, polling, cursor progression, deduplication, backoff, and clean shutdown.
- Add environment-based authentication for development.
- Parse the current Git origin for session creation.
- Map messages, activity, attachments, pull requests, usage, and lifecycle state without touching local buffer state.

Exit condition: tests can drive a complete create → inspect → message → sleep/archive flow through the fake transport.

### Phase 2: application and sidebar shell

- Add independent Devin sidebar visibility and state to the application.
- Add a stable dedicated show/hide command that can be bound through the existing keybinding system.
- Connect application lifecycle and repaint signals to the Devin controller.
- Ensure opening, hiding, or closing the Devin sidebar does not start, stop, switch, or reset a local ACP provider.
- Provide the UI layer with all supported commands and state, without deciding the final layout.

Exit condition: a deliberately plain internal sidebar can be shown and hidden, select a session, render raw normalized state, and invoke controller commands. It is a development shell, not the final interface.

### Phase 3: UI design and implementation

Implement the design recorded in Section 10. The implementation may reuse existing Agent visual components, but it must not merge ownership or lifecycle with the Agent sidebar.

Exit condition: the dedicated Devin sidebar satisfies the approved design states and accessibility checks while preserving the backend contract.

### Phase 4: release hardening

- Add operating-system credential storage and the designed connect/disconnect flow.
- Complete pagination, rate-limit, payload-bound, retry, and redaction handling.
- Add repository-selection behavior for absent or ambiguous remotes.
- Verify the feature on every supported platform.
- Document token setup, remote/local workspace boundaries, polling behavior, and destructive controls.

Exit condition: the definition of done and manual smoke matrix pass without environment-only credentials being required for ordinary users.

## 10. UI design plan

The fixed UI requirements remain:

- Devin has its own dedicated sidebar.
- The sidebar has its own show/hide action.
- It remains separate from the existing Agent provider selector and local-agent lifecycle.
- It must expose the session information and controller capabilities delivered by Sections 4 and 5.
- Destructive termination requires explicit confirmation.
- Authentication, loading, empty, waiting, failure, offline, and rate-limited states must be designed, not left as raw errors.
- The result must remain keyboard operable and accessible.

The rest of this section records the design decisions that satisfy those requirements. No backend phase should invent temporary product behavior; the Phase 2 development shell exposes the data and commands plainly, then is replaced by this design.

### 10.1 The user's job, and the design principle that follows

The local Agent sidebar is a conversation the user actively drives: one provider, one turn at a time, full attention. Supervising cloud Devin is a different job. The user has kicked off (or been pulled into, via Slack) several long-running remote sessions, then returned to local work. They check in occasionally. Three moments carry almost all of the value:

1. **Devin is blocked on them.** A session is waiting for a reply and the remote work is stalled until the user answers. This is the moment the sidebar must make loud, and the reply must be fast to send.
2. **Devin finished.** The user wants the outcome — usually a pull request — and a way to open it, not a transcript scroll.
3. **The user wants to kick off new work** against the current repository without leaving the editor.

Everything else — transcripts, activity feeds, lifecycle controls — is drill-in detail. Therefore the sidebar is **list-first, not chat-first**: its home surface is a triage list of sessions grouped by whether they need the user, and a session's conversation is one level deeper. This is the inverse of the Agent sidebar's hierarchy and is the correct inversion for a supervision surface.

Two boundary truths must stay visible without nagging: Devin works from its remote clone and cannot see unsaved or unpushed local work (surfaced once, in the create flow), and remote activity is remote (activity rows are labeled and never act like local file links).

### 10.2 Placement, sizing, and coexistence

The Devin sidebar docks on the **right**, in the same slot the Agent sidebar uses, with the same geometry: minimum width 320, default 440, drag-resizable up to 720, capped at 52% of content width, using the existing 5px divider and `CursorIcon::ResizeHorizontal` affordance. It stores its own persisted width field (`devin_sidebar_width`), independent of `agent_sidebar_width`.

**Coexistence decisions:**

- The Files explorer coexists with the Devin sidebar exactly as it does with the Agent sidebar.
- The Agent sidebar and the Devin sidebar are **mutually exclusive presentation surfaces**. Opening one hides the other. Both keep their full state (selected session, scroll position, composer draft) so toggling back restores the surface exactly. Rationale: `split_workspace` has one right slot; two right panels plus the explorer would crush the editor below usable width at common window sizes; and the user's attention model is one assistant surface at a time. This is a presentation constraint only — Devin polling rules follow Devin sidebar visibility, never Agent state.
- Toggling the Devin sidebar while the full agentic view is active first returns to the IDE layout (`set_agentic_mode(false)`), then opens the sidebar. The agentic view remains an Agent-only surface.

**Show/hide affordances:**

- New command `application.toggleDevinSidebar` in the keybindings catalog, `Global` scope, no default chord (matching `application.toggleAgentSidebar`), bindable through the existing Keybindings UI.
- A titlebar toggle icon in the right cluster, adjacent to the Agent sparkle toggle, following the existing rect-helper + interact pattern. When the sidebar is open, the icon state inverts and a close control also appears in the sidebar header, mirroring Agent behavior.

At the 320px minimum every layout below must remain functional: repository paths truncate in the middle, timestamps switch to compact relative form, and the metadata strip wraps. Nothing may overflow horizontally.

### 10.3 Navigation model

The sidebar has three views in a push/pop stack:

```text
Sessions list  ──open session──▶  Session detail
      │                                 ▲
      └──New session──▶  Create form ───┘ (on successful create)
```

- **Back** is a visible chevron button in the detail and create headers. Escape also navigates back when focus is inside the sidebar and no text field consumes it.
- Selection, scroll position, and composer drafts survive navigation and sidebar hide/show within a run.
- Hiding the sidebar never changes the view stack; reopening resumes exactly where the user left off, with an immediate refresh per Section 5.5.

### 10.4 Sessions list (home view)

**Header:** the title "Devin", a refresh button, a "New session" button, and an overflow menu (Open Devin web app, Disconnect). A close control on the far edge hides the sidebar.

**Filter row:** a single-line filter field (matches title, prompt snippet, and repository) and a segmented scope control — `Active | All | Archived` — using the existing `segment` component. Default scope: Active.

**Grouped list**, in fixed group order with sticky group labels:

| Group | Contents | Rationale |
| --- | --- | --- |
| Needs you | Sessions blocked on user input | The whole point of the surface; always pinned first |
| Working | Actively running sessions | Ambient awareness |
| Idle | Sleeping or suspended sessions | Resumable by messaging |
| Done | Completed, failed, and terminated sessions | Outcomes to review |

Within each group, most recently updated first. Archived sessions appear only under the Archived scope. Empty groups are omitted, not rendered empty.

**Row anatomy** (two lines, standard row height and hover/selection overlays from the theme):

- Line 1: status dot (colors per 10.7) · session title (or first-prompt snippet when untitled) · pull-request chip when the session has one.
- Line 2 (muted, smaller): status phrase · relative time since last activity · origin glyph (Slack, Devin web, Editur) · `owner/repository`.

The status dot carries the raw `status_detail` as hover text. Rows are focusable and open the detail view on click or Enter.

**Footer freshness line** (single muted line, always present when connected): "Updated 8s ago", switching to "Retrying…" during transient failures and "Rate limited — refresh slowed" during backoff. This line is the *only* surface for transient poll problems; they must not toast, flash, or clear the list.

**List empty states:**

- Connected, no sessions in scope: a centered empty state with one sentence and a "New session" button (Archived scope: sentence only).
- Not connected: the connect card (10.9) replaces the list entirely.

### 10.5 Session detail

Top-to-bottom anatomy:

1. **Header:** back chevron · session title · status pill (semantic category text; raw `status_detail` beneath it in micro type) · overflow menu with lifecycle actions: Sleep, Archive (or Unarchive), Open in Devin web app, and Terminate (styled with the danger token, separated last).
2. **Metadata strip** (wrapping, muted): `owner/repository` · origin · created time · ACU usage as `used / limit` when a limit exists, otherwise `used ACUs` · parent-session link when present · child-session links when present. Parent and child links navigate within the sidebar to that session's detail view; there is no tree UI (per Section 3 non-goals).
3. **Pull-request card**, pinned above the conversation whenever the session has one or more pull requests: PR title, state (draft/open/merged/closed), and an "Open" action that launches the system browser. This is the payoff artifact and must not be buried in the transcript. Multiple PRs stack as compact rows in one card.
4. **Conversation and activity stream** (scrollable, stick-to-bottom, height-culled like the Agent transcript):
   - User messages and Devin messages reuse the Agent bubble and markdown-galley rendering.
   - Remote activity events (shell, file, browser, Git, MCP, todo) render as dense collapsed rows, visually akin to the Agent's dense tool rows but clearly badged as remote. Consecutive activity events between two messages collapse into one expandable group row summarizing the burst (e.g. "14 remote actions — commands, file edits"). Expansion reveals individual rows with category icon, summary, timestamp, and bounded detail.
   - Remote file paths render as plain styled text with the session's repository as context. They are **not** `agent_path_link`s and never open local buffers.
   - Message attachments render as chips on the owning message; activating one opens the attachment URL in the system browser. Attachments are never auto-downloaded, and attachment URLs are never displayed raw (they may embed credentials).
5. **Waiting-on-you callout:** when the session is blocked on user input, a warning-toned callout sits directly above the composer — "Devin is waiting for your reply" — and the composer receives focus when the detail view opens in this state.
6. **Composer:** multiline text field at the bottom, Enter sends, Shift+Enter inserts a newline — identical muscle memory to the Agent composer, with its own focus id (`devin_prompt`). Behavior by session state:
   - Active or blocked: enabled, placeholder "Message Devin".
   - Sleeping: enabled, placeholder "Message Devin — sending will wake this session".
   - Terminated or archived: replaced by a single muted line stating why messaging is unavailable (with an Unarchive action for archived sessions).

Sent messages append optimistically with a pending affordance and reconcile against polled results by identity; a send failure marks the message with a retry action rather than silently dropping it.

### 10.6 Create-session flow

Creation is a pushed view (10.3), not a modal. Fields, in order:

1. **Repository** — prefilled from the normalized `origin` remote (Section 7). When the remote is absent or ambiguous, the field is empty with inline guidance and requires explicit entry; the form never guesses. Free-text `owner/repository` entry is always permitted.
2. **Prompt** — multiline, the dominant element of the form, autofocused when the repository is prefilled.
3. A passive one-line boundary note: "Devin works from the remote repository. Local uncommitted or unpushed changes are not visible to it." When the local project has uncommitted changes or unpushed commits, this line is present-tense and specific ("You have unpushed commits on `branch`"); otherwise it stays generic.

The first release intentionally exposes no tags, playbook, mode, or ACU-limit controls; sessions are tagged `editur` invisibly (Section 7). "Create" disables the form, shows an inline progress row, and on success navigates to the new session's detail view. On failure the form re-enables with a sanitized inline error callout above the actions — never a toast, since the user's prompt text is at stake and must remain visible and editable.

### 10.7 Status vocabulary and visual language

The backend's semantic categories (Section 5.3) map to fixed presentation:

| Category | Typical raw statuses | Color token | Motion |
| --- | --- | --- | --- |
| Needs you | blocked, awaiting user input | `semantic.warning` | None |
| Working | running, executing | `accent` | Subtle pulse on the status dot |
| Idle | sleeping, suspended | muted text tone | None |
| Finished | completed | `semantic.success` | None |
| Failed | failed, errored, terminated | `semantic.danger` | None |

Raw status and `status_detail` are always reachable — hover text on list dots, micro type under the detail status pill — but the category wording above is what the UI leads with. Unknown raw statuses map to a neutral category rendered with the raw string, never an error state.

Origin is a small glyph plus hover text (Slack, Devin web, Editur, other/unknown), used identically in list rows and the detail metadata strip. The Working pulse is the only animation on this surface and must degrade to a static dot when animations are disabled.

### 10.8 Lifecycle controls, confirmation, and recovery

All lifecycle controls live in the detail overflow menu (10.5). None appear on list rows — accidental lifecycle actions from a triage list are worse than one extra click.

| Action | Confirmation | Feedback | Recovery |
| --- | --- | --- | --- |
| Sleep | None (reversible by messaging) | Status pill updates on next poll; menu item disables while in flight | Send a message to wake |
| Archive | None (reversible) | Toast "Session archived"; view pops back to the list | Archived scope → Unarchive |
| Unarchive | None | Status updates in place | — |
| Terminate | **Modal `Dialog`, danger severity**, destructive-styled "Terminate session" button; body states that remote work stops permanently and cannot be resumed | Toast on success; session moves to Done | None — that is the point of the dialog |
| Send message | None | Optimistic append (10.5) | Inline retry on the failed message |

A lifecycle command failure surfaces as a danger toast with a sanitized message; the session's displayed state always re-converges to polled truth rather than trusting the optimistic transition.

The existing Agent `Cancel` action, keybindings, and permission cards are not connected to any of this (Section 5.2).

### 10.9 Authentication and connection states

There is no Devin section in Settings in the first release; connection lives entirely in the sidebar, mirroring how Agent providers authenticate in-surface.

- **Disconnected:** the sidebar body is a single connect card: one sentence of explanation, a masked single-line token field, a "Connect" button, and a link to Devin's personal-access-token documentation. The token is stored per Section 6 (environment for dev builds, OS credential store before release) and is never echoed back after entry.
- **Developer environment key:** when `DEVIN_API_KEY` is present, the connect card is skipped and the list header overflow shows "Using environment credentials" as a non-interactive informational item.
- **Organization selection:** if the authenticated identity requires choosing an organization, the connect card gains one dropdown after token validation. Single-organization identities never see it.
- **Connecting / validating:** the connect card's button shows inline progress; the sidebar never blocks the rest of the editor.
- **Auth failure at connect:** inline danger callout on the connect card ("That token was rejected" / "That token lacks session permissions"), field preserved for correction. Raw response bodies never render.
- **Auth failure mid-session (401/403 after connect):** a persistent warning banner pinned above the current view — "Devin connection lost — Reconnect" — with the last-fetched data left visible and clearly stale via the freshness line. Reconnect returns to the connect card with context intact.
- **Disconnect:** in the list-header overflow, confirmed with a neutral dialog ("Remove the stored Devin token from this machine?"). Disconnecting clears in-memory Devin state and shows the connect card; it never touches remote sessions.

### 10.10 State and transition inventory

Surface-level states the implementation must render and test:

| State | Entered when | Visible UI |
| --- | --- | --- |
| Disconnected | No credentials | Connect card (10.9) |
| Connecting | Token submitted | Connect card with inline progress |
| Auth failed | 401/403 at connect | Connect card with danger callout |
| Auth lost | 401/403 after connect | Reconnect banner over stale data |
| List loading | First fetch after connect | Skeleton rows or centered progress, header enabled |
| List empty | Fetch succeeded, no sessions in scope | Empty state with create action |
| List loaded | Sessions present | Grouped list (10.4) |
| List stale / retrying | Transient poll failure | Unchanged list, footer "Retrying…" |
| Rate limited | 429 / backoff active | Unchanged list, footer "Rate limited — refresh slowed" |
| Detail loading | Session opened, first fetch pending | Header + metadata immediately (from list data), stream placeholder |
| Detail loaded | Messages/events present | Full detail (10.5) |
| Needs-you detail | Session blocked on input | Waiting callout, composer focused |
| Terminal-session detail | Completed/failed/terminated | Stream intact, composer replaced per 10.5 |
| Create form | "New session" | Form (10.6) |
| Create submitting | Create sent | Disabled form, inline progress |
| Create failed | Create rejected | Re-enabled form, inline callout, prompt preserved |
| Offline | Network unreachable | Footer "Offline — will retry", stale data preserved |

Transitions between these states must never discard user-entered text (filter, composer drafts, create prompt) and must never clear fetched data in response to a fetch failure.

### 10.11 Notification behavior while the sidebar is hidden

**None in the first release.** Polling stops when the sidebar hides (Section 5.5), so any hidden-state badge would show stale data and train the user to distrust it. The titlebar toggle is a plain icon with no attention dot. Background desktop notifications and a live "needs you" badge are one deferred follow-on (Section 12) and must ship together with an explicitly designed background-polling mode, since both require the same data freshness guarantee.

While the sidebar is *visible*, no separate notification mechanism is needed — the Needs-you group at the top of the list is the notification.

### 10.12 Keyboard, focus, and accessibility

- **Scope:** a new `Scope::Devin` is active when focus is on `devin_prompt`, the list filter field, or the create form fields, following the existing scope-detection pattern.
- **Focus order:** opening the sidebar focuses the sessions list. In the list: Up/Down move between rows across group boundaries, Enter opens detail, type-ahead goes to the filter field. In detail: focus lands on the composer (always when the session needs a reply), Escape steps back to the list, Shift+Tab reaches the header controls. In the create form: repository → prompt → create/cancel.
- **Composer keys:** Enter sends, Shift+Enter newline — identical to the Agent composer.
- **Hit targets:** all interactive elements use standard control heights from the theme metrics; the status dot itself is not the hit target — the whole row is.
- **Accessible labels:** every interactive element sets `WidgetInfo`. Session rows announce title, semantic status, repository, and relative time as one label. The status pill announces the semantic category and the raw detail. The terminate dialog inherits the existing `Dialog` keyboard behavior (Enter confirms only the safe default, Escape cancels).
- **Reduced motion:** the Working pulse (10.7) is the only animation and falls back to a static dot when animations are disabled.
- **Color independence:** status is never conveyed by dot color alone — the status phrase on line 2 of each row and the pill text in detail carry the same information.

### 10.13 Component-reuse decision

| Reuse as-is | Adapt | Devin-specific (new) | Explicitly not reused |
| --- | --- | --- | --- |
| Theme tokens (`surface`, `semantic`, `accent`, metrics, motion) | Right-sidebar split, resize, and toggle plumbing (new fields, same pattern) | Session list rows and group headers | Agent provider selector and provider identity marks |
| `Dialog` for terminate/disconnect | Transcript `ScrollArea` with height culling and stick-to-bottom | Status pill and origin glyphs | Agent permission cards |
| `Toasts` for lifecycle feedback | Message bubbles and markdown galleys | Metadata strip and PR card | Agent diff rendering and changed-files footer (remote activity never becomes local diffs) |
| `chip`, `segment`, `selectable_row`, `icon_button` | Dense tool-row visuals → remote activity rows (re-badged, non-linking) | Connect card and reconnect banner | `agent_path_link` (would imply local files) |
| `TextEdit` composer anatomy (Enter/Shift+Enter) | Titlebar toggle rect/draw pattern | Create form and freshness footer | Agent session selector menus |

The adapted components share visual language with the Agent sidebar so the app feels like one product, while every piece that implies *local* agency (paths, diffs, permissions, provider identity) stays out so the remote boundary is never blurred.

### 10.14 Wireframes

Sessions list at the default 440px width:

```text
┌────────────────────────────────────────────┐
│ Devin                        ⟳   + New  ⋯ ✕│  header
├────────────────────────────────────────────┤
│ [ Filter sessions…      ] Active│All│Arch  │  filter + scope
├────────────────────────────────────────────┤
│ NEEDS YOU                                  │
│ ● Fix flaky auth retry test                │
│   Waiting for reply · 4m · ⧉ · owner/repo  │
│ WORKING                                    │
│ ◐ Add CSV export to reports                │
│   Writing code · 12m · ✎ · owner/repo      │
│ DONE                                       │
│ ✓ Refactor tree traversal      [PR #142]   │
│   Completed · 2h · ⌂ · owner/repo          │
├────────────────────────────────────────────┤
│ Updated 8s ago                             │  freshness footer
└────────────────────────────────────────────┘
```

Session detail at 440px, session blocked on the user:

```text
┌────────────────────────────────────────────┐
│ ‹  Fix flaky auth retry test            ⋯ │  back · title · menu
│    Needs you — waiting for your reply      │  pill + raw detail
├────────────────────────────────────────────┤
│ snowdamiz/editur · Slack · started 2h ago  │  metadata strip
│ 3.2 / 10 ACUs                              │
├────────────────────────────────────────────┤
│ ▣ PR #142 · Fix flaky auth retry    Open ↗ │  PR card (when present)
├────────────────────────────────────────────┤
│ YOU   Investigate the flaky auth test      │
│ DEVIN I reproduced it; the token refresh   │
│       races the retry timer…               │
│ ▸ 14 remote actions — commands, edits      │  collapsed activity group
│ DEVIN Should I pin the clock in the test   │
│       or widen the retry window?           │
│ ⚠ Devin is waiting for your reply          │  callout
├────────────────────────────────────────────┤
│ [ Message Devin…                    ] Send │  composer (focused)
└────────────────────────────────────────────┘
```

At the 320px minimum: repository paths middle-truncate, the scope segment collapses to a dropdown, the metadata strip wraps to as many lines as needed, timestamps use compact form ("4m"), and the PR card compresses to one row. No horizontal scrolling anywhere.

### 10.15 UI acceptance criteria and visual test cases

Acceptance criteria (all must hold, in addition to the Section 11 definition of done):

1. Opening the Devin sidebar while the Agent sidebar is open hides Agent and restores it fully — transcript, draft, scroll — when toggled back, and vice versa.
2. A session blocked on user input appears in the Needs-you group within one selected-session poll interval, and opening it focuses the composer with the waiting callout visible.
3. Enter sends and Shift+Enter inserts a newline in the Devin composer, matching the Agent composer exactly.
4. Terminate is impossible without the danger dialog; Escape cancels it; no other lifecycle action shows a modal.
5. A transient poll failure changes only the freshness footer; the list, detail stream, and all user-entered text are untouched.
6. Rate limiting and offline states are visibly distinguished in the footer and never toast.
7. The connect card never echoes a stored token, and no token fragment appears in any rendered error.
8. Remote file paths in activity rows are not clickable into local buffers, and no Devin activity appears in the editor's changed-files or diff surfaces.
9. Create with an absent or ambiguous origin requires explicit repository entry and shows the boundary note; a failed create preserves the prompt text.
10. Every session row, control, and dialog is reachable and operable by keyboard alone, and each carries an accessible label including status conveyed as text.
11. At 320px width, all views render without horizontal overflow or clipped controls.
12. With animations disabled, the Working indicator renders static.

Visual test cases: capture each state in the Section 10.10 inventory at 320px and 440px, plus the terminate dialog, the disconnect dialog, an expanded activity group, a multi-PR card, and a detail view for each of the five semantic status categories. These captures form the review set for the Phase 3 exit condition.

## 11. Definition of done

The integration is complete when:

- Devin opens through a dedicated sidebar and dedicated show/hide action.
- The existing Agent sidebar and local ACP provider selection behave exactly as before.
- The authenticated user can discover, create, inspect, message, sleep, archive, unarchive, terminate, and open supported Devin sessions.
- Slack-origin sessions can be discovered when Devin permissions make them visible.
- Messages and remote activity update without duplication and without blocking the UI thread.
- Hiding the sidebar stops active polling without affecting the remote session.
- Remote activity never appears as a local file mutation.
- Credentials and sensitive response material are not persisted or logged.
- Automated fake-transport tests and the manual authenticated smoke matrix pass.
- The Section 10 design is implemented and its acceptance criteria and visual test cases pass.

## 12. Deferred follow-ons

Add these only after real usage demonstrates the need:

- Child-session tree and batch launch controls.
- Explicit, previewed local-diff handoff.
- Background desktop notifications.
- Local pull-request checkout or comparison tools.
- Local Devin CLI over ACP as a separate Agent provider.
- Embedded remote desktop or terminal control if Cognition publishes a supported API.

