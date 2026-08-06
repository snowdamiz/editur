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
| Stripped arm64 binary | 8,393,056 bytes | 9,675,504 bytes | +1.22 MiB / <30 MiB |
| Installed, signed-out Cursor process to ACP initialize | — | 291 ms | Recorded |

Normal startup constructs only the Agent titlebar toggle: it performs no sidecar lookup, ACP initialization, network request, child-process launch, or agent repaint polling. Agent events wake egui immediately and are drained 64 at a time; while a turn is active, open-file reconciliation requests a frame at 500 ms intervals. A native input-to-present trace during authenticated streaming remains part of the release smoke test.

The pinned package is Cursor Agent `2026.07.23-e383d2b`. A direct managed-command spike negotiated stable protocol v1 and advertised load-session, HTTP/SSE MCP, image prompt, session-list, and `cursor_login` authentication capabilities. Starting it with its supported `--disable-auto-update` option left the executable and entrypoint hashes unchanged. Deterministic tests cover streaming, split tool updates and supplied diffs, same-session follow-ups, exact allow/reject decisions, cancellation, unknown notifications, malformed stdout, bounded stderr, unexpected exit, and descendant-free shutdown without credentials or network access. Windows release tests additionally put the wrapper and a fake descendant in the same kill-on-close job object and verify that closing the job releases the descendant's marker socket.

| Cursor release target | Archive | Extracted package | Entries |
| --- | ---: | ---: | ---: |
| macOS arm64 | 66.48 MiB | 198.84 MiB | 337 |
| macOS x86_64 | 68.65 MiB | 203.91 MiB | 337 |
| Linux x86_64 | 78.70 MiB | 223.20 MiB | 341 |
| Windows x86_64 | 60.15 MiB | 160.15 MiB | 249 |

Before promoting a continuous build to stable, the release owner must repeat the authenticated paid smoke on native macOS, Linux, and Windows: streaming, follow-up context, permission allow/reject coverage, cancellation, a reported file edit, browser authentication, and child-tree teardown. The project owner confirmed on 2026-08-06 that the direct installer-mediated Cursor ACP package flow is permitted; each stable release must still recheck that Cursor's registry distribution and terms have not changed.

## Rust highlighting

Run `cargo run --release --locked --example benchmark_highlighting`. Each edit alternately inserts and removes one character near the start of the file; the p95 is selected from 20 edits after the initial parse.

| Fixture | Initial parse | Median incremental edit | p95 incremental edit |
| --- | ---: | ---: | ---: |
| 1,220 bytes | 1.33 ms | 0.014 ms | 0.018 ms |
| 10,000 lines / 610,000 bytes | 121.76 ms | 1.63 ms | 2.36 ms |
| 1,048,590 bytes | 197.26 ms | 3.20 ms | 4.30 ms |

The 1 MiB incremental highlighting component is below the 16 ms input-to-painted target. A full OS-input-to-present p95 still needs native UI automation; this benchmark does not claim to include egui layout, event delivery, or presentation.

## Reproduction and platform status

```sh
cargo build --release --locked
EDITUR_LOG=debug target/release/editur PLAN.md
cargo run --release --locked --example benchmark_highlighting
```

Metal was launched and rendered on the reference Mac with the custom borderless chrome. The DX12 and Vulkan modules pass cross-target Clippy with warnings denied; their native runtime builds remain encoded in CI. Vulkan host checking used a metadata-only `pkg-config` shim because macOS has no Linux Wayland sysroot. Native keyboard-only GUI smoke tests and complete resource baselines remain release-approval checks on Windows and Linux.
