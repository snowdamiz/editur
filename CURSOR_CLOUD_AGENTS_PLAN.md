# Cursor Cloud Agents implementation plan

## 1. Purpose and reader

This plan is for the engineer adding Cursor Cloud Agents to Editur. After reading it, they should be able to validate Cursor's supported cloud surface, implement authentication and remote session control, integrate a cloud runtime into the existing Agent sidebar, verify the remote/local safety boundary, and release the feature without depending on Cursor IDE.

The post-read action is concrete: implement and ship a user flow that starts a Cursor Cloud Agent from the current GitHub repository, streams and follows its work in Editur, sends follow-up prompts, and opens the resulting branch, pull request, artifacts, or Cursor Web session.

This plan records the API surface verified on 2026-08-15. Cursor labels Cloud Agents API v1 public beta, so Phase 0 must revalidate every request and response used by the implementation before product code depends on it.

## 2. Decision summary

Implement Cursor Cloud as a remote surface in the existing Agent sidebar, backed directly by the public Cloud Agents REST API v1.

Do not implement it through ACP. Cursor ACP starts a local process over standard input/output and exposes local sessions, prompts, permissions, and Cursor-specific interaction methods. It has no documented cloud-runtime or cloud-handoff method.

Do not add Cursor Cloud to `ProviderId`. That enum remains the identity of local ACP providers. Add one application-level surface choice that distinguishes the current local ACP view from Cursor Cloud while preserving the last selected local provider.

Do not ship the TypeScript SDK, Python SDK, or SDK Bridge in the first implementation. Editur only needs cloud agents, and Cursor's own Bridge guidance recommends the HTTP API for that case. The existing Rust HTTP, JSON, TLS, and operating-system credential dependencies are sufficient unless the Phase 0 spike disproves that.

The minimum architecture is:

```text
Agent sidebar
    |
    +---- Local surface ---- AgentState ---- AgentController ---- ACP provider process
    |
    +---- Cursor Cloud ----- CursorCloudState
                                 ^
                                 |
                         CursorCloudController
                                 |
                                 v
                    https://api.cursor.com/v1
```

The cloud controller and state model are concrete Cursor integrations. Do not add a generic remote-agent trait, controller factory, or shared provider abstraction.

## 3. Product contract

The first release must let an authenticated user:

- Select **Cursor Cloud** from the existing Agent surface selector.
- Connect with a Cursor user API key without exposing it to preferences, logs, transcripts, or diagnostics.
- Resolve the current GitHub repository and pushed starting ref without guessing or pushing local work.
- See whether the local workspace contains unsaved, uncommitted, or unpushed work that the cloud agent cannot access.
- Start a cloud agent with a text prompt in `agent` or `plan` mode.
- Optionally select a model returned by Cursor, while allowing Cursor's configured default by omitting the model.
- Optionally request automatic pull-request creation; direct work on the current branch remains disabled.
- List, page, select, archive, and unarchive cloud agents visible to the API key.
- Inspect an agent's repository, runs, live status, final result, pushed branches, pull requests, token usage, and artifacts when available.
- Stream assistant text, thinking, and tool-call activity for the selected active run.
- Send a follow-up prompt after the current run reaches a terminal state.
- Cancel the active run without deleting the durable agent.
- Reconnect a dropped stream and recover terminal state when replay has expired.
- Open the canonical Cursor Web URL for capabilities Editur does not embed.
- Close Editur without cancelling or otherwise changing the remote run.

The integration must also preserve all existing local Agent behavior:

- Cursor, Codex, and Claude remain local ACP providers.
- Local provider selection, authentication, session history, permissions, diffs, buffer reconciliation, and process teardown keep their current semantics.
- Switching to Cursor Cloud never turns remote file paths into local paths and never adds remote edits to the local changed-files card.
- Returning from Cursor Cloud restores the last selected local provider, transcript, draft, and session state.

## 4. Non-goals

The first release will not include:

- A cloud mode hidden behind ACP messages, Cursor CLI input syntax, or undocumented Cursor endpoints.
- Replacement of Editur's local ACP implementation with the Cursor SDK.
- Automatic transfer of the current ACP conversation to a cloud agent.
- Automatic push of local branches, commits, diffs, unsaved buffers, or untracked files.
- Automatic checkout, merge, cherry-pick, or application of the cloud agent's branch.
- Direct writes to the starting branch. Every created agent uses `workOnCurrentBranch: false`.
- Permanent agent deletion. Archive and unarchive cover the reversible first-release lifecycle.
- Embedded remote desktop, browser, shell, environment builder, or Cursor Web application.
- Creation or management of Cursor Cloud environments, builds, secrets, self-hosted pools, or machines.
- Arbitrary inline MCP, environment-variable, hook, or custom-subagent configuration in Editur.
- No-repository or multi-repository agents.
- GitLab, Bitbucket, or Azure DevOps creation until Cursor documents those repository shapes in API v1 and an authenticated spike proves them.
- Background desktop notifications or a hidden-state attention badge.
- Full reconstruction of historical tool activity after Cursor's SSE retention window expires. The guaranteed fallback is run status, final result, Git metadata, and the Cursor Web link.
- Treating cloud tool calls as a stable schema. Cursor documents only the event envelope as stable.

## 5. Verified Cursor integration surface

### 5.1 ACP does not expose Cloud Agents

Cursor's documented ACP server runs with `agent acp`, uses newline-delimited JSON-RPC over standard input/output, creates or loads local sessions, and accepts a local working directory. Cursor's documented extension methods cover questions, plans, todos, subagent tasks, and image generation. No cloud launch, cloud list, runtime-selection, or handoff method is documented.

The interactive Cursor CLI supports a user-facing `&` handoff to Cloud Agents, but this is CLI interaction syntax rather than an ACP method. Editur must not synthesize it and depend on undocumented parsing behavior.

See [Cursor ACP](https://cursor.com/docs/cli/acp).

### 5.2 Cloud Agents API v1 is the supported launch surface

The public API uses durable agents plus per-prompt runs:

- An **agent** owns conversation and remote workspace state across prompts.
- A **run** represents one prompt, stream, status, result, and cancellation lifecycle.
- Only one run can be active on an agent at a time.
- The server returns `bc-...` agent identifiers and `run-...` run identifiers.
- Agent responses include a canonical `https://cursor.com/agents/...` URL.

The API accepts user and service-account API keys with Basic or Bearer authentication. Editur will use Bearer authentication because it avoids synthesizing Basic credentials.

See [Cloud Agents API](https://cursor.com/docs/cloud-agent/api/endpoints) and [Cursor APIs overview](https://cursor.com/docs/api).

### 5.3 Endpoint mapping

| User action | API operation | Implementation note |
| --- | --- | --- |
| Validate credentials | `GET /v1/me` | Return only sanitized identity and key-name metadata to state. |
| List agents | `GET /v1/agents` | Page with `limit` and `cursor`; request active-only unless the user selects archived scope. |
| Load agent metadata | `GET /v1/agents/{id}` | Fetch lazily on selection to avoid an N+1 list request. |
| Create agent and initial run | `POST /v1/agents` | Send one GitHub repository, a pushed starting ref, prompt, mode, optional model, and optional auto-PR flag. |
| List runs | `GET /v1/agents/{id}/runs` | Page newest first; load full run details only as needed. |
| Load run | `GET /v1/agents/{id}/runs/{runId}` | Guaranteed recovery source for status, final result, duration, and Git metadata. |
| Stream run | `GET /v1/agents/{id}/runs/{runId}/stream` | Consume SSE and reconnect with `Last-Event-ID`. |
| Follow up | `POST /v1/agents/{id}/runs` | Preserve remote conversation/workspace state; handle `409 agent_busy`. |
| Cancel run | `POST /v1/agents/{id}/runs/{runId}/cancel` | Cancels only the run; a later follow-up creates a new run. |
| Usage | `GET /v1/agents/{id}/usage` | Fetch after a terminal result or on explicit detail expansion. |
| List artifacts | `GET /v1/agents/{id}/artifacts` | Artifacts are agent-scoped and paths are remote. |
| Open artifact | `GET /v1/agents/{id}/artifacts/download?path=...` | Open the returned short-lived URL externally; do not attach API authorization to it. |
| Archive/unarchive | `POST /v1/agents/{id}/archive` and `/unarchive` | Reversible lifecycle; archived agents cannot accept runs. |
| List models | `GET /v1/models` | Fetch lazily when the create form opens its model picker. |
| List repositories | `GET /v1/repositories` | Optional fallback only; Cursor documents strict limits and potentially long latency. |

Do not implement permanent `DELETE`, worker-token, fleet-management, webhook v0, or Admin API endpoints.

### 5.4 Supported SSE events

Consume the documented simplified events and ignore `interaction_update` to prevent duplicate rendering:

| Event | State effect |
| --- | --- |
| `status` | Update current run status; the leading status event has no replay ID. |
| `assistant` | Append or coalesce assistant text deltas. |
| `thinking` | Append or coalesce thought deltas, subject to the same visibility rules as local Agent thoughts. |
| `tool_call` | Upsert one remote tool activity by `callId`; treat `args` and `result` as untrusted, unstable JSON. |
| `heartbeat` | Update stream freshness without creating transcript content. |
| `result` | Set terminal status, final text, duration, and Git metadata. Avoid duplicating text already received through deltas. |
| `error` | Surface a sanitized stream error and decide whether it is reconnectable. |
| `done` | Close the local stream reader; it does not change the durable agent. |

The API documents `Last-Event-ID` replay and a server-provided retention header. A `410 stream_expired` response must fall back to the run endpoint rather than retry forever.

## 6. Remote/local invariants

These are correctness requirements, not presentation preferences.

### 6.1 Repository state

Cursor Cloud works from a server-side clone. It cannot see:

- Unsaved Editur buffers.
- Uncommitted changes.
- Untracked files.
- Local commits that are not reachable from the remote starting ref.
- Local-only `.cursor` configuration or environment changes.

The create flow must display the exact remote repository and starting ref. When detectable, it must also display dirty-worktree and ahead-of-upstream warnings before submission. The warning is informative rather than a blocking modal, but an absent or unresolvable remote ref blocks creation.

Editur does not push anything as part of cloud-agent creation.

### 6.2 File and tool activity

Every path, command, URL, diff-like payload, and tool result from the Cloud API is remote data.

- Remote paths render as text with a `Remote` badge.
- Remote paths never call local path-opening helpers.
- Remote edit activity never populates `AgentState.changed_paths`, baselines, refresh queues, tab dirtiness, project search refreshes, or LSP synchronization.
- Remote tool payloads do not enter the local Agent transcript model merely to reuse rendering code.
- Pure rendering helpers such as markdown bubbles and collapsed activity cards may be reused with cloud-owned data types.

### 6.3 Process and run lifetime

- Dropping the local cloud controller closes local HTTP work only.
- Application shutdown never sends run cancellation, archive, or delete.
- Hiding the Agent sidebar never affects the remote run.
- Switching back to a local provider never affects the remote run.
- Cancellation requires an explicit user action against the currently displayed run ID.
- Archive and unarchive apply to the durable agent, not an individual run.

### 6.4 Interaction model

Cloud runs are autonomous. The public REST surface does not document ACP-style permission decisions or responses to blocking questions. Do not render local permission or question cards for cloud events.

If the stream reports that a run awaits input but provides no documented response endpoint, show a bounded notice and an **Open in Cursor Web** action. Do not guess an API request.

## 7. User experience plan

### 7.1 Surface selection

Keep one right-side Agent slot. Extend its existing identity menu with these conceptual choices:

- Cursor — This machine
- Cursor Cloud
- Codex — This machine
- Claude — This machine

Cursor Cloud is an application surface, not an ACP provider descriptor. The application retains two independent selections:

- The last selected local `ProviderId`.
- Whether the Agent surface currently shows local ACP or Cursor Cloud.

Selecting a local row sets the surface to local and applies the existing provider-switch lifecycle. Selecting Cursor Cloud sets the surface to cloud without overwriting the last local provider.

The surface selector must appear even in a build that contains only the Cursor ACP package, because Cursor Local and Cursor Cloud are still two available choices. Builds without the network feature show Cursor Cloud as unavailable with concise explanatory text.

Do not offer Cursor Cloud in the full agentic view. Selecting it from the sidebar exits full agentic view first because that view assumes local diffs and buffers.

Switching surfaces is disabled while the current local turn has unresolved work, matching the existing provider-switch safety behavior. A running cloud turn does not block switching back to local because the cloud run survives independently.

### 7.2 Cloud home and session list

When authenticated, Cursor Cloud opens to the most recently selected durable agent if it remains visible to the key. Otherwise it shows the cloud-agent list.

The list includes:

- Agent name.
- Active or archived state.
- Latest run status when available.
- Last updated time.
- A compact repository label when already loaded.

The list is newest first, paged in batches of at most 100, and has Active and Archived scopes. Do not issue one metadata request per list row. Full repositories and run state load after selection.

Header actions:

- New cloud agent.
- Refresh.
- Open Cursor Cloud in the system browser.
- Disconnect credentials.

### 7.3 Create-agent flow

Creation is an in-sidebar view rather than a modal so the prompt survives errors.

Fields and behavior:

1. **Repository** — prefilled from the normalized GitHub origin. If resolution fails, offer explicit HTTPS repository entry or a user-triggered repository lookup.
2. **Starting ref** — prefilled from the pushed upstream branch. Show the remote commit identifier when available.
3. **Workspace boundary** — state whether dirty files or unpushed commits are excluded.
4. **Mode** — Agent or Plan. Agent is the default.
5. **Model** — Cursor default plus the lazily fetched model catalog. Omit `model` for Cursor default.
6. **Create pull request automatically** — unchecked by default. Regardless of this choice, `workOnCurrentBranch` remains false.
7. **Prompt** — required multiline text.
8. **Images** — optional after the text-only vertical slice passes. Accept only Cursor-supported image formats and the smaller intersection of Cursor's limits and Editur's existing attachment limits.

Submission disables the form but preserves every value. On success, navigate to the new agent detail and attach the initial run stream. On failure, re-enable the form with a sanitized inline error.

Do not expose named environments, inline MCP servers, environment variables, custom subagents, multi-repo arrays, no-repo mode, or direct-branch work.

### 7.4 Agent detail

The detail view contains:

1. Header with back action, name, status, archive/unarchive, open-in-web, and close-sidebar actions.
2. Repository and remote-environment metadata.
3. Current or latest run status, duration, and token usage.
4. Pushed branch and pull-request card when returned by Cursor.
5. Artifact card when artifacts exist.
6. Conversation/activity region.
7. Follow-up composer or active-run cancel action.

The conversation/activity region renders:

- User prompts submitted during the current Editur process.
- Assistant text and thinking received live.
- Remote tool-call cards upserted by call ID.
- Final result text for historical runs.
- A visible history-gap row when earlier activity is unavailable because the app did not observe it or replay expired.

Historical REST data is not presented as a complete transcript. If a prior run exposes only its final result, label it **Run result** and offer **Open full conversation in Cursor Web**.

### 7.5 Composer and lifecycle

- A new agent requires an initial prompt through the create flow.
- An active run disables follow-up submission and shows **Cancel run**.
- A terminal run enables the follow-up composer.
- An archived agent replaces the composer with an **Unarchive to continue** action.
- `409 agent_busy` refreshes the latest run instead of blindly retrying the prompt.
- Enter sends and Shift+Enter inserts a newline, matching the local Agent composer.
- Prompt text clears only after Cursor accepts the create or follow-up request.

Archive is reversible and requires no modal. Permanent delete is absent. Cancelling an active run uses a confirmation dialog that names the run and explains that partial remote changes may remain in its branch.

### 7.6 Hidden and disconnected behavior

The cloud controller starts lazily on first selection of Cursor Cloud.

When another surface is selected or the sidebar is hidden:

- An already attached active-run SSE stream may continue so the in-memory transcript remains current.
- Agent-list polling stops; there is no continuous list poll in the first release.
- No badge or desktop notification is shown.
- Terminal status can be applied to cloud state without touching local Agent state.

Disconnecting removes the stored key and closes local Cloud API requests. It does not cancel, archive, or delete any remote agent.

## 8. Application architecture

### 8.1 Ownership boundary

Add one concrete Cursor Cloud module with four responsibilities:

- **Credentials** — load environment or keyring credentials, validate only safe local constraints, and redact debug output.
- **Transport** — authenticated REST requests, response bounds, typed normalization, SSE parsing, and error mapping.
- **Controller** — bounded commands/events, list operations, selected-agent generation, stream ownership, retry/backoff, and shutdown.
- **State** — selected agent, list pages, runs, transcript/activity, creation draft, model/repository choices, artifacts, usage, errors, and connection state.

Keep network work off the UI thread. Use the same wake-callback pattern as the existing background controllers so incoming cloud events request a frame.

Do not make `AgentController` generic. Do not make local `AgentState` hold cloud fields. The application draw path chooses the local or cloud renderer based on the current surface.

### 8.2 Minimal surface state

Use a small application-level surface enum with two states:

```text
Local
CursorCloud
```

Retain `selected_provider` for the local state. Add one `CursorCloudState` and at most one `CursorCloudController` to the application. Cloud selection does not require a map because there is one cloud backend.

Persist only:

- Last selected Agent surface.
- Last selected cloud agent ID.

Do not persist API keys, presigned artifact URLs, raw tool payloads, or complete transcripts in normal preferences.

### 8.3 Cloud state model

The normalized model should include:

- `CloudConnection`: disconnected, validating, ready, offline, rate-limited, failed.
- `CloudIdentity`: sanitized API-key name and optional user identity.
- `CloudRepository`: canonical HTTPS URL, starting ref, remote commit, dirty flag, ahead count, and availability state.
- `CloudAgentSummary`: ID, name, status, URL, environment, latest run ID, timestamps, and archived state.
- `CloudAgentDetail`: summary plus repositories, auto-PR flag, and branch-work policy.
- `CloudRun`: ID, agent ID, status, result, timestamps, duration, Git metadata, and stream completeness.
- `CloudTranscriptItem`: user text, assistant text, thought, remote tool activity, status/gap marker, or sanitized error.
- `CloudToolActivity`: call ID, name, status, bounded arguments/result, and extracted remote path labels.
- `CloudArtifact`: relative path, size, and updated timestamp.
- `CloudUsage`: token totals and per-run usage.
- `CloudModelChoice`: model ID, display name, description, parameters, and variants.

Use generation numbers on agent selection and stream attachment. Late responses for an earlier selection must be ignored by the reducer.

Deduplicate:

- Agents by agent ID.
- Runs by run ID.
- Stream events by SSE event ID when present.
- Tool activity by call ID.
- Artifacts by relative path.

### 8.4 Controller commands

The first-release command set should cover:

- Set surface visibility.
- Validate or save credentials.
- Disconnect.
- Refresh or page agents.
- Select an agent with a generation number.
- Refresh selected agent and runs.
- Create an agent with normalized creation options.
- Create a follow-up run.
- Attach or reattach to a run stream.
- Cancel the current run after UI confirmation.
- Refresh usage.
- Refresh artifacts.
- Archive or unarchive the selected agent.
- Load models on demand.
- Load repositories on explicit request only.
- Shut down local workers.

Opening web pages and presigned artifact URLs remains an application/UI action after the transport has returned a validated HTTPS URL.

### 8.5 Controller events

Normalize transport output into bounded application events:

- Connection and credential-source changes.
- Identity loaded.
- Repository resolution changed.
- Model or repository catalog loaded.
- Agent page loaded.
- Selected agent loaded.
- Run page loaded.
- Run started or status changed.
- User prompt accepted.
- Assistant or thought delta.
- Remote tool activity updated.
- Stream gap detected.
- Run finished with result and Git metadata.
- Usage loaded.
- Artifacts loaded.
- Agent archived or unarchived.
- Sanitized failure with retry metadata.

Never send authorization headers, full raw response bodies, or unbounded JSON through the event channel.

### 8.6 Thread and stream model

A synchronous SSE body can block while control requests still need to run. Use two bounded roles:

1. A controller worker owns commands, ordinary REST calls, stateful retry scheduling, and stream generations.
2. At most one stream reader owns the selected run's SSE request and sends parsed internal events back to the controller or directly to the bounded application event channel with its generation.

The stream reader must be replaceable when selection or run changes. Use a stop flag plus finite receive/read timeouts so shutdown does not wait indefinitely on a socket. Join old readers before considering their generation reusable.

Do not add an async runtime solely for SSE. Add one only if the Phase 0 spike proves the existing HTTP stack cannot provide correct streaming, timeout, and shutdown behavior on supported platforms.

Coalesce adjacent assistant and thought deltas before waking the UI. Flush on a short interval, a type change, a tool event, terminal status, or a small bounded byte threshold. This prevents token-level events from outrunning the application's bounded frame drain.

### 8.7 Transport limits

Use explicit bounds:

- Normal JSON response: at most 4 MiB.
- One SSE event payload: at most 1 MiB before parsing.
- Displayed tool arguments or result: at most 64 KiB each, with a truncation marker.
- Agent/run list page: at most the API maximum of 100 items.
- Model and repository catalogs: bounded by response bytes and item count before entering state.
- Assistant/thought transcript: use the same overall resident transcript limits as the local Agent where practical.
- Images: maximum five, supported MIME types only, and never larger than Editur's existing per-file and total attachment limits.

An oversized item fails that item or request safely. It does not allocate based on an upstream length without checking the bound.

## 9. SSE parsing and recovery contract

### 9.1 Parser behavior

The parser must support standard SSE framing required by Cursor:

- `event`, `data`, `id`, and `retry` fields.
- Multiple `data` lines joined with newline characters.
- Blank-line event termination.
- CRLF and LF input.
- Comment/heartbeat lines.
- A final complete event at end-of-stream.
- Empty IDs and events without IDs.
- Bounded field and event sizes.

Event IDs are opaque strings. Never parse or order them numerically.

### 9.2 Reconnection

After a transient disconnect:

1. Preserve the latest fully dispatched event ID for that run.
2. Reconnect with `Last-Event-ID`.
3. Accept the leading status event without treating it as duplicate transcript content.
4. Back off with jitter after repeated failures and respect retry guidance when available.
5. Reset failure count only after the stream is healthy or the run reaches terminal state.

If Cursor returns `400 invalid_last_event_id`:

- Confirm the run and agent IDs still match the selected generation.
- Fetch the run state.
- Restart without the stale ID only if the run remains active, and insert one history-gap marker.

If Cursor returns `410 stream_expired`:

- Fetch the run state.
- If terminal, render the final result and mark prior live activity incomplete.
- If active, restart from the current stream head without an ID and insert one history-gap marker.

Do not retry `401`, `403`, invalid request shapes, or terminal run errors as transient network failures.

### 9.3 Duplicate final text

Cursor may stream assistant deltas and later repeat final text in the `result` event or run resource. The state reducer must not append the complete final string after already accumulating the same assistant response.

Prefer this rule:

- The live assistant item is authoritative when non-empty.
- The final result fills an empty assistant result or is stored as run metadata.
- If the strings materially differ, retain the live item and show the final result in a separate collapsed **Run result** row rather than trying to merge text heuristically.

## 10. Authentication and credential handling

### 10.1 Credential sources

Support two sources:

1. `CURSOR_API_KEY` for developer and managed environments.
2. Operating-system credential storage for user-entered keys.

Use a Cursor Cloud-specific keyring account so it cannot collide with Devin, ACP providers, or future services.

Do not read or copy:

- Cursor ACP login tokens.
- Cursor CLI configuration files.
- Cursor IDE storage.
- `CURSOR_AUTH_TOKEN`.

The official SDK documentation states that SDK integrations do not auto-discover credentials from a local Cursor installation. Treat Cloud API authentication as a separate explicit connection.

### 10.2 Connect flow

The disconnected cloud surface shows:

- A short explanation that a Cursor Cloud API key is required.
- A masked input.
- A link to Cursor's API-key page.
- A Connect action.

On submission:

1. Trim the key and reject empty or unreasonably large values locally.
2. Keep it in a redacting credential type.
3. Validate it with `GET /v1/me`.
4. Store it in the OS credential store only after validation succeeds.
5. Clear the input buffer after storage.
6. Load the agent list.

Do not rely only on a key prefix for validation; the server is authoritative.

### 10.3 Request security

- Fix the API origin to `https://api.cursor.com` in production.
- Use rustls through the existing HTTP dependency.
- Prevent redirects from forwarding the Authorization header to a different origin. Prefer no redirects for authenticated API calls.
- Set `Authorization: Bearer ...` only on Cursor API requests.
- Never attach the Cursor key to artifact presigned URLs or Cursor Web URLs.
- Sanitize all HTTP and API errors before UI or diagnostics receive them.
- Truncate upstream error messages and prefer documented structured error codes.
- Never log request headers, prompt images, raw tool payloads, response bodies, or presigned URLs.

### 10.4 Disconnect and auth loss

Disconnecting deletes the keyring record, clears in-memory identity/catalog data, stops local API work, and returns to the connect card. It never changes remote agents.

A `401` or `403` after connection preserves already loaded non-secret state but marks it stale and returns the surface to a reconnect-required state. Retrying requires new or corrected credentials.

## 11. Git repository and environment resolution

### 11.1 Repository URL

Resolve the current project's `origin` remote using a bounded Git subprocess. Accept common GitHub HTTPS and SSH forms, remove a trailing `.git`, reject embedded username/password data, and produce canonical `https://github.com/owner/repository`.

If there is no single safe GitHub origin:

- Do not guess.
- Allow explicit canonical HTTPS entry.
- Offer the Cursor repository catalog only when the user requests it.

Cursor documents repository-catalog limits of one request per user per minute and thirty per user per hour, with potentially long latency. Cache a successful response in memory and never poll it.

### 11.2 Starting ref

Resolve:

- Current branch.
- Configured upstream branch.
- Local HEAD commit.
- Upstream commit.
- Ahead count.
- Dirty and untracked state.

Creation is allowed when a known remote branch or reachable remote commit can be supplied as `startingRef`. A detached local HEAD, branch without a pushed upstream, or ambiguous remote requires explicit user selection of a remote ref.

Never pass a local-only commit SHA merely because Git can resolve it locally.

### 11.3 Creation policy

Always send:

- One canonical repository URL.
- The confirmed remote starting ref.
- `workOnCurrentBranch: false`.
- `mode: agent` or `mode: plan`.

Send only when selected:

- `model` and supported parameter values from the live model catalog.
- `autoCreatePR: true`.
- Supported prompt images.

Omit fields when Editur wants Cursor's default. In particular, omitting `model` is more robust than pinning a model slug.

### 11.4 Cloud environment boundary

Editur does not configure or build the remote environment. Cursor uses its configured environment, snapshot, or pushed `.cursor/environment.json` state.

The create view should link to Cursor Cloud setup documentation when a run reports environment/setup failure. Do not infer that a local build success means the cloud environment is configured.

Do not expose `envVars` in the first release. Cursor currently documents a rollout behavior where unsupported accounts may silently ignore them, which is unsuitable for a secrets UI.

## 12. Error and recovery behavior

Normalize errors into actionable categories:

| Category | Examples | UI/recovery |
| --- | --- | --- |
| Authentication | 401, invalid key | Reconnect card; no automatic retry. |
| Forbidden/integration | 403, repository not connected | Preserve form; explain repository access; link to Cursor setup. |
| Invalid request | 400, unsupported model/ref | Preserve user input; show sanitized field-level message. |
| Not found | 404 agent/run | Refresh list and clear selection only if confirmed absent. |
| Busy/conflict | 409 `agent_busy` | Refresh latest run; do not duplicate the prompt. |
| Rate limited | 429 | Respect retry guidance, show slowed state, keep loaded content. |
| Offline/transient | timeout, connection reset, 5xx | Back off with jitter; keep stale content visible. |
| Replay expired | 410 | Fetch run, recover result/status, show one stream-gap marker. |
| Oversized/malformed | bound or schema failure | Stop that request, preserve prior state, show safe protocol error. |

Creation and follow-up prompts clear only after a successful response identifies the created agent/run. A timeout after request transmission is ambiguous: refresh agents/runs before enabling an identical retry. Do not create a duplicate automatically.

For idempotent creation, investigate the documented client-supplied `agentId` during Phase 0. Cursor currently documents that it conflicts with agent-scoped environment variables, which Editur does not send. If authenticated testing proves it reliable, generate one `bc-<uuid>` per submission and reuse it only for ambiguity recovery. Otherwise, resolve ambiguous creates by refreshing and asking the user before retrying.

## 13. TDD and verification strategy

Use focused red-green vertical slices. Automated tests use a local fake HTTP/SSE server and sanitized fixtures; they never require network access or a real Cursor key.

### 13.1 Pure tests

Add small tests for logic that can regress:

1. GitHub remote normalization rejects credentials and unsupported hosts.
2. Remote-ref resolution distinguishes clean/pushed, dirty, ahead, detached, and no-upstream states.
3. Credential debug and errors never contain a key.
4. Agent, run, model, artifact, usage, and error responses normalize only required fields.
5. SSE parsing covers multiline data, CRLF, comments, IDs, missing IDs, retry fields, EOF, and size limits.
6. State application deduplicates events, replaces tool updates, rejects stale generations, and avoids duplicate final text.
7. Stream-expiry recovery inserts one gap marker and converges on run state.

Do not write tests that merely restate passive serde field mappings.

### 13.2 Controller tests

One fake-server integration suite should prove:

- Validate credentials, list agents, and page with `nextCursor`.
- Create agent, receive IDs, attach SSE, stream text/tool/status, and finish.
- Follow up on the same durable agent after terminal state.
- Handle `409 agent_busy` without submitting twice.
- Cancel the active run and keep the durable agent usable.
- Reconnect with the exact last opaque event ID.
- Recover from invalid/expired replay using the run endpoint.
- Preserve selected-agent state when an older response arrives late.
- Archive and unarchive idempotently.
- Fetch usage and artifacts without downloading artifact content through the API transport.
- Handle 401, 403, 404, 429, 5xx, timeout, malformed JSON, malformed SSE, and oversized events.
- Drop the controller without issuing cancel/archive/delete and without leaking a stream thread.

### 13.3 Application and UI tests

Extend headless UI coverage for:

- Cursor Cloud appears alongside a Cursor-only local build.
- Switching to cloud preserves local provider/session/draft state and does not start another ACP provider.
- A running local turn prevents a destructive surface switch.
- A running cloud turn permits returning to local while the remote stream continues.
- The connect form masks and clears credentials.
- Create flow renders repository/ref and dirty/unpushed boundaries.
- Prompt text survives every failed or ambiguous request.
- Remote tool paths are not local links and never enter local changed-file UI.
- Historical result and stream-gap states are labeled honestly.
- Cancel confirmation, archive, unarchive, artifact, branch, PR, and open-web controls call the intended command/action.
- Keyboard focus, Enter/Shift+Enter, Escape/back navigation, accessible labels, and 320-pixel sidebar layout work.

### 13.4 Manual authenticated smoke matrix

Run on native macOS, Linux, and Windows before stable release:

1. Connect a user API key and restart Editur to prove keyring loading.
2. Launch an Agent-mode run from a pushed branch with a dirty local worktree and confirm the dirty change is absent remotely.
3. Launch a Plan-mode run using Cursor default model.
4. Launch with one explicitly selected model from the live catalog.
5. Stream representative read, shell, edit, subagent, and MCP tool activity.
6. Hide the sidebar and switch to a local provider while the cloud run continues.
7. Quit Editur mid-run, reopen, and recover the terminal result or active stream without cancelling it.
8. Send a follow-up after completion.
9. Cancel a run, then successfully send a new follow-up.
10. Confirm a branch or pull request appears and opens externally.
11. Open an artifact through its presigned URL without leaking the Cursor key.
12. Archive and unarchive the durable agent.
13. Open an agent created in Cursor Web and verify the historical-detail limitation is communicated rather than fabricated.
14. Disconnect and confirm remote agents remain unchanged.
15. Inspect debug logs and diagnostics for key, Authorization header, prompt-image data, raw tool secrets, and presigned URLs.

### 13.5 Performance and disk checks

- Normal Editur startup performs no Cursor Cloud request and starts no cloud controller.
- Opening the local Agent surface performs no Cloud API request.
- Agent-list refresh is user-driven or surface-driven, never frame-driven.
- Model and repository catalogs load lazily.
- Stream deltas are coalesced so rendering remains bounded during token-heavy output.
- Measure sidebar frame cost with a long cloud transcript and many tool updates.
- Check project disk usage before and after full Rust test/build cycles.
- When incremental output becomes material, remove only this project's Rust incremental build directories; never clean global Cargo caches or unrelated workspaces.

## 14. Implementation phases

### Phase 0: authenticated API and UX spike

- Re-read the current API, SDK, Bridge, ACP, pricing, security, and Cloud Agent setup documentation.
- Exercise `GET /v1/me`, agents, runs, stream, models, repositories, usage, artifacts, archive, and unarchive with a real user key.
- Create one disposable agent against a test repository and record sanitized fixtures.
- Verify simplified SSE event names, delta behavior, tool payload truncation, heartbeat cadence, replay IDs, retention headers, and terminal ordering.
- Close the client mid-run and test replay before and after retention expiry.
- Determine what historical content REST exposes for agents created elsewhere.
- Verify client-supplied agent-ID behavior for ambiguous create recovery.
- Verify app-created agents appear in Cursor Web and that the returned URL opens the expected agent.
- Confirm current terms permit Editur's use and distribution model.

Stop condition: if REST cannot provide reliable live streaming, terminal recovery, and the explicitly accepted historical-result experience, do not use undocumented endpoints. Reassess the official SDK Bridge and its `sdk.v1` conversation surface as a separate plan revision.

Exit condition: a small internal harness can create, stream, follow up, cancel, list, reopen, archive, and inspect a real cloud agent with documented behavior and sanitized fixtures.

### Phase 1: transport vertical slice

- Write failing fake-server tests for credential validation, create, stream, and terminal recovery.
- Implement the redacting credential type and environment/keyring loading seam without adding the user-facing save flow yet.
- Implement fixed-origin Bearer HTTP requests, bounds, status/error normalization, and typed minimal response structs.
- Implement the SSE parser and one stream reader with deterministic shutdown.
- Implement reconnect, `Last-Event-ID`, expired replay fallback, and delta coalescing.
- Keep every API operation not required by create → stream → finish out of this phase.

Exit condition: the fake server drives one complete create → live stream → result sequence with no UI and no leaked worker.

### Phase 2: controller and state vertical slice

- Add Cursor Cloud commands, events, state, selection generations, and reducer tests.
- Add list/select/run loading and the active stream lifecycle.
- Add follow-up and cancellation.
- Add Git metadata extraction for one canonical GitHub origin and pushed upstream ref.
- Ensure remote activity cannot enter local Agent state by construction.

Exit condition: tests drive list → select → create → stream → follow-up → cancel/recover entirely through the controller/state boundary.

### Phase 3: Agent surface integration

- Add Local/Cursor Cloud surface selection while retaining the existing local provider selection.
- Dispatch the Agent sidebar draw path to local or cloud state.
- Preserve local provider state and controller lifecycle when entering and leaving cloud.
- Add cloud home, session list, create form, detail view, transcript/activity, and composer shell.
- Reuse pure visual helpers but keep cloud data types and remote path rendering separate.
- Keep full agentic view local-only.

Exit condition: a development build can launch and follow a text-only cloud run entirely inside the Agent sidebar without regressing local providers.

### Phase 4: authentication and creation completeness

- Add the connect, validate, keyring-save, reconnect, disconnect, and auth-loss UI.
- Complete dirty/unpushed/ref warnings and explicit repository fallback.
- Add lazy model selection and optional auto-PR.
- Add supported image attachments using the intersection of API and Editur limits.
- Add accessible labels, focus order, narrow-sidebar behavior, and input preservation.

Exit condition: an ordinary user can connect and launch a correctly scoped cloud agent without environment variables or Cursor IDE.

### Phase 5: recovery, history, and outputs

- Complete agents/runs pagination, latest-run recovery, and honest historical-result presentation.
- Add usage, branch, pull-request, artifact, archive, unarchive, and open-web actions.
- Complete rate-limit, offline, ambiguous-create, stale-generation, invalid-replay, and expired-replay handling.
- Ensure stream continuation while the cloud surface is hidden does not cause unnecessary frames or local state mutation.

Exit condition: interruption, restart, external-origin agent, and lifecycle cases converge on correct server state without duplicate work.

### Phase 6: release hardening

- Run focused unit/controller/UI tests, then the full suite.
- Run the authenticated smoke matrix on every supported platform.
- Verify no secret-bearing data enters logs, diagnostics, preferences, crash output, or artifact links.
- Record Cloud API version/date and manual compatibility results in release documentation.
- Document API-key setup, repository prerequisites, billing/plan expectations, remote/local boundaries, history limitations, and Cursor Web fallback.
- Measure startup, idle, streaming, and long-transcript performance.
- Check and manage only this project's incremental Rust build disk usage during the cross-platform cycle.

Exit condition: all acceptance criteria pass and removing or hiding Cursor Cloud leaves local Agent functionality and stored ACP data untouched.

## 15. Definition of done

The integration is complete when:

- Cursor Cloud is selectable in the existing Agent sidebar without becoming an ACP `ProviderId`.
- No Cloud API process or network request occurs until the user selects Cursor Cloud.
- A user can authenticate with an API key stored in the OS credential store.
- The create flow identifies the exact remote repository/ref and warns about excluded local state.
- Every created agent works on a new Cursor branch rather than the starting branch.
- The user can create, list, select, stream, follow up, cancel, archive, and unarchive cloud agents.
- Run text, thinking, remote tools, status, result, usage, Git outputs, and artifacts render when the API exposes them.
- Reconnect uses opaque event IDs and expired replay falls back to run state without an infinite loop.
- Closing Editur or changing surfaces never cancels or archives a remote agent.
- Remote paths and edits never affect local buffers, file links, changed-file counts, search refresh, LSP, or diffs.
- Historical gaps are explicitly labeled and the full Cursor Web session is one action away.
- Local Cursor, Codex, and Claude ACP behavior remains unchanged.
- Automated fake-transport tests and the native authenticated smoke matrix pass.
- Credentials and sensitive payloads are absent from persistent settings, logs, diagnostics, and rendered raw errors.
- Public user documentation explains setup, billing/plan dependency, GitHub access, remote workspace behavior, and limitations.

## 16. Risks and stop conditions

| Risk | Default control | Stop/reassess when |
| --- | --- | --- |
| Public-beta API changes | Typed minimal structs, ignore unknown fields, Phase 0 and release smoke | Required fields or semantics become undocumented or unstable. |
| REST history is incomplete | Honest run-result UI plus Cursor Web link | Product requires complete cross-device transcript reconstruction inside Editur. |
| Synchronous HTTP cannot stream safely | Dedicated bounded stream reader and finite timeouts | Shutdown leaks/hangs or native platforms disagree; evaluate SDK Bridge or a focused streaming dependency. |
| Cloud activity contaminates local state | Separate state/events/rendering and non-linking paths | Any automated test can make a cloud event dirty a local buffer or changed-files record. |
| Duplicate create after timeout | Optional client ID or refresh-before-retry | API cannot disambiguate and duplicate paid work is plausible. |
| API key leakage | Keyring, redacting type, fixed host, no body/header logging | Any key fragment appears in test diagnostics or logs. |
| Repository mismatch | Canonical origin and pushed-ref validation | Editur cannot prove the remote ref Cursor will clone. |
| Repository catalog rate limits | Explicit lazy load and in-memory cache | Normal navigation triggers catalog calls. |
| Tool schema drift | Stable envelope only; raw JSON bounded/optional | UI correctness depends on an undocumented tool-specific shape. |
| User expects local changes in cloud | Specific create warning; no auto-push | Testing shows users still believe dirty/unpushed work was transferred. |
| Feature regresses local ACP | Separate controller/state and targeted regression suite | Cloud selection requires broad changes to ACP protocol or provider internals. |
| Binary/dependency growth | Direct REST with existing dependencies | Required behavior forces SDK Bridge packaging; write a separate supply-chain plan first. |

## 17. Deferred follow-ons

Add only after real usage demonstrates the need:

- Complete historical conversation retrieval through the official SDK Bridge.
- Background completion/needs-input notifications and a trustworthy hidden-state badge.
- Explicit previewed handoff of a local diff or pushed temporary branch.
- Local comparison or checkout of the cloud branch.
- Multi-repository and no-repository agents.
- Named cloud environments and self-hosted workers.
- Inline MCP, environment-variable, and custom-subagent configuration.
- Remote desktop embedding if Cursor publishes a supported embeddable API.
- Permanent delete with a destructive confirmation and recovery policy.
- GitLab, Bitbucket, and Azure DevOps once API v1 documents and proves them.
- A shared remote-agent visual component layer, but only after Cursor Cloud and Devin expose enough identical stable behavior to justify it.

## 18. Reference snapshot

Primary references verified for this plan:

- [Cursor ACP](https://cursor.com/docs/cli/acp)
- [Cloud Agents overview](https://cursor.com/docs/cloud-agent)
- [Cloud Agents API v1](https://cursor.com/docs/cloud-agent/api/endpoints)
- [Cursor APIs overview and authentication](https://cursor.com/docs/api)
- [Cursor TypeScript SDK](https://cursor.com/docs/sdk/typescript)
- [Cursor SDK Bridge](https://cursor.com/docs/sdk/bridge)

Before implementation or release, re-check these pages rather than relying on copied model IDs, rate limits, beta fields, or tool payload examples in this plan.
