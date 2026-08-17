# Devin sidebar capability parity plan

## 1. Purpose

This plan closes the gap between Editur's current Devin sidebar and the developer-facing functionality exposed by Devin's documented MCP server and organization-scoped v3 API as of 2026-08-16.

It supplements `DEVIN_SIDEBAR_PLAN.md` and supersedes that plan's deliberate first-release exclusions for advanced session creation, batch work, playbooks, knowledge, schedules, and related developer workflows. The existing sidebar remains a cloud-Devin surface, separate from Editur's local ACP Agent.

## 2. Audit verdict

The current implementation is a useful session supervisor, but it is not at capability parity. It implements the narrow first-release slice it was originally designed for:

- secure credential entry and keyring storage;
- session listing and local filtering;
- minimal session creation with one repository and a prompt;
- status, message, attachment, and activity retrieval;
- follow-up messages and local file attachment staging;
- sleep, archive, unarchive, and terminate controls;
- polling, stale-response rejection, response bounds, backoff, and remote/local workspace separation.

There are two kinds of work left:

1. **Correctness work in the existing slice.** Current v3 field names, status semantics, attachment upload, and pagination are not represented faithfully enough to trust against the live service.
2. **Missing core functionality.** Most documented MCP resource tools, advanced session operations, and developer-facing v3 workflows have no sidebar route.

Do not start by adding new panels. First make the existing session path conform to captured live schemas and current v3 fixtures.

## 3. Sources and scope

Primary sources:

- [Devin MCP](https://docs.devin.ai/work-with-devin/devin-mcp)
- [Advanced Capabilities](https://docs.devin.ai/work-with-devin/advanced-capabilities)
- [Devin API overview](https://docs.devin.ai/api-reference/overview)
- [v3 API index](https://docs.devin.ai/llms.txt)
- [Create Session](https://docs.devin.ai/api-reference/v3/sessions/post-organizations-sessions)
- [List Sessions](https://docs.devin.ai/api-reference/v3/sessions/organizations-sessions)
- [Send Session Message](https://docs.devin.ai/api-reference/v3/sessions/post-organizations-sessions-messages)
- [Upload Attachment](https://docs.devin.ai/api-reference/v3/attachments/post-organizations-attachments)
- [Permissions and RBAC](https://docs.devin.ai/api-reference/v3/overview)

### In scope

Expose developer-facing organization functionality that is useful while working in an editor:

- complete session discovery, creation, inspection, interaction, lineage, and lifecycle;
- repository discovery, DeepWiki documentation, and repository Q&A;
- playbook and knowledge management;
- schedules and developer-owned automations;
- integration visibility and setup/configuration links;
- session insights and Devin Review;
- repository indexing and session environment resources needed to launch successful work;
- organization and session secrets through write-only, explicitly confirmed flows.

Every feature is permission-gated. A missing permission must remove or disable only that feature, not break connection to the rest of Devin.

### Out of scope

Do not turn the sidebar into an enterprise administration console. Exclude:

- enterprise organization, membership, role, service-user, API-key, IdP-group, IP-list, and org-group administration;
- billing, consumption, account metrics, audit logs, guardrail reporting, queue, hypervisor, and infrastructure administration;
- enterprise-wide Git permission administration and code-scan administration;
- Devin Desktop-only autocomplete, local Cascade/CLI, Spaces, and other surfaces without a supported MCP or v3 contract;
- embedded remote IDE, shell, browser, or desktop control until Cognition documents a supported client contract;
- automatic merge, checkout, or application of remote changes;
- silent upload of local diffs or secrets.

The sidebar may deep-link to the Devin web application for an unsupported interactive action such as approving work or taking over the remote browser.

## 4. Current parity matrix

| Capability | Current state | Required change |
| --- | --- | --- |
| Token/keyring authentication | Implemented | Preserve; add organization discovery/switching and per-org capability state. |
| Session list | Partial | Add server-side filters, current pagination fields, all documented origin/status semantics, and total/has-next metadata. |
| Session status triage | Incorrect for v3 | Derive semantic state from both `status` and `status_detail`; distinguish waiting for user, waiting for approval, finished, suspended reasons, failure, and resuming. |
| Session detail | Partial | Render full metadata, all PRs, tags, usage, mode, playbook, category, structured output, lineage, automation/schedule origin, and attachments. |
| Message history | Partial | Normalize current `source`, `event_id`, `created_at`, and `end_cursor`; separate historical pagination from refresh. |
| Send message | Implemented without a current contract fixture | Validate the exact MCP schema and use v3 `attachment_urls` when the REST fallback is used. |
| Attachment upload/download | Outdated/partial | Replace `/v1/attachments` and string-response assumptions with the organization-scoped v3 endpoint and typed response. |
| Session activity | Partial | Add summary paging, event detail retrieval, full-text event search, and rendering for event-linked PRs, children, recordings, and attachments. |
| Pull requests | Partial and schema-incompatible | Normalize `pr_url`/`pr_state`, render every PR, and keep opening external only. |
| Child sessions | Partial and schema-incompatible | Normalize `child_session_ids`, fetch child summaries, expose parent/child navigation, and support managed child creation. |
| Session tags | Read-only field, not rendered | Get, append, and replace tags from detail. |
| Minimal session create | Implemented | Keep as the default fast path. |
| Advanced session create | Missing | Add title, multiple repos, mode, playbook, child playbook, knowledge, tags, ACU limit, platform, resumability, attachments, links, structured output, and explicitly gated secret/impersonation options. |
| Batch create and gather | Missing | Create parallel sessions and use `devin_session_gather` for an explicit wait action, never for the normal UI poll loop. |
| Session search | Local text filter only | Expose documented tag, playbook, origin, schedule, user, parent, repository, category, and time filters. |
| Session insights | Missing | List/get/generate insights and show the result in session detail. |
| Repository documentation | Missing | Add repo listing, wiki structure/content, and `ask_question`. |
| Playbooks | Missing | Full list/get/create/update/delete and playbook selection during create. |
| Knowledge | Missing | Full note CRUD, folder browsing, filtering/search, and suggestion review/dismissal when advertised by MCP. |
| Schedules | Missing | Full list/get/create/update/enable-disable/delete with cron/one-time configuration, agent, and notification options. |
| Integrations | Missing | List native integrations and MCP servers, filter by install state, and open documented setup/settings URLs. |
| Automations | Missing | Organization v3 list/get/create/update/delete plus schema/template discovery. |
| Devin Review | Missing | Trigger review for a PR/MR and poll/display latest review status. |
| Repository indexing | Missing | List available/indexed repositories, show indexing state, and expose explicit index/remove actions. |
| Environment blueprints/builds | Missing | Manage org/repo blueprints and files, trigger/cancel builds, inspect logs, and pin/unpin successful builds. |
| Secrets | Missing | List metadata, create, and delete org secrets; allow selected secret IDs or ephemeral session secrets during creation without ever reading values back. |

## 5. Confirmed correctness gaps

### P0. Status classification defeats the sidebar's main triage job

`normalize::session` computes the semantic category from `status` alone. Current v3 sessions use combinations such as `running + waiting_for_user`, `running + waiting_for_approval`, `running + finished`, and `suspended + <reason>`. The existing mapping therefore places sessions that need the user in Working, may leave finished sessions messageable, and continues fast polling for terminal sessions.

Required fix:

- make semantic classification a table over `(status, status_detail, is_archived)`;
- treat `waiting_for_user` as Needs you;
- treat `waiting_for_approval` as a distinct Needs approval state with an Open Devin action;
- treat `running + finished` and `exit` as terminal success;
- distinguish suspended-idle from suspended quota/payment/error reasons;
- preserve raw values for display and forward compatibility;
- add one table-driven test covering every documented v3 value plus unknown values.

### P0. The transport tests describe a fake schema, not Devin's schema

The fake server invents shallow tool schemas and response bodies. It proves Editur's client and fake agree, but it does not prove compatibility with Devin. The original plan required an authenticated protocol spike and sanitized fixtures; no such fixtures are present.

Required fix:

- capture a sanitized `initialize` response and full `tools/list` output from the live MCP server;
- capture one sanitized success payload for every action Editur supports;
- preserve unions, nested `items` schemas, required fields, and enum values in fixtures;
- make request tests assert the complete `tools/call.arguments` object, not only the tool/action name;
- fail a fixture-refresh check when required documented operations disappear or change shape;
- never put credentials, signed URLs, organization names, user identities, prompts, or repository names in fixtures.

### P0. Current v3 response shapes are only partly normalized

The normalizer omits or mishandles current names including:

- `end_cursor` and `has_next_page`;
- message `source`;
- PR `pr_url` and `pr_state`;
- `child_session_ids` arrays of strings;
- `acus_consumed`, `devin_mode`, `category`, `subcategory`, `playbook_id`, `automation_id`, and `structured_output`;
- detailed suspended and failure reasons.

Required fix:

- use typed structs for stable v3 REST payloads;
- keep MCP normalization isolated at its boundary and driven by captured schemas;
- do not keep expanding generic alias lists as a substitute for contract fixtures;
- add one sanitized fixture test per response family: session, page, message, attachment, PR, insight, and event.

### P0. Attachment upload uses the legacy endpoint and response shape

The current transport posts to `https://api.devin.ai/v1/attachments` and decodes the body as a JSON string. The current organization API documents `POST /v3/organizations/{org_id}/attachments` returning an object with `attachment_id`, `name`, and `url`.

Required fix:

- introduce the organization-scoped v3 base path after org resolution;
- parse the typed attachment response;
- pass attachment URLs through documented create/message fields instead of embedding magic `ATTACHMENT:"..."` text unless a captured MCP schema explicitly requires that form;
- retain the current size limits, no-credential cross-host downloads, and redaction rules;
- preserve failed pending messages and staged local files for retry.

### P0. Pagination and refresh share one cursor

Selected-session polling calls the same message/event loaders used by the Load more buttons. The stored page cursor is advanced automatically by every refresh and can eventually return to `None`, causing a later poll to replace previously accumulated history. Historical paging and new-item refresh are different operations and need separate state.

Required fix:

- capture and document cursor direction for every live MCP action and v3 endpoint;
- maintain independent history cursors and refresh high-water marks where the service supports them;
- never advance a history cursor during background polling;
- never clear accumulated history merely because an end cursor is `None`;
- test initial load, two manual older pages, three no-op polls, a newly arriving item, and session switching.

### P1. The detail UI does not expose state it already owns

`SessionDetail` stores usage, PRs, children, and attachments, but the sidebar omits the planned metadata strip, shows only the first PR in the header, does not show unattached files, and does not render tags or usage. Activity rendering also ignores its attachment, PR, and child-session links.

Required fix:

- finish the existing detail contract before adding new resource screens;
- show raw status detail as visible text, not hover-only;
- add the waiting/approval callout above the composer;
- render all PRs and remote attachments in compact cards;
- render tags, ACUs, mode, playbook, origin, created/updated time, lineage, and structured output;
- disable impossible lifecycle actions based on semantic state.

### P1. One existing Devin UI regression test is failing

`cargo test devin` currently passes 34 Devin-filtered tests and fails `app::tests::devin_home_footer_holds_freshness_and_overflow_and_nothing_clips` at the PR-pill alignment assertion. `cargo test --test devin_state` passes all four tests.

Required fix:

- restore the 320 px list-row layout contract before accepting more sidebar controls;
- keep future advanced controls behind pushed views or disclosures so the minimum-width session list remains stable.

## 6. Target architecture

Keep the current concrete design; do not introduce a generic provider framework.

### 6.1 MCP transport

Use MCP for capabilities uniquely or most completely exposed there:

- repository wiki structure/content/Q&A and repository listing;
- session event summaries/detail/search;
- sleep/unarchive and tag operations when advertised;
- batch create/gather;
- playbook, knowledge, and schedule management;
- integration listing.

At connection time, build a `DevinCapabilities` value from `tools/list`. Core session browsing must remain usable if an unrelated tool is absent. Do not require `devin_session_gather` merely to connect when the UI is not using it.

Add explicit transport methods for documented actions. A single private `call_tool` helper remains sufficient; no trait, factory, or tool DSL is needed.

### 6.2 v3 organization client

Add one small synchronous v3 client, using the existing `ureq::Agent`, for documented developer workflows not covered by MCP and for typed fallbacks:

- identity/org discovery;
- sessions, messages, attachments, tags, and insights;
- Devin Review;
- repositories and indexing;
- automations;
- secrets;
- blueprints, builds, and logs.

Share authentication headers, timeouts, status mapping, response limits, retry guidance, and redaction with the MCP transport. Do not add another HTTP dependency or async runtime.

### 6.3 Organization and permission state

- Resolve org-scoped credentials automatically.
- For PATs and enterprise credentials, fetch the user's available organizations and require an explicit selection when there is more than one.
- Store the selected org ID beside the keyring credential, but never store returned role data as an authorization guarantee.
- Track feature capabilities per organization.
- Treat `403` as a feature-level permission failure; leave unrelated data and actions working.
- On org switch, clear org-owned remote state and cursors while preserving unsent local drafts until the user confirms the switch.

### 6.4 State boundaries

Split state by product resource, not protocol:

- sessions and selected-session detail;
- repositories/docs;
- playbooks;
- knowledge/folders/suggestions;
- schedules/automations;
- integrations;
- environment/secrets;
- reviews/insights.

Each resource gets loading, loaded, stale, forbidden, and failed states plus its own pagination. Do not add a generic normalized resource store.

Secret values are request-only values. They must never enter `DevinState`, `Debug`, toasts, persisted preferences, fixtures, or retry payloads after a request finishes. A failed secret submission keeps only the field names and requires re-entry of values.

## 7. Sidebar information architecture

Keep Sessions as the default home. Add a compact section switcher reachable from the Devin header/overflow:

- Sessions
- Review
- Repositories
- Knowledge
- Playbooks
- Automations
- Environment
- Integrations

This is one sidebar with pushed list/detail/editor views. Do not place every feature on the session home or add a second Devin sidebar.

### 7.1 Session home and search

- Keep Needs you first, then Needs approval, Working, Idle, and Done.
- Make Active include recent completed/failed outcomes long enough to review, or rename the existing scope so its behavior is unambiguous.
- Keep fast local filtering for already-loaded rows.
- Add a Filters view for server-side origin, repository, tags, playbook, schedule, user, parent, category, status, and created/updated ranges.
- Surface result count and active filter chips.
- Preserve filters and scroll position when visiting detail.

### 7.2 Create session

The first visible fields remain repository and prompt. Add an Advanced disclosure rather than making the common path longer.

Always available when supported:

- one or more repositories selected from Devin's available repository list;
- prompt, title, and initial attachments;
- agent mode;
- playbook;
- tags;
- maximum ACU limit.

Advanced:

- knowledge notes;
- child playbook and session links for managed sessions;
- structured-output schema plus required/optional toggle;
- VM platform/outpost placement;
- resumable/disposable session;
- selected organization secret IDs;
- ephemeral session secrets;
- create-as-user and bypass-approval only when explicitly permitted and clearly labeled.

Batch count belongs beside Create only when `devin_session_create` advertises multi-create. After a batch launch, show the created rows and offer Wait for all, backed by `devin_session_gather`.

### 7.3 Session detail

- visible semantic and raw status, including approval/quota/payment reasons;
- repository/origin/user/times/mode/playbook/category/ACU metadata;
- editable tags;
- parent and child navigation plus Launch child;
- all PRs and structured output;
- all messages and attachments with correct history paging;
- activity summary rows with Fetch details and Search activity;
- generated session insight with Generate/Refresh;
- state-valid message and lifecycle controls;
- Open in Devin for interactive browser/shell/approval actions.

### 7.4 Repository documentation

- list/search available repositories and show indexing state;
- browse wiki topics and render wiki contents as remote documentation;
- ask a question against up to ten selected repositories and render cited answers;
- provide explicit Index, Re-index, Remove index, and branch-removal actions where v3 permits them;
- never confuse remote wiki source paths with local clickable file paths.

### 7.5 Knowledge and playbooks

Use the same small list/detail/editor pattern for both.

Knowledge:

- folder tree, note counts, repository/folder filters, and search;
- get/create/update/delete note;
- list/view/dismiss suggestions when the MCP tool advertises them.

Playbooks:

- list/get/create/update/delete;
- automation macro field where advertised;
- open related sessions and choose a playbook from session creation.

All delete actions require confirmation. Preserve editor drafts on request failure.

### 7.6 Automations and integrations

Automations contains two subviews:

- Schedules: recurring cron and one-time schedules, enabled state, agent, notification preferences, create/update/delete.
- Event automations: list/get/create/update/delete using server-provided trigger schemas and templates. Do not hard-code third-party trigger forms when the API already describes them.

Integrations is read-only inside Editur:

- list native integrations and MCP servers;
- filter installed/not installed/all;
- open the returned setup or settings URL in the system browser;
- do not implement OAuth callbacks or store third-party credentials.

### 7.7 Review and environment

Review:

- enter or select a PR/MR URL;
- trigger Devin Review after confirmation if it incurs work/usage;
- show latest review status and link to the result;
- show session insights in the related session, not as a duplicate analytics dashboard.

Environment:

- repository and organization blueprints with a text editor for YAML contents;
- blueprint file list/upload/delete;
- build list/detail, trigger, cancel, logs, pin, and unpin;
- organization secret metadata plus create/delete;
- never reveal an existing secret value or put secret values in a reusable draft.

## 8. Implementation phases

### Phase 0: restore a trustworthy session foundation

1. Capture sanitized live MCP schemas and payloads.
2. Add current v3 response fixtures.
3. Fix status/detail classification.
4. Fix current field normalization for cursors, messages, PRs, children, and usage.
5. Replace the v1 attachment upload path with v3.
6. Separate background refresh from manual history pagination.
7. Fix the failing minimum-width UI test.
8. Run a real credential smoke test for list, detail, message with attachment, sleep, archive/unarchive, and terminate.

Exit condition: the existing advertised sidebar functions work against the live service and all Devin tests pass.

### Phase 1: complete session parity

1. Add org selection and capability discovery.
2. Add server-side search/filter controls.
3. Complete the detail metadata/PR/attachment/tag/structured-output UI.
4. Add advanced creation fields with the minimal form as default.
5. Add event details/search, tag mutation, child launch, batch create, and gather.
6. Add session insights.

Exit condition: every operation described by the five session MCP tools and organization session/attachment/insight endpoints is reachable or explicitly marked unavailable by permission.

### Phase 2: add MCP resource parity

1. Repository documentation and Q&A.
2. Playbook CRUD.
3. Knowledge CRUD, folders, filters, and suggestions.
4. Schedule CRUD.
5. Integration inventory and links.

Exit condition: every tool advertised on the official Devin MCP page has a focused sidebar route and contract test.

### Phase 3: add developer-facing v3 workflows

1. Devin Review.
2. Event automations with schema/template discovery.
3. Repository discovery and indexing management.
4. Blueprint and snapshot-build management.
5. Organization and session-secret flows.

Exit condition: the in-scope organization API rows in Section 4 are reachable and permission-gated.

### Phase 4: release hardening

1. Cross-platform keyring and file-upload verification.
2. Keyboard/accessibility passes for every new list/detail/editor view.
3. Payload, attachment, markdown, YAML, and JSON-schema size limits.
4. Redaction tests for credentials, signed URLs, secrets, prompt content, and remote error bodies.
5. Offline/rate-limit/stale-data behavior per resource.
6. Authenticated smoke matrix for org-scoped service keys, enterprise keys, and PATs.
7. Documentation updates and screenshots at 320 px and 440 px widths.

Exit condition: all automated tests pass, the authenticated smoke matrix passes, and no feature failure disables unrelated Devin functionality.

## 9. TDD and verification

Follow red-green vertical slices. Keep tests small and behavioral.

Required automated checks:

- table-driven v3 status semantics;
- exact MCP request shape against sanitized live `tools/list` fixtures;
- typed v3 request/response fixtures for every used endpoint family;
- cursor state machine for history versus polling;
- capability/permission gating;
- secret and signed-URL redaction;
- stale response rejection during session/org switches;
- one UI interaction test per new view, plus 320 px overflow coverage;
- one fake-server end-to-end test per phase, not one test per passive field.

Required authenticated checks use a disposable test organization and data:

- org discovery and switch;
- repository discovery and docs query;
- create one minimal, one advanced, one child, and one two-session batch;
- message with attachment and automatic resume from suspended;
- waiting-for-user and waiting-for-approval triage;
- event detail and search;
- tag update, insight generation, sleep, archive/unarchive, and terminate;
- temporary playbook, knowledge note, schedule, automation, secret, blueprint, build, and review, each cleaned up through its documented API;
- no credentials, secret values, signed URLs, or private content in logs or fixtures.

Do not make live credential tests part of ordinary CI.

## 10. Definition of done

Capability parity is complete when:

- the current session workflow is validated against captured live MCP schemas and current v3 fixtures;
- every documented MCP tool has a usable, permission-aware sidebar route;
- all in-scope developer-facing organization API capabilities in Section 4 are available without opening Devin's web app;
- unsupported interactive remote controls deep-link to Devin with an honest explanation;
- enterprise administration remains outside the sidebar;
- session triage correctly identifies user input, approval, completion, suspension, and failure states;
- pagination cannot lose or duplicate history;
- local files, buffers, Git state, and remote Devin state remain separate;
- secret values and credentials never enter persistent or observable application state;
- all Devin-targeted tests and the authenticated smoke matrix pass.

## 11. First implementation slice

Start with one small, root-cause slice:

1. Add a sanitized current v3 session fixture containing `running + waiting_for_user`, `end_cursor`, a `source` message, `pr_url`/`pr_state`, and `child_session_ids`.
2. Make the fixture fail the current normalizer and state behavior.
3. Correct those mappings and the separate history cursor behavior.
4. Render the waiting callout and every PR from that fixture.
5. Re-run all Devin tests and the 320 px layout test.

This repairs the sidebar's highest-value promise—showing what needs the user—before expanding its surface area.
