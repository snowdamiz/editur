# Codex ACP integration plan

## 1. Purpose and reader

This is the Codex-specific follow-on to the [multi-provider ACP refactor plan](ACP_PROVIDER_PLAN.md). It is for an Editur contributor taking the existing provider boundary from “Codex-shaped” to a releasable Codex integration.

After reading this plan, the contributor should be able to pin, package, launch, authenticate, exercise, switch away from, repair, and release Codex through the canonical ACP adapter without adding a second controller or weakening Editur's supply-chain rules.

Existing code may already satisfy some items. Treat it as a candidate implementation: prove each exit condition with a focused test or smoke result before checking it off.

## 2. Definition of done

The Codex integration is complete only when:

- A release build containing Cursor and Codex shows the provider selector in both Agent layouts.
- Selecting Codex displays its terms notice, provisions only the pinned private package, and starts it without `npx`, a global Node.js installation, or a mutable package tag.
- The managed launch cannot be redirected through `CODEX_PATH`, `NODE_OPTIONS`, `NODE_PATH`, or another executable override.
- ChatGPT browser login works through the agent-owned ACP authentication method.
- API-key login works only from the user's environment or provider-owned credential store. Editur never asks for, receives, logs, or persists the key.
- Standard ACP sessions, streaming, tools, plans, permissions, cancellation, images, usage, model/mode/configuration controls, and history render through the shared controller and state reducer.
- Switching to or from Codex stops the old process tree first and never carries a session id, permission, interaction, transcript, capability, or diagnostic across providers.
- All release targets pass package verification and process teardown checks.
- A real-account smoke run is recorded for every supported target before stable promotion.

## 3. Scope

### 3.1 Included

- Canonical `@agentclientprotocol/codex-acp` integration over stdio ACP.
- A pinned adapter, compatible pinned Codex dependency, and private runtime.
- Cursor-to-Codex and Codex-to-Cursor selection, persistence, and switching.
- ChatGPT login and environment-owned API-key authentication as advertised by the adapter.
- Capability-driven session history, model, reasoning, fast-mode, sandbox, approval, tool, plan, image, review, and MCP presentation where these arrive through standard ACP.
- Provider-scoped installation, verification, cache, diagnostics, and hidden-session state.
- Native macOS Apple Silicon, macOS Intel, Linux x86_64, and Windows x86_64 release artifacts.

### 3.2 Excluded

- Direct Codex App Server integration in Editur. The canonical adapter owns that translation boundary.
- An Editur API-key form, credential vault, or environment editor.
- Global Node.js installation, `npx`, remote install scripts, or `PATH` mutation.
- User-selectable adapter implementations under the Codex provider id.
- Codex-only transcript rendering or a second controller.
- Namespaced Codex extensions until a captured fixture proves standard ACP loses required information.
- Codex cloud task orchestration, remote environments, or simultaneous local providers.
- Experimental App Server or adapter methods solely because they exist upstream.

## 4. Prerequisite and current snapshot

The shared work in [ACP_PROVIDER_PLAN.md](ACP_PROVIDER_PLAN.md) is a prerequisite: closed provider catalog, generic controller, provider bundle, provider-scoped storage, safe switching, capability-driven UI, and provider-gated extensions.

Snapshot checked 2026-08-09:

| Concern | Candidate state | Required audit |
| --- | --- | --- |
| Provider catalog | Codex is lazy and selectable when packaged | Verify the selector is absent in Cursor-only builds and present in two-provider bundles |
| Adapter | `@agentclientprotocol/codex-acp` `1.1.14` | Pin the exact upstream commit and archive inputs |
| Codex dependency | `@openai/codex` `0.147.0` from the adapter lockfile | Verify the installed dependency's own version probe |
| Runtime | Private Node.js `22.22.0` | Verify official runtime checksum, license inclusion, target, and executable mode |
| Package | Deterministic ZIP produced in release CI | Verify the exact extracted file set and both version probes |
| Provisioning | Lazy, provider-scoped, format-v2 bundle | Reject command, path, host, checksum, platform, and version drift |
| Authentication | Shared ACP auth UI preserves method kind | Prove ChatGPT and environment-key behavior against the pinned adapter |
| Controller | Standard ACP shared with Cursor | Prove Codex fixtures do not need a vendor branch |
| Release smoke | Package probe automated | Real auth, paid prompt, edits, switching, and teardown remain release gates |

Do not preserve a candidate behavior merely because it exists. The acceptance criteria in this document win.

## 5. Pinned integration decision

### 5.1 Adapter

Use the canonical [`agentclientprotocol/codex-acp`](https://github.com/agentclientprotocol/codex-acp) adapter. The initial audited pin is:

- Package: `@agentclientprotocol/codex-acp`
- Version: `1.1.14`
- Commit: [`5faefec5d55ded33c54b68ffec93def4f6c547f5`](https://github.com/agentclientprotocol/codex-acp/commit/5faefec5d55ded33c54b68ffec93def4f6c547f5)
- License: Apache-2.0

This is an ACP ecosystem adapter that translates standard ACP calls to the Codex App Server. It is not a native ACP server maintained solely by OpenAI. Keep that distinction in user-facing terms and support documentation.

Do not silently replace it with a community fork or a direct App Server client. A replacement requires a new compatibility and distribution review against this plan.

### 5.2 Runtime distribution

For the first release, build the exact adapter commit with its committed lockfile and package it with:

- The built adapter entrypoint.
- Its package metadata, README, and license.
- The exact production `@openai/codex` dependency selected by the lockfile.
- A private, exact Node.js runtime and its license.

The initial audited pins are `@openai/codex` `0.147.0` and Node.js `22.22.0`.

The upstream adapter can build standalone executables, but those are not the default until CI can prove their runtime provenance, licenses, checksums, platform behavior, size, and teardown characteristics as strongly as the private Node package. Do not mix both distribution shapes under one release pin.

### 5.3 Upgrade rule

An upgrade changes the adapter version, adapter commit, Codex dependency, runtime, or package layout only through one reviewed change. The change must regenerate every target artifact, refresh checksums, rerun the full automated and manual matrix, and document capability or authentication differences.

No runtime component may update itself inside Editur's managed directory.

## 6. Architecture and launch contract

```text
Provider selector
      |
      v
shared AgentController + ProviderId::Codex
      |
      v
prepared managed launch
private Node -> codex-acp -> bundled Codex -> Codex App Server
      |
      v
standard ACP commands, events, permissions, and session capabilities
```

The provider layer owns the exact executable, entrypoint, environment policy, package verification, display metadata, and terms policy. The shared controller owns ACP initialization, authentication requests, sessions, prompts, cancellation, permissions, normalized events, and shutdown.

The prepared Codex launch must:

- Execute the managed private Node binary by absolute path.
- Pass the managed adapter entrypoint by absolute path.
- Use the selected project root as the working directory.
- Use a Codex-provider cache directory for Node's compile cache.
- Leave the adapter on its bundled `@openai/codex` resolution path.
- Remove `CODEX_PATH`, `NODE_OPTIONS`, and `NODE_PATH` so the managed package cannot be redirected or preloaded.
- Remove `APP_SERVER_LOGS` by default because upstream debug logs can include protocol payloads. A future diagnostic opt-in must be bounded, provider-scoped, and disclosed.
- Inherit provider-owned auth and network inputs needed for normal operation, including existing Codex credentials, `OPENAI_API_KEY`, `CODEX_API_KEY`, proxy variables, and trusted CA variables such as `CODEX_CA_CERTIFICATE`, `SSL_CERT_FILE`, and `NODE_EXTRA_CA_CERTS`, without copying their values into Editur state or logs.
- Preserve the user's `CODEX_HOME` behavior so Codex owns its authentication and configuration. Editur must not read its credential files.
- Make no global environment or filesystem changes.

The launcher must contain the entire descendant tree. Closing Agent, switching provider, application exit, initialization failure, or a broken stdout stream must terminate the adapter and its Codex child on every target.

## 7. Authentication contract

Codex supports ChatGPT login and API-key login for local work. The adapter advertises its supported methods during ACP initialization; Editur renders only what the pinned adapter actually returns.

### 7.1 ChatGPT login

- Render ChatGPT as an agent-owned `Connect Codex` action.
- Send the advertised ACP authentication method id unchanged.
- Let the adapter and Codex open and complete the browser flow.
- Keep the connection UI responsive while authentication is pending.
- Retry session creation after a successful authentication response.
- Never inspect, proxy, or persist browser tokens.

### 7.2 API-key login

- Editur does not render a secret field.
- If the adapter advertises standard environment authentication, show the required variable names but never their values.
- If the pinned adapter represents API-key auth as an agent-owned method while actually requiring an environment value, normalize that exact, tested metadata in the provider boundary or block the method until upstream advertises it safely.
- Accept `CODEX_API_KEY` first and `OPENAI_API_KEY` as the provider's fallback, matching the pinned adapter contract.
- A missing environment credential produces setup guidance, not a blank browser-style button and not a prompt for the key.

### 7.3 Unsupported auth

Custom gateway auth, device-code URL elicitation, terminal auth, or a new method remains hidden or explicitly unsupported until Editur advertises and implements the required standard ACP client capability. Never send partial or fabricated authentication metadata.

### 7.4 Credential and privacy boundary

Editur must not read Codex auth files, echo auth-related environment values, include secrets in diagnostics, or copy provider logs into the transcript. Authentication persistence and refresh remain Codex responsibilities.

## 8. ACP capability mapping

Start from standard ACP. The pinned adapter is expected to cover text prompts, embedded context, images, resource links, sessions, model and mode configuration, plans, tools, file changes, terminal output, permissions, usage, reviews, subagents, and MCP activity.

For each initialization or session response:

- Show History only when both list and load are advertised and verified.
- Populate model, reasoning, fast-mode, sandbox, approval, and other configuration controls only from session config options.
- Preserve the exact permission choices and outcomes supplied by the adapter.
- Normalize streamed text, thought, plan, tool, diff, image, terminal, usage, and completion events through existing event types.
- Keep Cursor's `run-everything` behavior hidden for Codex unless a standard, tested equivalent is advertised.
- Ignore unknown namespaced metadata safely.
- Do not enable adapter experimental APIs or implement `_meta.codex.*` behavior in the first release.

Capture a sanitized initialization/session fixture from the pinned adapter and retain it as the compatibility contract. It must contain no account ids, paths outside a temporary project, prompt content, or credentials.

## 9. Session, configuration, and switching behavior

- Codex sessions remain Codex-owned; Editur persists only the selected provider and its own hidden-session removals.
- Hidden-session state is namespaced by Codex and project.
- A Codex session id is never loaded through Cursor or copied into a new provider state.
- Provider-advertised models, modes, config options, auth status, usage, commands, and diagnostics are live state and are cleared on switch.
- The unsent composer draft survives switching but is never submitted automatically.
- A non-empty transcript requires confirmation before leaving Codex.
- Switching is disabled during a turn, permission request, or interaction request.
- The UI stays in `Switching to ...` until the old process tree is gone.
- A failed Codex installation, authentication, or connection keeps Codex selected so Retry is meaningful.
- Switching back to Cursor follows the same lifecycle; it does not revive an old process.

If the pinned adapter does not reliably list and load project sessions, hide History and create a new session. Do not emulate history from Editur transcripts.

## 10. Provisioning and supply-chain contract

### 10.1 Build inputs

CI must fetch only:

- The exact adapter commit.
- The exact package dependencies resolved by its committed lockfile.
- The exact target Node.js archive verified against the official checksum list.

Build with lifecycle scripts disabled unless a reviewed package requires a specific audited script. Do not resolve `latest`, a semver range without the lockfile, or an unpinned Git ref.

### 10.2 Deterministic package

The target archive must have stable ordering, timestamps, path separators, and executable modes. Reject symlinks, path traversal, duplicate paths, special files, and undeclared content.

The generated manifest pins:

- Provider id and adapter version.
- Target operating system and architecture.
- Exact archive URL, size, checksum, and format.
- Exact command and entrypoint.
- Exact extracted files, kinds, sizes, checksums, and executable flags.
- Compression, extraction, and entry-count limits.
- Adapter license and OpenAI terms URLs.
- Version probe command and expected output.

### 10.3 Runtime verification

Before embedding a provider bundle, CI must extract the finished archive through the same provisioner used by the application and execute:

1. The adapter `--version` probe, expecting the pinned adapter version.
2. The bundled Codex `--version` probe, expecting the lockfile version.
3. A no-network launch fixture that proves the private Node runtime can load the adapter and locate the bundled Codex package.

Reject a package if any probe resolves through the host `PATH`, a global module directory, or an undeclared file.

### 10.4 Distribution and updates

- Publish provider archives under Editur's pinned provider release namespace.
- Allow only the exact GitHub release host and its documented release-asset redirect host.
- Attest the archive and provider bundle produced by the native release job.
- Keep Cursor in every format-v2 bundle; Codex is optional and lazy.
- Bootstrap installers continue provisioning Cursor only.
- Self-update validates the new bundle and repairs Cursor; Codex repairs lazily on next selection.
- Removing Codex from a future bundle hides it without recursively deleting its managed files, sessions, configuration, or credentials.

## 11. UI contract

When the embedded bundle contains Cursor and Codex:

- The IDE Agent header begins with `[Codex ⌄]` when Codex is selected.
- The full Agent session rail uses the same provider identity control.
- The provider menu shows the catalog-owned icon, name, short description, selected check, and `Installs on first use` status.
- Model, reasoning, mode, sandbox, and approval selectors remain in the composer and update only after the Codex session is ready.
- First selection opens a terms notice with adapter license and OpenAI terms links before download.
- Provisioning reads `Installing Codex Agent`; connection and auth copy use Codex metadata.
- Unsupported controls disappear rather than being disabled placeholders.
- Disabled provider rows explain that the user must stop or answer the active request first.
- All rows remain keyboard reachable and expose a complete accessibility label.

A Cursor-only or empty development bundle must not show a fake Codex selector.

## 12. TDD and verification strategy

Use one meaningful red-green cycle per non-trivial behavior. Reuse the fake ACP process and generic state/UI tests; do not clone the entire controller suite for Codex.

### 12.1 Provider and launch tests

- Codex catalog metadata is complete and has no vendor extension enabled.
- A prepared Codex launch uses only managed absolute command and entrypoint paths.
- The launch removes `CODEX_PATH`, `NODE_OPTIONS`, `NODE_PATH`, and `APP_SERVER_LOGS` even when the parent environment sets them.
- Credential variable names may reach the child while their values never appear in events, errors, receipts, or snapshots.
- Codex uses its provider-scoped cache and managed root.
- Cursor and Codex managed paths, locks, receipts, and active markers cannot collide.

### 12.2 Manifest and package tests

- The exact release specification generates a valid Codex manifest for every target.
- Wrong host, redirect, command, entrypoint, platform, checksum, file set, executable bit, or version response is rejected.
- Deterministic packaging produces identical bytes from identical staged content.
- Archive extraction rejects traversal, links, special files, extra entries, oversized data, and missing files.
- Both adapter and Codex dependency probes execute from the extracted managed tree.
- A corrupt installation repairs without changing another provider.

### 12.3 ACP controller tests

- The same generic prompt, stream, tool, permission, cancellation, and shutdown flow passes for Cursor and Codex provider ids.
- The sanitized Codex initialization fixture produces the expected history and configuration capabilities.
- Cursor extensions are ignored for Codex.
- Unknown Codex metadata is ignored without disconnecting.
- ChatGPT auth remains agent-owned.
- API-key auth is environment-owned or blocked with setup guidance; it never produces a secret input.
- Malformed stdout, bounded stderr, adapter exit, and Codex child exit become provider-aware connection failures.

### 12.4 State and UI tests

- The provider selector requires at least two packaged providers.
- Codex identity, description, lazy-install status, terms, and connection copy are visible and unclipped.
- Provider selection remains distinct from model and mode selection.
- Switching confirmation, blocking, draft preservation, state clearing, and shutdown ordering are enforced.
- History and configuration controls follow advertised capabilities.
- A failed Codex start remains selected and exposes Retry and provider switching.

### 12.5 Process tests

- Unix shutdown leaves no adapter or Codex descendant.
- Windows assigns the complete tree to one kill-on-close job object.
- Closing stdin, cancelling initialization, switching, and application exit all converge on the same teardown result.
- A hung child is bounded by the existing shutdown timeout and cannot keep Editur alive.

## 13. Manual smoke matrix

Run with a disposable account. CI must not receive real credentials or spend paid tokens.

For each supported target:

1. Start from a clean Editur data directory with Cursor already available.
2. Open Agent and verify no Codex work happened before this point.
3. Select Codex, review terms, provision, and record archive and installed sizes.
4. Complete ChatGPT browser authentication without credentials passing through Editur.
5. Repeat with an environment-provided API key when that method is supported; confirm the key never appears in UI or logs.
6. Record protocol version, auth methods, capabilities, models, modes, and config options.
7. Create a session, stream a response, and send a same-session follow-up.
8. Exercise a plan, tool, terminal output, image input, file edit, exact allow/reject permission, cancellation, and review when advertised.
9. Verify buffer reconciliation and project refresh.
10. List/load history when advertised; otherwise verify the control is absent.
11. Switch to Cursor, confirm Codex descendants exit, switch back, and verify provider/session isolation.
12. Corrupt one managed Codex file and verify lazy repair.
13. Exit Editur and confirm no adapter or Codex descendant remains.

Record time to first connection, time to warm connection, unsupported features, auth method, package sizes, and any adapter diagnostics. Redact account, project, prompt, path, and credential data.

## 14. Implementation phases

### Phase 0: Audit the candidate integration

- Trace the selected-provider path from Agent open through package preparation, managed launch, ACP initialization, and teardown.
- Compare the pinned adapter fixture and package layout with the candidate manifest, tests, UI, and CI workflow.
- Produce a checklist of already-proven behavior and missing evidence.

Exit condition: every later phase item is either proven by a named check or remains explicitly open.

### Phase 1: Lock the managed launch

- Add a failing test for executable/runtime environment overrides.
- Make the provider-owned launch environment deterministic while retaining approved credential and network variables.
- Prove absolute managed command/entrypoint resolution and provider-scoped cache behavior.

Exit condition: a hostile parent environment cannot redirect or preload the managed Codex runtime.

### Phase 2: Close authentication behavior

- Capture the exact auth methods advertised by the pinned adapter.
- Prove ChatGPT browser login through standard ACP.
- Normalize or block API-key auth so Editur never presents a secret field or a misleading browser action.
- Keep unsupported gateway, device-code, terminal, and experimental methods unavailable.

Exit condition: every visible auth action has a tested completion path and no credential crosses Editur state.

### Phase 3: Prove standard ACP compatibility

- Add the sanitized pinned-adapter fixture.
- Run shared command/event/state behavior for Codex.
- Verify capability-driven history and configuration controls.
- Keep namespaced Codex metadata ignored until a separate justified slice.

Exit condition: Codex requires no transcript fork, controller fork, or vendor extension for the release baseline.

### Phase 4: Harden packaging and release

- Reproduce every native provider archive from the exact commit, lockfile, and Node archive.
- Run deterministic archive, manifest, extraction, adapter-version, Codex-version, and no-global-runtime probes.
- Embed Cursor and Codex manifests in the format-v2 provider bundle.
- Publish and attest provider archives with the application artifacts.

Exit condition: every native release binary contains an attested bundle that can lazily install its matching Codex archive.

### Phase 5: Complete UI and switching

- Verify the provider selector, first-use terms, provider metadata, capability controls, failure states, and accessibility in both layouts.
- Prove old-process shutdown precedes new-process launch.
- Prove state, sessions, diagnostics, and managed files remain isolated.

Exit condition: a user can move between Cursor and Codex without process, state, or session leakage.

### Phase 6: Release gate

- Run the full automated suite and native packaging jobs.
- Complete and record the real-account smoke matrix.
- Recheck adapter license, OpenAI terms, dependency lock, archive checksums, authentication behavior, and update behavior.
- Exercise rollback by publishing a Cursor-only candidate bundle.

Exit condition: Codex can be promoted or removed without disabling Cursor or corrupting provider data.

## 15. Acceptance criteria

- [ ] Codex uses the canonical pinned adapter and its exact locked Codex dependency.
- [ ] The private runtime and every managed file are pinned, verified, licensed, and attested.
- [ ] Editur never invokes `npx`, a global Node installation, a mutable tag, or a remote install script.
- [ ] `CODEX_PATH`, `NODE_OPTIONS`, `NODE_PATH`, and equivalent execution overrides cannot redirect the managed launch; `APP_SERVER_LOGS` is off by default.
- [ ] No Codex provisioning, installation inspection, process, or repaint loop starts before Agent is opened.
- [ ] ChatGPT auth completes through the provider-owned browser flow.
- [ ] API-key auth is environment-owned and no secret enters Editur UI, state, logs, or persistence.
- [ ] Standard ACP controller, events, transcript state, and buffer reconciliation remain shared.
- [ ] Codex has no vendor extension enabled for the first release.
- [ ] History, configuration, model, mode, sandbox, approval, and permission UI follow advertised capabilities.
- [ ] Cursor-only builds hide the selector; Cursor-plus-Codex builds show it in both layouts.
- [ ] Terms acceptance precedes first Codex download.
- [ ] Codex installation, cache, receipts, hidden sessions, configuration, and diagnostics are provider-scoped.
- [ ] Switching is blocked during active requests, stops the old tree first, preserves only the draft, and clears provider-bound state.
- [ ] A failed Codex start stays selected and can Retry or switch back.
- [ ] Automated tests require no account, networked prompt, or paid token.
- [ ] Every supported target passes package probes, native teardown, and the manual provider smoke matrix.
- [ ] A Cursor-only rollback bundle remains functional and leaves Codex-owned data untouched.

## 16. Risks and stop conditions

| Risk | Default response | Revisit when |
| --- | --- | --- |
| Adapter or Codex dependency is not exactly reproducible | Block the release | Exact source, lockfile, integrity, and artifacts reproduce on every target |
| Managed launch can honor an executable or preload override | Block the release | The override is removed and covered by a child-environment test |
| API-key auth requires Editur to collect the key | Block that method | The adapter advertises environment auth or a safe provider-layer normalization is tested |
| Browser auth exposes tokens to Editur | Block Codex | The adapter completes login without token transport through the client |
| Adapter needs a global runtime, `npx`, or mutable download | Block Codex | A pinned private or standalone runtime passes the full package contract |
| App Server schema drifts from the adapter | Keep the last known-good pin | A reviewed adapter upgrade passes fixtures and smoke tests |
| Adapter self-updates managed files | Block Codex | A supported immutable/update-disabled path exists |
| Native package lacks the matching Codex executable | Block that target | The extracted dependency version probe passes natively |
| Process teardown leaves a descendant | Block that target | Native containment and exit checks pass repeatedly |
| Session list/load is unreliable | Hide History | The adapter advertises and passes stable project-scoped list/load behavior |
| Namespaced metadata appears useful | Ignore it | A fixture proves standard ACP loses material information and a provider-gated extension is reviewed |
| Package or installed size exceeds release targets | Keep Codex lazy or block the target | Measurements justify the artifact or an equally safe smaller distribution exists |
| License, terms, or data handling changes | Remove Codex from the bundle | Legal and product review approves the new terms |

## 17. Reference snapshot

Checked 2026-08-09:

- [Multi-provider ACP refactor plan](ACP_PROVIDER_PLAN.md)
- [Canonical Codex ACP adapter](https://github.com/agentclientprotocol/codex-acp)
- [Pinned adapter commit](https://github.com/agentclientprotocol/codex-acp/commit/5faefec5d55ded33c54b68ffec93def4f6c547f5)
- [OpenAI Codex repository](https://github.com/openai/codex)
- [Codex authentication](https://developers.openai.com/codex/auth)
- [Codex App Server](https://developers.openai.com/codex/app-server)
- [Agent Client Protocol](https://agentclientprotocol.com/)

Versions, capabilities, auth methods, package layout, and terms are release inputs. Re-pin and re-verify them for every Codex upgrade; never discover or adopt them dynamically at runtime.
