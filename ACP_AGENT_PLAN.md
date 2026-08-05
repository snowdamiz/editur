# ACP agent sidebar implementation plan

## 1. Purpose

Add agentic development to Editur as a native sidebar backed by Cursor through the Agent Client Protocol (ACP). The editor remains a fast Rust application; Cursor runs as a separate process and communicates with Editur over ACP.

### Audience and outcome

This plan is for the engineer implementing the feature. After reading it, they should be able to build the first useful agent sidebar, verify its safety and performance, and know which capabilities are deliberately deferred.

## 2. Product contract

The first useful release must:

- Add Files and Agent views to the existing sidebar without replacing the editor.
- Provision a compatible Cursor Agent automatically during Editur installation and updates; the user must not install a CLI separately.
- Start Cursor only after the Agent view is opened.
- Use the current project root as the agent workspace.
- Stream assistant text, plans, tool activity, errors, and completion state without blocking the UI thread.
- Let the user send a follow-up in the same session.
- Let the user stop an active turn.
- Present every ACP permission request with its proposed action and the choices supplied by the agent.
- Return only the choice the user selects; never silently approve a request.
- Keep open buffers coherent when the agent changes files on disk.
- Repair or reinstall the managed Cursor Agent when it is missing or corrupt, and fail clearly when automatic provisioning, authentication, compatibility, or execution fails.
- Add no work, child process, network request, or idle polling to normal application startup when the Agent view is never opened.

The first release will not include:

- Cursor cloud agents or the Cursor TypeScript/Python SDK.
- Multiple simultaneous agents, subagents, side chats, or conversation search.
- Git worktrees, automatic commits, branches, pull requests, or background jobs.
- A custom agent implementation, model router, tool framework, or MCP manager.
- Mirroring Cursor Agent, modifying a user's global Cursor installation, or adding Cursor commands to `PATH`.
- Persistent Editur-owned permission allowlists.
- ACP v2 draft features.

These remain follow-up work only after the local single-session workflow is reliable.

## 3. Architecture decision

Use Editur as an ACP client and Cursor Agent as the ACP server:

```text
egui Agent sidebar
        |
        v
AgentController on a background thread
        |
        v
official Rust ACP client
        |
        v
Editur-managed cursor-agent acp subprocess
        |
        v
Cursor agent harness and hosted models
```

Use the official `agent-client-protocol` Rust crate and negotiate stable protocol v1. Do not enable ACP v2 draft, conductor, proxy-chain, or MCP-over-ACP features for the first release.

The controller owns the process, connection, and session. The application owns only renderable state and sends commands to the controller through channels, following the existing background-search controller pattern. No ACP parsing, process reads, writes, or waits occur on the UI thread.

Run the ACP connection on one dedicated thread using the crate's supported blocking connection path. Do not add a global async runtime. If the pinned crate cannot operate this way, stop during the protocol spike and measure the smallest supported executor before changing the runtime model.

Use one Cursor process and one active ACP session per Editur process. Support one active prompt turn at a time. Reuse the process and session for follow-ups, and shut them down when Editur exits.

## 4. Protocol spike and go/no-go gate

Before building the sidebar, prove the Cursor Agent release selected from the official ACP Registry works through stable ACP v1. Record the tested Cursor Agent version and the capabilities returned during initialization.

The spike must demonstrate:

1. Editur can install the registry package into a private application-data directory and launch its declared ACP command without invoking a shell.
2. ACP initialization succeeds and reports a compatible protocol version.
3. A session can be created for an explicit project root.
4. A prompt streams assistant and tool-call updates.
5. A second prompt continues the same session.
6. A sensitive command produces `session/request_permission` and waits for the response.
7. Allow and reject decisions have the expected effect.
8. Cancellation stops an active prompt.
9. A file edit is visible on disk and identifies the affected path in ACP updates when Cursor provides one.
10. Closing the connection terminates the child process and its descendants on macOS, Linux, and Windows.
11. The package does not replace itself behind Editur's back, or Cursor provides a supported way to disable that behavior for a managed installation.

Use a real Cursor account only for this manual smoke test. Automated tests must use a deterministic fake ACP process and must not require network access or Cursor credentials.

Do not continue to the full UI if Cursor's ACP implementation cannot provide streaming, cancellation, and interactive permission requests. Reassess the Cursor SDK or another ACP-compatible agent instead.

## 5. Controller contract

The UI sends a small command enum to the controller:

| Command | Meaning |
| --- | --- |
| Connect | Launch Cursor and initialize ACP for the project root. |
| NewSession | Discard the current conversation and create a new session. |
| Prompt | Send user text to the current session. |
| DecidePermission | Return the selected ACP option for one pending request. |
| Cancel | Cancel the active prompt. |
| Shutdown | Close the connection and terminate the child process. |

The controller sends normalized events back to the UI:

| Event | UI effect |
| --- | --- |
| ConnectionChanged | Show starting, ready, incompatible, signed-out, or failed state. |
| SessionReady | Enable the prompt input. |
| UserMessage | Add the submitted prompt to the transcript. |
| AssistantDelta | Append streamed assistant text. |
| PlanUpdated | Render the current plan and task status. |
| ToolCallUpdated | Render or update one tool activity card. |
| PermissionRequested | Render an approval card and retain its request identifier. |
| UsageUpdated | Update usage or cost only when Cursor reports it. |
| TurnFinished | Re-enable input and trigger file reconciliation. |
| Error | Show a recoverable error without losing the transcript. |
| ProcessExited | Disable the session and offer an explicit reconnect. |

Normalize ACP messages at the controller boundary. The egui layer should not depend on raw JSON-RPC values or Cursor-specific message shapes. Preserve unknown ACP updates in logs and ignore them safely so additive protocol changes do not crash the editor.

Do not add a trait, factory, or pluggable agent abstraction. Add that only when a second agent implementation is actually being integrated.

## 6. Provisioning, process discovery, and authentication

The user must not install Cursor Agent separately. Editur owns a private, versioned installation under its application-data directory and launches that exact executable. It does not alter shell configuration, `PATH`, or an existing Cursor installation.

### Release manifest

At release time, CI selects a tested Cursor entry from the official ACP Registry. For every Editur release target, it downloads the declared archive directly from Cursor and produces an Editur sidecar manifest containing:

- Cursor Agent version.
- Target operating system and architecture.
- Cursor-owned HTTPS archive URL.
- SHA-256 of the exact archive tested by CI.
- Relative paths, file types, and SHA-256 values for the extracted package files.
- Archive format, executable path, and ACP arguments.
- Maximum compressed and extracted sizes.
- Cursor license and terms links.

Publish that small manifest with the signed or attested Editur release. Do not resolve the mutable `latest` registry entry on an end user's machine, mirror the proprietary archive, or execute Cursor's remote installation script. The user downloads the package directly from the Cursor URL pinned by the Editur release.

### Installation and update

Both bootstrap installers state that Cursor Agent is an included third-party dependency, then provision the pinned sidecar after installing Editur without requiring a second setup step. The self-updater provisions the sidecar required by the new Editur release before replacing the current Editur executable.

Provisioning must:

1. Skip the download when the exact version is already installed and every managed package file matches the release manifest.
2. Download only over HTTPS from the pinned Cursor host into a fresh temporary directory.
3. Enforce the manifest's compressed-size limit while downloading.
4. Verify SHA-256 before extraction.
5. Reject archive entries that escape the staging directory and enforce extracted-size and entry-count limits.
6. Verify every extracted path, file type, and checksum against the release manifest, then verify that the declared command reports the pinned version.
7. Move the complete staged version atomically into the private version directory.
8. Switch the active-version marker only after validation succeeds.
9. Keep the prior known-good version until the new Editur and sidecar have both launched successfully, then remove only obsolete managed versions.

Do not call `cursor-agent update`: Editur updates the pinned sidecar through its own verified release flow. Confirm during the spike that Cursor Agent's default self-update can be disabled or does not mutate an ACP Registry installation. If it cannot be controlled, that is a release blocker because the installed binary would no longer match Editur's tested version and checksum.

If sidecar provisioning fails during an Editur update, keep the current Editur and Cursor Agent versions and return a concise error. If the managed files later disappear or fail verification, opening the Agent view runs the same provisioner in the background, shows download progress, and offers Retry. It never directs the user to install a CLI manually.

The ACP Registry marks Cursor Agent as proprietary. Downloading the official registry package directly onto the user's machine avoids redistributing it inside Editur's archives, but the release still requires confirmation that this installer-mediated use complies with Cursor's current terms. If Cursor withdraws the registry distribution or disallows this use, do not fall back to an undocumented download path.

### Authentication

Installation does not bypass account consent. On first use, start ACP-advertised browser authentication inside the Agent view. The user may need to sign in to Cursor once, but they must not need a terminal command. Never collect, persist, print, or forward Cursor credentials through Editur; Cursor Agent owns its credential storage.

Capture stderr into a bounded diagnostic buffer. Normal protocol messages belong on stdout; malformed stdout is a connection error. On an unexpected exit, keep the transcript, show the exit status and bounded diagnostic text, and restart only after the user chooses Retry or sends another prompt. Do not create an automatic restart loop.

## 7. Sidebar interaction

The existing sidebar gains a compact Files/Agent switcher. Preserve the current sidebar width and toggle shortcut.

The Agent view contains, from top to bottom:

1. Provisioning, authentication, connection, and session status with New Session and retry actions.
2. A scrollable transcript containing messages, plans, tool calls, permission requests, and errors.
3. A multiline prompt input with Send while idle and Stop while a turn is active.

For the first release:

- Keep one transcript in memory.
- Disable Send for an empty prompt or while another prompt is active.
- Keep follow-ups in the same ACP session.
- Auto-scroll only when the user is already near the bottom.
- Collapse completed tool cards by default; keep running, failed, and approval cards expanded.
- Show the tool title, status, affected path, command, and working directory when ACP supplies them.
- Show unknown tool input in a plain expandable details view rather than guessing its meaning.
- Render model and mode controls only when the session advertises supported options. Do not hardcode model identifiers.
- Keep an outstanding approval visible even if the user switches back to Files, and mark the Agent tab as waiting.

Conversation history, transcript search, Markdown-rich rendering, images, and parallel chats are not required for the first release.

## 8. Permission and safety behavior

An ACP permission request is a blocking user decision, not an informational event.

- Display the exact options and labels supplied by the agent.
- Associate the UI card with the ACP request identifier and tool call.
- Send exactly one response for each request.
- Disable the card immediately after a selection to prevent duplicate responses.
- Never manufacture an Always Allow option or persist a choice beyond what ACP defines.
- Never auto-approve because a command looks harmless.
- If the connection closes, treat the pending request as abandoned and do not claim it was denied or allowed.
- On application shutdown, close the connection and terminate the child instead of inventing a permission response.

ACP approval is not a complete operating-system security boundary. During the spike, verify which shell, file, network, and MCP actions Cursor routes through permission requests and which it executes inside its own sandbox or policy. Document any uncovered behavior before release.

Launch the agent with the project root as its working directory and pass only declared workspace roots. Do not expose home-directory or arbitrary filesystem capabilities beyond what the feature requires.

## 9. Buffer and project coherence

Cursor edits files on disk while Editur may hold one of those files in memory. Data safety takes priority over seamlessness.

Before every prompt, require the current buffer to be saved. Offer Save and Run or Cancel; do not send stale, unsaved text as if Cursor could see it.

During an active turn, the user may continue editing. Reuse the existing disk fingerprint and safe-save conflict behavior:

- If the open buffer is clean and its disk fingerprint changes, reload it while preserving the nearest valid cursor position.
- If the open buffer is dirty and its disk fingerprint changes, do not reload or merge automatically. Mark an external-change conflict and retain the user's text.
- A later save must continue to stop at the existing conflict dialog rather than overwrite the agent's edit.
- Reconcile once after every tool update that identifies the open path, once when the window regains focus, and once when the turn finishes.
- While a turn is active, use a slow metadata-first check for the open file only; do not add a recursive file watcher dependency.

After a completed turn, invalidate affected tree entries and the project-search index when ACP reports changed paths. If Cursor does not reliably report paths, refresh the current buffer and visible tree state only; do not scan or hash the entire project after every turn.

The first release renders diffs supplied by ACP when available. It does not implement a second diff engine or snapshot the project. Git-backed worktree review can be added later if direct-workspace conflicts become a real usability problem.

## 10. Performance requirements

The feature must preserve Editur's native startup and idle behavior:

- No agent crate initialization or executable lookup on normal editor startup.
- No Cursor process before the Agent view is opened.
- No ACP work on the UI thread.
- No repaint loop while the agent is idle.
- During streaming, drain a bounded number of events per frame so one large update cannot stall input.
- Bound transcript text, captured stderr, and retained raw tool details. Drop old diagnostic detail first; if completed messages must be removed, insert a visible truncation marker.
- Keep one long-lived Cursor process instead of starting one per prompt.

Measure and record:

| Metric | Acceptance target |
| --- | ---: |
| Normal startup regression with Agent unopened | At most 5 ms on the reference machine |
| Idle CPU with Agent unopened | No measurable regression from the release baseline |
| Time from opening Agent to connection-ready | Record; no fixed target until the spike establishes Cursor's cost |
| ACP event receipt to rendered update | Under one frame during an active stream |
| Release binary growth | Remain under the existing 30 MiB product target |
| Cursor archive download and installed footprint | Record separately for every release target |

The Cursor package and child process are measured separately from the Editur binary. Model and network latency are not Editur performance regressions.

## 11. TDD and verification strategy

Build the feature in vertical slices. Each non-trivial controller behavior starts with one failing test using a fake ACP process, then the smallest implementation that passes.

### Automated checks

The fake process must cover:

- Successful initialization and session creation.
- Text and tool updates split across multiple messages.
- Follow-up prompts in one session.
- A permission request that remains pending until the test responds.
- Allow and reject responses using the supplied option identifiers.
- Cancellation.
- Malformed JSON and unknown message types.
- Agent stderr output and unexpected exit.
- Clean shutdown with no orphan child process.

Provisioning tests must cover:

- Selecting the current platform from a pinned manifest.
- Refusing an unpinned host, wrong checksum, oversized response, and unsupported archive.
- Rejecting path traversal and extraction-limit violations.
- Installing into a staged version and switching active versions only after validation.
- Leaving the prior version active after an interrupted or failed update.
- Treating an already verified version as an idempotent no-op.

Add focused state tests for:

- Prompt enablement and one-active-turn enforcement.
- Permission cards accepting exactly one decision.
- Streamed deltas preserving order.
- Clean-buffer reload versus dirty-buffer conflict.
- Reconnect preserving the visible transcript but creating a new connection/session.

Do not test egui's own layout or ACP crate internals. Keep networked Cursor smoke tests manual because CI must not require an account, consume paid tokens, or depend on service availability.

Before each phase is declared complete, run the repository's tests, formatting, Clippy with warnings denied, and the relevant release build. Check this project's build-directory size before and after dependency changes, and remove only this project's incremental artifacts when space becomes excessive.

## 12. Implementation phases

### Phase 0: Validate Cursor ACP

- Complete the protocol spike and record the tested Cursor Agent version and capabilities.
- Confirm the ACP Registry packages, executable paths, native Windows support, authentication, and self-update behavior on supported platforms.
- Confirm installer-mediated direct download is permitted by Cursor's current distribution terms.
- Confirm stable ACP v1 is sufficient.
- Measure the official Rust ACP crate's release-size and compile-time impact before keeping it.

Exit condition: the registry package can be provisioned without a manual install, and streaming, follow-ups, permission decisions, cancellation, editing, and shutdown work against it.

### Phase 1: Sidecar provisioning

- Generate a pinned, per-platform sidecar manifest in the release pipeline.
- Implement the shared verified provisioner and its failure-path tests first.
- Call it from bootstrap installation, self-update, and Agent-view repair.
- Keep Cursor Agent private to Editur and preserve one prior known-good version.

Exit condition: a clean machine can install or update Editur and receive the tested Cursor Agent without a separate command or global PATH change.

### Phase 2: Controller vertical slice

- Add the narrowly featured ACP dependency.
- Implement the controller thread, process lifecycle, commands, and normalized events.
- Build the fake ACP process and controller tests first.
- Handle missing executable, incompatible protocol, malformed output, and process exit.

Exit condition: a test can start the fake agent, stream one prompt, complete it, and shut down without an orphan process.

### Phase 3: Minimal Agent view

- Add the Files/Agent switcher and in-memory transcript.
- Connect lazily on first Agent view open.
- Send one prompt, render streaming text and tool updates, then send a follow-up.
- Keep idle rendering event-driven.

Exit condition: a real read-only Cursor prompt can be completed entirely from the sidebar without freezing editor input.

### Phase 4: Permissions and cancellation

- Render ACP permission requests using the returned choices.
- Wire decisions and prevent duplicate responses.
- Add Stop and protocol cancellation.
- Keep pending approval visible across sidebar view changes.

Exit condition: the user can inspect, allow, or reject a sensitive command, and can stop a long-running turn.

### Phase 5: File reconciliation

- Gate every prompt on a saved current buffer.
- Reuse disk fingerprints to reload clean external edits.
- Preserve dirty buffers and surface conflicts.
- Invalidate reported tree and search state after changes.

Exit condition: agent edits appear in Editur without silent data loss, and user edits are never overwritten automatically.

### Phase 6: Product hardening

- Render advertised model and mode options.
- Complete automatic repair, authentication, incompatible-version, reconnect, and diagnostic states.
- Add bounded transcript/tool/error retention.
- Run native smoke tests on macOS, Linux, and Windows.
- Measure startup, idle, streaming latency, memory, and release size.
- Document installation, authentication, data-use, and billing expectations.

Exit condition: all acceptance criteria pass and the normal non-agent editor remains within its existing performance targets.

## 13. Acceptance checklist

- [ ] Opening Editur normally does not start or inspect Cursor Agent.
- [ ] Bootstrap installation and self-update provision the tested Cursor Agent without a separate user command.
- [ ] The managed sidecar is private to Editur and does not modify `PATH` or a global Cursor installation.
- [ ] Opening Agent starts one background Cursor ACP process and keeps the UI responsive.
- [ ] Missing or corrupt sidecar files repair in the Agent view without asking for a manual CLI installation.
- [ ] Authentication happens through the sidebar's browser flow.
- [ ] The first prompt streams visible progress and a follow-up keeps context.
- [ ] Only one prompt can run at a time.
- [ ] Stop cancels the active prompt.
- [ ] Permission requests show the proposed action and wait for the user's exact choice.
- [ ] No permission request is auto-approved by Editur.
- [ ] Clean open files reload after agent edits.
- [ ] Dirty open files survive external edits and retain save-conflict protection.
- [ ] Agent failure cannot crash the editor or leave an orphan process.
- [ ] Tests use a fake agent and require no Cursor account or network.
- [ ] Stable ACP v1 is negotiated and unsupported capabilities are hidden or disabled.
- [ ] Normal startup, idle CPU, and release size remain within their targets.
- [ ] Cursor installation, authentication, privacy, billing, and beta limitations are documented for users.

## 14. Risks and upgrade triggers

| Risk | Initial response | Revisit when |
| --- | --- | --- |
| Cursor's ACP implementation changes or omits an optional feature | Negotiate capabilities, require a tested minimum version, and fail clearly. | A required stable-v1 behavior is unavailable. |
| ACP v2 becomes stable | Stay on v1. | Cursor supports stable v2 and it removes code or unlocks a required capability. |
| Cursor changes or withdraws its ACP Registry distribution | Keep the last compatible installed version and block new releases that cannot provision legally. | Cursor restores a documented installation route or another ACP agent is selected. |
| The registry package has no publisher-supplied checksum | Pin the archive URL and CI-computed SHA-256 in the attested Editur release manifest. | Cursor or the registry publishes signatures or checksums that can replace this trust path. |
| Cursor Agent self-updates outside Editur | Require a supported way to prevent mutation of the managed copy. | Cursor exposes a stable managed-update contract. |
| Direct agent edits conflict with user work | Keep fingerprint conflict protection and one active turn. | Conflicts are common enough to justify Git worktrees. |
| Official ACP crate materially increases size or runtime complexity | Keep only stable-v1 features and measure before merging. | It breaks the 30 MiB target or forces a global runtime. |
| Cursor ACP remains too unstable | Stop rather than building compatibility hacks. | The Cursor SDK or another ACP agent provides the required editor contract more reliably. |
| Transcript memory grows without bound | Bound diagnostic and raw tool data first. | Persistent or very long sessions become a real product need. |

## 15. References

- [Cursor in JetBrains through ACP](https://cursor.com/blog/jetbrains-acp)
- [Cursor Agent in the ACP Registry](https://github.com/agentclientprotocol/registry/blob/main/cursor/agent.json)
- [ACP Registry distribution format](https://github.com/agentclientprotocol/registry)
- [ACP architecture](https://agentclientprotocol.com/get-started/architecture)
- [ACP stable v1 documentation](https://agentclientprotocol.com/protocol/v1/overview)
- [Official ACP Rust SDK](https://github.com/agentclientprotocol/rust-sdk)
- [Cursor Agent installation and update behavior](https://docs.cursor.com/en/cli/installation)
- [Cursor terms of service](https://cursor.com/terms-of-service)
- [Cursor SDK headless auto-review behavior](https://cursor.com/changelog/sdk-updates-jun-2026)
