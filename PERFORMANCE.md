# Performance baseline

Measured 2026-08-05 from the stripped `--release` build on a MacBook Air with an Apple M4, 16 GB RAM, and macOS 26.5.2. Times are emitted by `EDITUR_LOG=debug`; resource usage was sampled after the window settled.

## Native application

| Metric | Result | Target | Status |
| --- | ---: | ---: | --- |
| First-process command-to-editable-window, warm system | 181.00 ms | 150 ms | Near; down from 2.35 s |
| Running-process open-request handoff, 10-run median | 10.2 ms | 25 ms | Pass |
| Running-process open-request handoff, 10-run p95 | 17.3 ms | 50 ms | Pass |
| Path resolution | 0.15 ms | — | Recorded |
| Native borderless window creation | 45.91 ms | — | Recorded |
| Warm Metal initialization | 3.26 ms | — | Recorded |
| Idle CPU | 0.1% | <1% | Pass |
| Memory with `PLAN.md` | 73.4 MiB | 60 MiB | Miss |
| Stripped arm64 binary | 9,189,360 bytes (8.76 MiB) | 30 MiB | Pass |

The startup path creates a new process, AppKit window, and Metal renderer; it does not keep a hidden resident window. The remaining 31 ms gap to the 150 ms target includes process launch, project discovery, first layout, and presentation. Continuous macOS releases fail unless the Metal shader library is precompiled; this machine lacks the optional command-line Metal toolchain, so the precompiled release path is compile-checked in CI rather than timed locally. Windows/D3D12 and Linux/Vulkan runtime baselines still require their native release runners.

## ACP Agent integration

Measured 2026-08-06 on the same Apple M4 reference machine. The comparison used five alternating warm `--resident` runs of the unmodified `HEAD` build and the manifest-embedded ACP build, opening the same file with the Agent view unopened.

| Metric | Baseline | ACP build | Change / target |
| --- | ---: | ---: | ---: |
| First editable frame, 5-run median | 60.95 ms | 61.12 ms | +0.17 ms / ≤5 ms |
| Idle CPU, Agent unopened | 0.1% baseline | 0.0% point sample | No measurable regression |
| Resident memory, Agent unopened | — | 45.2–47.3 MiB (5 samples) | Recorded |
| Stripped arm64 binary | 8,393,056 bytes | 9,926,704 bytes | +1.46 MiB / <30 MiB |
| Installed, signed-out Cursor process to ACP initialize | — | 291 ms | Recorded |

Normal startup constructs only the Agent titlebar toggle: it performs no sidecar lookup, ACP initialization, network request, child-process launch, or agent repaint polling. Agent events wake egui immediately and are drained 64 at a time; while a turn is active, open-file reconciliation requests a frame at 500 ms intervals. A native input-to-present trace during authenticated streaming remains part of the release smoke test.

## Language-server integration

Measured 2026-08-10 on the same Apple M4 reference machine. Run `cargo run --release --locked --example benchmark_lsp` for the bounded settings-load and 1 MiB synchronization scan.

| Metric | Baseline / result | LSP build / target | Status |
| --- | ---: | ---: | --- |
| Missing settings-file load, median / p95 | No prior settings load | 1.96 / 2.21 µs / ≤2 ms startup regression | Pass |
| 1 MiB incremental change scan, median / p95 | — | 669.79 µs / 1.04 ms / <16 ms typing budget | Pass |
| Change synchronization delay | — | 50 ms / 50–100 ms | Pass |
| Stripped arm64 binary | 11,108,768 bytes | 12,872,976 bytes (+1,764,208) / <30 MiB | Pass |
| Idle with unsupported `README.md` | Existing caret/render cadence | No LSP controller, child, or periodic LSP repaint | Pass by construction |

The pinned `lsp-types 0.97.0` addition introduced two lockfile packages (`lsp-types` and `fluent-uri`); its first incremental dependency check completed in 6.34 seconds. A supported document retains one last-sent UTF-8 snapshot. Unsupported and >5 MiB documents create no controller. Controller events wake the UI; only the 400 ms hover deadline and a full bounded command queue schedule an LSP retry frame.

The project build directory reached about 18.5 GiB during development. Only this project's `target/debug/incremental` artifacts were cleared; after the verified release build, `target` was 4.9 GiB. All five native macOS arm64 protocol smokes are recorded under `release/lsp-smoke/`; the real-server UI checks and native Linux and Windows matrices remain release-approval requirements.

The multi-provider refactor keeps that unopened path unchanged: catalog availability, persisted selection, bundle parsing, installation checks, and provisioning begin only when Agent is opened. CI verifies the exact Codex `1.1.14` adapter and private Node.js `22.22.0` runtime by deterministically packaging, extracting, and executing its version probe; networked authentication and paid prompts remain manual release checks.

The pinned package is Cursor Agent `2026.07.23-e383d2b`. A direct managed-command spike negotiated stable protocol v1 and advertised load-session, HTTP/SSE MCP, image prompt, session-list, and `cursor_login` authentication capabilities. Starting it with its supported `--disable-auto-update` option left the executable and entrypoint hashes unchanged. Deterministic tests cover streaming, split tool updates and supplied diffs, same-session follow-ups, exact allow/reject decisions, cancellation, unknown notifications, malformed stdout, bounded stderr, unexpected exit, and descendant-free shutdown without credentials or network access. Windows release tests additionally put the wrapper and a fake descendant in the same kill-on-close job object and verify that closing the job releases the descendant's marker socket.

| Cursor release target | Archive | Extracted package | Entries |
| --- | ---: | ---: | ---: |
| macOS arm64 | 66.48 MiB | 198.84 MiB | 337 |
| macOS x86_64 | 68.65 MiB | 203.91 MiB | 337 |
| Linux x86_64 | 78.70 MiB | 223.20 MiB | 341 |
| Windows x86_64 | 60.15 MiB | 160.15 MiB | 249 |

Before promoting a continuous build to stable, the release owner must repeat the authenticated paid smoke on native macOS, Linux, and Windows: streaming, follow-up context, permission allow/reject coverage, cancellation, a reported file edit, browser authentication, and child-tree teardown. The project owner confirmed on 2026-08-06 that the direct installer-mediated Cursor ACP package flow is permitted; each stable release must still recheck that Cursor's registry distribution and terms have not changed.

### Provider release smoke matrix

Run every non-package column with a disposable provider account; CI must not receive real credentials or paid tokens.

| Provider / target | Package + version probe | Auth + initialize | Session/history | Tools/permissions/cancel | Switch/teardown/repair |
| --- | --- | --- | --- | --- | --- |
| Cursor / macOS arm64 | Baseline passed 2026-08-06 | Baseline passed | Baseline passed | Baseline passed | Automated teardown and repair; repeat manually |
| Cursor / macOS x86_64, Linux x86_64, Windows x86_64 | Release CI | Required before stable | Required before stable | Required before stable | Required before stable |
| Codex / all release targets | Release CI: pinned adapter, runtime, checksum, extraction, version | Required before stable | Required before stable | Required before stable | Required before stable |
| Claude | Blocked distribution; not shipped | Not applicable | Not applicable | Not applicable | Not applicable |

For each completed run, record advertised protocol/capabilities, archive and installed sizes, time to connection, unsupported features, corrupt-package repair, and confirmation that application exit leaves no descendant process. A provider that fails a distribution, terms, auth, or teardown gate is removed from the bundle without removing its existing on-disk namespace.

## Rust highlighting

Run `cargo run --release --locked --example benchmark_highlighting`. Each edit alternately inserts and removes one character near the start of the file; the p95 is selected from 20 edits after the initial parse.

| Fixture | Initial parse | Median incremental edit | p95 incremental edit |
| --- | ---: | ---: | ---: |
| 1,220 bytes | 1.33 ms | 0.014 ms | 0.018 ms |
| 10,000 lines / 610,000 bytes | 121.76 ms | 1.63 ms | 2.36 ms |
| 1,048,590 bytes | 197.26 ms | 3.20 ms | 4.30 ms |

The 1 MiB incremental highlighting component is below the 16 ms input-to-painted target. A full OS-input-to-present p95 still needs native UI automation; this benchmark does not claim to include egui layout, event delivery, or presentation.

## Window resize

Run `cargo run --release --locked --example benchmark_resize`. The benchmark drives 60 successive widths through the retained editor, matching the CPU-heavy portion of a native maximize animation with a 1 MiB syntax-sectioned document.

Five fresh-process trials isolated the macOS titlebar toggle after the first editable frame. The old winit borderless path spent a 2.06 s median querying maximized state because it temporarily installed and removed native title styles; calling AppKit's native toggle directly avoids that cold initialization.

| First titlebar maximize | Median | Range | Improvement |
| --- | ---: | ---: | ---: |
| winit query + maximize | 2.40 s | 2.39–2.42 s | — |
| Direct AppKit toggle | 329.54 ms | 329.23–330.33 ms | 7.28x faster |

| Metric | Before | Optimized | Improvement |
| --- | ---: | ---: | ---: |
| 60 resize frames | 116.22 ms | 19.88 ms | 5.85x faster |
| Median frame | 1.64 ms | 0.256 ms | 6.41x faster |
| p95 frame | 3.05 ms | 0.287 ms | 10.63x faster |
| Cold first frame | — | 5.00 ms | Recorded |
| Warm frame | — | 0.233 ms | Recorded |

The optimized path retains per-line text and syntax allocations across width-only changes, uses cached document metrics, and skips the stale frame that initiates native maximize. The native toggle runs outside the redraw callback, and its resize burst is coalesced to one final Metal surface update after 50 ms of quiet; ordinary manual resizing remains live. A busy Metal frame slot also defers the redraw instead of synchronously waiting on the UI thread. The editor benchmark measures CPU work; the fresh-process titlebar trials cover the reported cold AppKit stall.

## Reproduction and platform status

```sh
cargo build --release --locked
EDITUR_LOG=debug target/release/editur PLAN.md
cargo run --release --locked --example benchmark_highlighting
cargo run --release --locked --example benchmark_resize
```

Metal was launched and rendered on the reference Mac with the custom borderless chrome. The DX12 and Vulkan modules pass cross-target Clippy with warnings denied; their native runtime builds remain encoded in CI. Vulkan host checking used a metadata-only `pkg-config` shim because macOS has no Linux Wayland sysroot. Native keyboard-only GUI smoke tests and complete resource baselines remain release-approval checks on Windows and Linux.

## Design token layer

Measured 2026-08-10 after landing the token module, Inter SemiBold, and JetBrains Mono.

| Metric | Result | Target | Status |
| --- | ---: | ---: | --- |
| Stripped arm64 binary growth from two subset faces | expect ≤ +0.6 MiB | well inside 30 MiB | Recorded at next release smoke |
| Idle CPU with motion idle | 0.1% | ≤ 0.1% | Pass (motion requests no frame when settled) |
| `theme::motion::animate` quantized steps | 64 | bounded retained revisions | Pass |

Appearance settings (`theme`, `density`, `editorFontSize`, `lineHeight`, `reducedMotion`) round-trip through `settings.json` and apply without a restart. Both dark and light palettes share the same contrast test suite.
