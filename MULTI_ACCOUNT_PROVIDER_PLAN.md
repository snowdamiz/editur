# Multi-account ACP provider plan

## 1. Purpose

Add multiple accounts per ACP provider to Editur. Users must be able to select an account manually and, when a provider account exhausts its plan during a turn, optionally continue the run through another configured account on the same provider.

This plan follows the existing shared-provider architecture in [ACP_PROVIDER_PLAN.md](ACP_PROVIDER_PLAN.md). It does not add a second controller implementation, provider plugin system, credential vault, or parallel provider runtime.

The core design is:

- Treat an account as part of the ACP process identity, not as mutable state inside a live process.
- Run only the currently selected account's provider process.
- Isolate provider-owned credentials and native sessions by account.
- Keep manual account sessions independent.
- On proven plan exhaustion, start the next account in a new native ACP session and carry the interrupted conversation forward with a bounded Editur handoff.

ACP does not guarantee that active sessions survive `logout`, and `session/load` or `session/resume` can only restore a native session visible to the newly authenticated agent. Consequently, Editur must not promise that reauthenticating a live process transfers the underlying native session.

Relevant protocol references:

- [ACP authentication and logout](https://agentclientprotocol.com/protocol/v1/authentication)
- [ACP session creation, loading, and resuming](https://agentclientprotocol.com/protocol/v1/session-setup)

## 2. Product contract

The completed feature must:

- Allow more than one named account for Cursor, Codex, and Claude when the provider has a safe account-isolation mechanism.
- Preserve existing users' current provider credentials and sessions without copying or inspecting them.
- Let users manually switch accounts while no turn, permission request, or interaction request is active.
- Keep account authentication, native sessions, hidden-session history, active-session metadata, and Editur-created session metadata isolated.
- Let users opt accounts into automatic failover and order their failover priority.
- Fail over only after a provider-specific, tested usage-exhaustion signal.
- Continue an interrupted run with the prior conversation, attachments, partial work, and current workspace available to the replacement account.
- Bound failover attempts so one user turn never retries an account twice or loops forever.
- Persist no API key, auth token, refresh token, or browser credential in Editur preferences.
- Keep provider packages and compile caches shared so each account does not duplicate the managed installation.
- Keep only one ACP provider process active per Agent pane.

The initial implementation will not include:

- Simultaneous account processes or load balancing across healthy accounts.
- Automatic account rotation before an actual usage-limit rejection.
- Cross-provider failover, such as Codex to Claude.
- Copying or modifying provider-owned credential files.
- A general secret manager or API-key entry form.
- Exact reconstruction of every native session segment after a cross-account handoff.
- Automatic deletion of provider-owned account directories when an account is removed from Editur.
- Text matching on arbitrary errors such as `429`, `quota`, or `limit` without a pinned provider contract.

## 3. Current implementation

Snapshot audited 2026-08-20.

### 3.1 Provider and process identity

The provider catalog uses a closed `ProviderId` enum with Cursor, Codex, and Claude. `PreparedAgent` describes one provider command, arguments, environment additions, and environment removals.

`run_managed_process` starts the selected installed sidecar and inherits the rest of Editur's parent environment. It does not currently set an account-specific `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, Cursor credential source, or account identifier.

This makes authentication an implicit property of the spawned process and the user's global provider configuration.

### 3.2 Authentication

The controller initializes ACP, normalizes the advertised authentication methods, and retries session startup after successful authentication.

- ACP agent authentication sends only the advertised method id on the existing connection.
- Claude terminal authentication opens the same prepared command in a system terminal and then retries session creation.
- Codex API-key authentication is treated as environment-owned.
- Editur does not currently capture or invoke ACP `logout`.

Provider logout is not a safe account-switch mechanism. ACP explicitly leaves the fate of existing sessions after logout undefined.

### 3.3 Application state

The selected identity is currently only `ProviderId`.

Both the primary Agent pane and split Agent pane runtime use:

```rust
selected_provider: ProviderId,
provider_agents: HashMap<ProviderId, AgentState>,
agent_controllers: HashMap<ProviderId, AgentController>,
```

Provider switching drops the old controller, swaps the provider's retained `AgentState`, saves the selected provider, and starts the new provider.

### 3.4 Session persistence and detection

Editur metadata is scoped by provider and project:

```text
agents/<provider>/session-history/<project-hash>.json
agents/<provider>/active-session/<project-hash>.json
agents/<provider>/editur-sessions/<project-hash>.json
```

Account identity is absent, so adding accounts without changing these paths would mix hidden sessions, active-session restoration, and Editur session ownership.

External session detection also reads global provider stores:

- Cursor: Cursor application data and `~/.cursor/projects`.
- Codex: `CODEX_HOME`, otherwise `~/.codex`.
- Claude: `CLAUDE_CONFIG_DIR`, otherwise `~/.claude/projects`.

### 3.5 Turn failures and usage

`UsageUpdated` represents context-window usage and optional cost. It is not a provider-plan exhaustion signal.

`send_prompt` currently converts ACP errors into user-facing strings. Apart from Cursor's narrow transport-resume classifier, the structured ACP error code and data do not reach application state.

The existing Cursor resume path retries the same native session with `Continue from where you left off.` after a known transport drop. It is not an account failover mechanism.

### 3.6 Existing handoff behavior

External session import already has a useful precedent:

- It visits a provider transcript.
- It converts the useful user, assistant, and tool entries into a handoff prompt.
- It excludes thoughts and images.
- It bounds the final prompt to 64 KiB.
- It creates a new ACP session and continues there.

The cross-account implementation should generalize this helper rather than create a second handoff format.

## 4. Target account model

Add the smallest account-qualified identity:

```rust
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct AccountKey {
    pub provider: ProviderId,
    pub account_id: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderAccount {
    pub key: AccountKey,
    pub label: String,
    pub auth_source: AuthSource,
    pub auto_failover: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AuthSource {
    Legacy,
    ProviderManaged,
    Environment { variable: String },
}
```

Use a monotonic local `u64` account id. Do not add a UUID or random-id dependency.

`Legacy` means the existing global provider configuration and session storage. At migration, create one legacy account per available provider with a label such as `Current login`. This preserves current behavior without reading, copying, or relocating credentials.

`ProviderManaged` means authentication runs inside an isolated provider data directory.

`Environment` stores only the name of an environment variable. At process spawn, Editur maps that variable's value to the provider's canonical credential variable. The value must never be serialized, rendered, or included in diagnostics.

Persist a bounded, versioned account registry using the existing atomic small-file pattern:

```text
<data-dir>/agent-accounts.json
```

The registry order is the user's failover order. Persist the selected `AccountKey` instead of only `ProviderId`.

## 5. Storage and process isolation

Keep managed packages and compilation caches shared. Put only account-owned state under an account directory:

```text
agents/
  codex/
    versions/                         shared
    cache/                            shared
    accounts/1/provider-data/         provider auth, config, native sessions
    accounts/1/session-history/       Editur hidden-session metadata
    accounts/1/active-session/        Editur active-session metadata
    accounts/1/editur-sessions/       Editur session-origin metadata
  claude/
    ...
```

Add an account-aware storage-root helper. It must accept a validated numeric account id, resolve beneath the selected provider root, and never accept a path supplied directly through internal process arguments.

### 5.1 Codex

For a provider-managed account:

- Set `CODEX_HOME` to the account's provider-data directory.
- Configure Codex to use file-based CLI auth storage within that directory.
- Remove ambient `CODEX_API_KEY` and `OPENAI_API_KEY` so another account cannot override the isolated login.
- If the account uses `AuthSource::Environment`, map its named variable to the required canonical Codex credential variable for only that process.
- Keep existing executable-redirection and protocol-log environment removals.

Codex describes `CODEX_HOME` as its state directory, including its SQLite state, logs, configuration, sessions, and file-backed authentication. This isolation therefore also gives each account its own Codex configuration. Editur must not copy a user's global Codex configuration into it automatically.

Reference: [Codex configuration source](https://github.com/openai/codex/blob/main/codex-rs/core/src/config/mod.rs).

### 5.2 Claude

For a provider-managed account:

- Set `CLAUDE_CONFIG_DIR` to the account's provider-data directory.
- Preserve that environment for both the ACP process and Claude's terminal authentication command.
- Continue removing `CLAUDE_CODE_OAUTH_TOKEN` and executable or Node overrides.
- Remove ambient `ANTHROPIC_API_KEY` for provider-managed subscription accounts.
- If the account uses `AuthSource::Environment`, map its named variable to `ANTHROPIC_API_KEY` for only that process.

The pinned Claude ACP adapter derives its configuration root from `CLAUDE_CONFIG_DIR`.

Reference: [Claude ACP adapter v0.66.0](https://github.com/agentclientprotocol/claude-agent-acp/blob/v0.66.0/src/acp-agent.ts).

### 5.3 Cursor

Cursor documents browser login, `CURSOR_API_KEY`, and `CURSOR_AUTH_TOKEN`, but not a dedicated credential-directory variable.

The first safe implementation should:

- Support multiple Cursor API-key or auth-token accounts using `AuthSource::Environment`.
- Remove ambient `CURSOR_API_KEY` and `CURSOR_AUTH_TOKEN` before mapping the selected account's named environment variable.
- Preserve the current global browser login as the Cursor legacy account.
- Add isolated browser login only after a pinned-binary test proves where credentials and sessions are stored and proves isolation will not alter the environment seen by agent-run shell tools.

Do not set a synthetic `HOME` for Cursor merely because its current files appear beneath `~/.cursor`. Changing `HOME` would also affect child shells and unrelated tooling.

References:

- [Cursor CLI authentication](https://docs.cursor.com/en/cli/reference/authentication)
- [Cursor ACP documentation](https://prod.cursor.com/docs/cli/acp)

### 5.4 Internal launch contract

Change the managed internal launch from conceptually:

```text
--agent-process <provider> <project-root>
```

to:

```text
--agent-process <provider> <account-id> <project-root>
```

The child process must reload the account registry and verify that:

- The account exists.
- The account belongs to the supplied provider.
- The account id is valid and bounded.
- Its resolved directories remain beneath the expected provider root.

Test-only direct process launch may keep an explicit synthetic account seam so fake-agent tests do not require a real user registry.

## 6. Account-qualified controller and application state

Change account-sensitive state to:

```rust
selected_account: AccountKey,
account_agents: HashMap<AccountKey, AgentState>,
agent_controllers: HashMap<AccountKey, AgentController>,
```

`AgentController::start_with_wake` and managed session startup receive an `AccountKey` or a validated account profile. The shared ACP connection loop remains provider-neutral.

Scope these paths by account and project:

- Hidden-session history.
- Active-session restoration.
- Editur-created session ids.
- External session discovery roots.

Session ids from one account must never be loaded through another account's process.

Continue running only the selected account's controller. The map exists to retain account state consistently with the current provider state design, not to keep every account process alive.

## 7. Manual account management and switching

Extend the existing provider menu instead of introducing a separate account settings system:

```text
Codex
  ✓ Work account
    Personal account
    + Add account
Claude
    Team Max
```

Each account row shows:

- Account label.
- Current selection.
- Authentication/setup state.
- Whether it participates in automatic failover.
- Its position in failover priority.

Use provider name and account label in accessible widget labels; do not rely on icons or color.

Adding a provider-managed account should:

1. Create the registry entry and isolated provider-data directory.
2. Select the account.
3. Start the ordinary account-qualified provider process.
4. Reuse the existing ACP or terminal authentication UI.
5. Create a session after authentication succeeds.

Manual switching is permitted only when no turn, permission request, or interaction request is active. If work is active, keep account rows disabled and offer Stop or completion of the pending request first.

When a switch is accepted:

1. Preserve the unsent composer draft.
2. Shut down the old account process completely.
3. Swap to the target account's independent `AgentState`.
4. Persist the selected `AccountKey`.
5. Start the target process with its isolated environment.
6. Restore only that account's native project session.

Do not automatically carry a manually selected account's conversation into another account. Manual account sessions remain independent; automatic exhaustion handling has the explicit handoff path described below.

## 8. Structured plan-exhaustion detection

Add a provider-neutral failure classification before formatting errors for display:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TurnFailureKind {
    UsageExhausted { reset_at: Option<SystemTime> },
    Authentication,
    Transport,
    Other,
}

Event::TurnFailed {
    kind: TurnFailureKind,
    message: String,
}
```

Keep `TurnFinished` as the state transition that makes the session idle. The application should begin failover only after pending turn events are applied and the old account is no longer active.

### 8.1 Codex classifier

Classify usage exhaustion only when:

```text
error.data.codexErrorInfo == "usageLimitExceeded"
```

The pinned Codex ACP adapter waits for Codex's internal retry behavior and then returns this structured marker in the failed prompt request.

Reference: [Codex ACP v1.1.14 error handling](https://github.com/agentclientprotocol/codex-acp/blob/v1.1.14/src/CodexEventHandler.ts).

Do not infer exhaustion from token-usage notifications, a generic internal error, or HTTP status text.

### 8.2 Claude classifier

Preserve the latest `_meta._claude.rateLimit` object from Claude usage updates. Classify subscription-plan exhaustion only when:

- The latest structured rate-limit status is `rejected`.
- The terminal prompt error contains `data.errorKind == "rate_limit"`.

The pinned adapter forwards Claude SDK assistant error kinds as structured ACP error data and forwards SDK rate-limit state in usage-update metadata.

A `rate_limit` error without rejected plan state may be a transient API rate limit. It must not consume another account automatically.

Do not treat `billing_error`, `authentication_failed`, `overloaded`, or `max_output_tokens` as plan exhaustion.

### 8.3 Cursor classifier

Before enabling automatic Cursor failover:

1. Capture a sanitized ACP failure from the exact pinned Cursor binary when a real account exhausts its plan.
2. Identify a stable structured error field or exact versioned error shape.
3. Add the fixture to the fake agent and classifier tests.
4. Fail closed if the marker is absent or changes after an upgrade.

Do not release a classifier based on broad message fragments such as `limit`, `quota`, `usage`, or `429`.

### 8.4 Exhaustion state

When an account is proven exhausted:

- Exclude it from the current turn's remaining candidates.
- If the provider supplies a trustworthy reset timestamp, keep it unavailable until that time.
- Otherwise keep it unavailable until the user manually retries it or the application restarts.
- Display the status in the account menu.

Do not parse a reset time from arbitrary human-readable error text.

## 9. Automatic failover state machine

Automatic failover is enabled only when at least one other account for the same provider has `auto_failover` enabled.

Retain a small per-turn state:

```rust
struct FailoverAttempt {
    provider: ProviderId,
    attempted_accounts: HashSet<u64>,
    original_prompt: PromptEnvelope,
}
```

`PromptEnvelope` retains the original visible text and attachments only until the turn completes or exhausts all candidates. It must obey the existing prompt and attachment bounds.

On `UsageExhausted`:

1. Apply all failure and turn-finished events from the current controller.
2. Mark the current account exhausted and add it to `attempted_accounts`.
3. Select the first ordered account for the same provider that is enabled, not exhausted, and not already attempted.
4. Capture a bounded handoff from the visible transcript, including partial output and tool progress.
5. Shut down the exhausted account process.
6. Start the candidate account process.
7. If authentication is required or startup fails, record that candidate as attempted and continue to the next candidate.
8. Create a new native session on the first ready candidate.
9. Send the hidden continuation handoff with the original attachments that remain valid.
10. Preserve the visible transcript and add an account-switch divider.
11. Continue normal event processing on the destination native session.

The selector must never:

- Choose another provider.
- Choose an account twice for the same user turn.
- Switch on a generic failure.
- Start more than one replacement process at once.
- Retry indefinitely.

If no candidate succeeds, stop the turn, preserve the conversation, and show a clear action to retry or select an account manually.

## 10. Cross-account session continuation

### 10.1 Native-session boundary

Account-isolated provider data means account B usually cannot load account A's native session id. Provider session stores must not be copied or edited to work around that boundary.

Continuation therefore uses:

```text
Account A native session
        |
        | structured usage exhaustion
        v
bounded Editur handoff
        |
        v
Account B new native session
```

The visible Editur conversation remains continuous during the run, but its active provider-native session id changes to account B's new session.

### 10.2 Handoff content

Generalize the existing external-session handoff builder so it can consume either an imported transcript or the current `AgentState` transcript.

The live handoff should include, within the existing 64 KiB bound:

- User messages.
- Assistant messages, including partial output from the exhausted turn.
- Concise completed and in-progress tool descriptions and useful outputs.
- The fact that the prior account stopped because its usage plan was exhausted.
- An instruction to inspect the current workspace before editing and not repeat completed work.

It should omit:

- Thoughts and hidden reasoning.
- Secret question answers.
- Raw authentication or provider diagnostics.
- Binary data and encoded images.
- Redundant long tool logs that do not help continuation.

Retain and attach the original prompt's still-valid file or image attachments separately, subject to normal attachment capability and size checks.

### 10.3 Transcript behavior

Add a visible transcript item such as:

```text
Switched from Codex · Work to Codex · Personal after Work reached its usage limit.
```

The actual handoff prompt should use a versioned Editur marker. Live sending hides the synthetic prompt from the visible transcript. When `session/load` later replays it, the state reducer recognizes the marker and renders an account-switch/handoff card instead of a large raw user message.

The destination provider persists the handoff as part of its native session, so loading that destination session after restart restores enough context to continue. The initial implementation does not need a second logical-session database that joins every native account segment.

If future requirements demand exact restoration of the original pre-switch transcript and segment boundaries, add a bounded logical-session manifest only after this simpler design proves insufficient.

## 11. Implementation plan

### Phase 0: Provider account-contract fixtures

Before changing product state:

1. Record how the exact bundled Codex version behaves with two distinct `CODEX_HOME` directories and file-backed auth.
2. Record how the exact bundled Claude adapter behaves with two distinct `CLAUDE_CONFIG_DIR` directories, including terminal auth.
3. Verify Cursor API-key and auth-token accounts using separately named source environment variables.
4. Capture Cursor browser-login storage behavior without changing the user's real global login.
5. Capture sanitized usage-exhaustion errors and relevant rate-limit updates for each provider.

Exit criteria:

- Codex and Claude account A cannot see account B's credentials or sessions.
- Cursor environment accounts are isolated.
- Every enabled automatic classifier has a version-pinned fixture.
- Unproven Cursor browser isolation and exhaustion detection remain disabled rather than inferred.

### Phase 1: Account registry and path isolation

Tests first:

- Loading without an account file creates valid legacy accounts.
- Existing selected-provider preference migrates to the matching legacy account.
- Registry files are size-bounded, versioned, and reject unknown fields.
- Labels and environment-variable names are bounded and validated.
- Account ids cannot escape the provider directory.
- Account A and B produce distinct provider-data and session-metadata paths.
- The registry never serializes a credential value.

Then implement:

- `AccountKey`, `ProviderAccount`, and `AuthSource`.
- Atomic registry load/save and migration.
- Account-aware provider data and Editur session paths.
- Account-qualified internal process arguments.
- Provider launch environment construction.
- Explicit account roots for external-session discovery.

Exit criteria:

- Single-account users retain current behavior.
- Two synthetic accounts cannot collide in any account-owned path.
- Managed provider packages remain shared.

### Phase 2: Manual account selection

Tests first:

- Selecting account A starts only A's controller.
- A to B to A restores each account's independent `AgentState` and native session.
- Account switching is disabled during turns, permissions, and interactions.
- Provider and account selection persists across restart.
- An account authentication action reaches the selected account-qualified controller.
- Split Agent panes retain their own selected account state without sharing controllers.

Then implement:

- Account-qualified state and controller maps.
- Provider-menu account rows and Add account action.
- Account labels, ordering, and automatic-failover toggle.
- Safe account switching through the existing provider-switch lifecycle.

Exit criteria:

- Manual switching works without restarting Editur.
- Only the selected account process remains alive.
- Sessions never appear under another account.

### Phase 3: Structured failure classification

Tests first:

- Codex `codexErrorInfo: usageLimitExceeded` becomes `UsageExhausted`.
- Other Codex errors remain non-failover failures.
- Claude rejected plan state plus `errorKind: rate_limit` becomes `UsageExhausted`.
- Claude transient `rate_limit`, billing, authentication, overloaded, and generic errors do not.
- Context-window usage alone never triggers failover.
- Cursor remains fail-closed unless its versioned fixture is present.
- Existing Cursor transport resume still retries only the same account and session.

Then implement:

- `TurnFailureKind` and `TurnFailed`.
- Preservation of structured ACP error data.
- Preservation of relevant provider rate-limit metadata.
- Pure provider-specific classification helpers.
- In-memory account exhaustion state and trustworthy reset timestamps.

Exit criteria:

- No user-facing error formatting is used as a failover signal.
- Every enabled classifier has focused fixtures from the bundled adapter version.

### Phase 4: Automatic handoff and continuation

Extend `editur-fake-agent` with account-aware behavior:

- Account A emits partial assistant and tool updates, then a structured usage-exhaustion error.
- Account B accepts a new session and a tagged handoff, then completes the run.
- A candidate can report authentication required.
- Every candidate can be exhausted to test terminal failure.

Tests first:

- Failover selects accounts in configured order.
- Disabled, exhausted, unauthenticated, and previously attempted accounts are skipped.
- No account is attempted twice in one turn.
- The visible user prompt appears only once.
- The handoff is bounded and excludes thoughts and secret answers.
- Partial workspace changes remain visible to the replacement account.
- Attachments are retained only when still valid and supported.
- The account-switch divider appears once.
- Destination session id becomes active and is saved under the destination account.
- Reloading the destination session collapses the tagged handoff into a card.
- All-accounts-exhausted stops cleanly without a retry loop.

Then implement:

- Per-turn `FailoverAttempt` state.
- Generalized bounded handoff builder.
- Live transcript visitation including archived transcript pages.
- Hidden handoff prompt sending.
- Account-switch transcript item and replay normalization.
- Bounded candidate selection and startup failure handling.

Exit criteria:

- The fake-agent integration proves an interrupted run completes on account B.
- The destination session remains loadable after restart.
- Generic prompt failures never invoke the failover path.

### Phase 5: UI, release, and live verification

Automated checks:

- Keyboard navigation and accessible labels for account rows and actions.
- Clear states for selected, unauthenticated, exhausted, unavailable-until, and automatic-failover accounts.
- Confirmation and Stop behavior while work is active.
- No credential values in state snapshots, errors, stderr diagnostics, or test output.
- Process-tree teardown when switching accounts on every supported platform.

Manual checks against the exact bundled packages:

- Two Codex browser accounts.
- Two Claude subscription accounts.
- Cursor API-key or auth-token accounts.
- Cursor browser accounts only if Phase 0 proves safe isolation.
- Manual switching, exhaustion, reset, restart/load, cancellation, and all-accounts-exhausted behavior.

Run the normal format, lint, and test suite. Monitor this project's disk usage and remove only this project's Rust incremental build artifacts if `target` growth becomes material.

## 12. Acceptance criteria

The work is complete when:

1. Two accounts on the same provider do not share credentials or account-owned session metadata.
2. Existing users retain their current provider login and sessions as a legacy account.
3. Users can add, label, select, and order accounts without restarting Editur.
4. Manual account switching preserves independent per-account sessions.
5. Only a tested structured plan-exhaustion signal triggers automatic switching.
6. A transient 429, transport failure, authentication error, billing error, context-window limit, or arbitrary text does not trigger automatic switching.
7. An exhausted run can continue through a ready account on the same provider with prior context, partial output, attachments, and workspace changes available.
8. The visible transcript records the switch without duplicating the original user message or exposing the synthetic handoff.
9. No account is attempted more than once for a user turn.
10. Exhausting or failing every configured account stops with a recoverable UI state.
11. Destination native sessions remain loadable after restart.
12. Editur persists no provider credential values and does not copy provider auth files.
13. Only one ACP account process runs per Agent pane.
14. Existing provider selection, authentication, session history, Cursor transport resume, and buffer reconciliation behavior continue to pass their current tests.

## 13. Expected files and seams

The implementation should primarily touch:

- `src/agent/provider.rs`: account identity, registry, selected account, and provider launch environment.
- `src/agent/mod.rs`: account-qualified managed process launch.
- `src/agent/provision.rs`: account-root helper while leaving installations shared.
- `src/agent/controller.rs`: account-qualified session paths, structured turn failures, and hidden handoff sending.
- `src/agent/external_sessions.rs`: explicit account discovery roots and generalized bounded handoff builder.
- `src/agent/state.rs`: retained account state, handoff transcript visitation, and account-switch transcript items.
- `src/cli.rs` and `src/main.rs`: validated internal account id argument.
- `src/app.rs` and `src/app/agent_view.rs`: account-qualified maps, selection UI, manual switching, and failover orchestration.
- `src/bin/editur-fake-agent.rs`: versioned failure and two-account continuation fixtures.
- Existing controller, state, and application test modules: focused TDD coverage.

Do not add a provider trait, account service, credential database, background process pool, or logical-session database unless a proven requirement cannot be met through these existing seams.
