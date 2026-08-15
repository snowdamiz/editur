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

The detailed visual design is intentionally not defined here. Section 10 records the design work that must be filled in by a separate UI pass.

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

Fill in Section 10 before implementing the final sidebar. The design pass may reuse existing Agent visual components, but it must not merge ownership or lifecycle with the Agent sidebar.

Exit condition: the dedicated Devin sidebar satisfies the approved design states and accessibility checks while preserving the backend contract.

### Phase 4: release hardening

- Add operating-system credential storage and the designed connect/disconnect flow.
- Complete pagination, rate-limit, payload-bound, retry, and redaction handling.
- Add repository-selection behavior for absent or ambiguous remotes.
- Verify the feature on every supported platform.
- Document token setup, remote/local workspace boundaries, polling behavior, and destructive controls.

Exit condition: the definition of done and manual smoke matrix pass without environment-only credentials being required for ordinary users.

## 10. UI design plan — intentionally open

The only fixed UI requirements are:

- Devin has its own dedicated sidebar.
- The sidebar has its own show/hide action.
- It remains separate from the existing Agent provider selector and local-agent lifecycle.
- It must expose the session information and controller capabilities delivered by Sections 4 and 5.
- Destructive termination requires explicit confirmation.
- Authentication, loading, empty, waiting, failure, offline, and rate-limited states must be designed, not left as raw errors.
- The result must remain keyboard operable and accessible.

The UI design contributor must decide and record:

- Sidebar placement, sizing, resizing, and behavior relative to other sidebars and narrow windows.
- Whether multiple sidebars can coexist or are mutually exclusive presentation surfaces.
- Session discovery, grouping, filtering, selection, and creation flows.
- Conversation and activity information hierarchy.
- Status, origin, ACU, pull-request, attachment, and child-session presentation.
- Follow-up messaging and lifecycle-control placement.
- Confirmation and recovery behavior for sleep, archive, unarchive, and terminate.
- Authentication and organization-selection flows.
- Notification behavior while the sidebar is hidden.
- Which existing Agent components should be reused visually and which should remain Devin-specific.
- Focus order, shortcuts, accessible labels, reduced-motion behavior, minimum hit targets, and screen-reader announcements.

Expected UI-plan outputs:

- State and transition inventory.
- Wireframes for the chosen desktop widths.
- Keyboard and accessibility behavior.
- Component-reuse decision.
- Final UI acceptance criteria and visual test cases.

No backend phase should invent temporary product behavior to answer these design questions. The Phase 2 development shell should expose the data and commands plainly, then be replaced by the approved design.

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
- Section 10 has been completed by the UI design pass and its acceptance criteria pass.

## 12. Deferred follow-ons

Add these only after real usage demonstrates the need:

- Child-session tree and batch launch controls.
- Explicit, previewed local-diff handoff.
- Background desktop notifications.
- Local pull-request checkout or comparison tools.
- Local Devin CLI over ACP as a separate Agent provider.
- Embedded remote desktop or terminal control if Cognition publishes a supported API.

