# Performance baseline

Measured 2026-08-04 from the stripped `--release` build on a MacBook Air with an Apple M4, 16 GB RAM, and macOS 26.5.2. Times are emitted by `EDITUR_LOG=debug`; resource usage was sampled after the window settled.

## Native application

| Metric | Result | Target | Status |
| --- | ---: | ---: | --- |
| Warm command-to-editable-window, 5-run median | 2.19 s | 150 ms | Miss |
| Five-run p95 | 2.26 s | 300 ms cold p95 | Miss; this run did not isolate cold cache state |
| Path resolution | 0.15 ms | — | Recorded |
| Native window creation, median | 2.08 s | — | Dominant startup cost in this desktop-hosted session |
| Metal initialization, median | 3.47 ms | — | Recorded |
| Idle CPU | 0.0% | <1% | Pass |
| Resident memory with `PLAN.md` | 67.9 MiB | 60 MiB | Miss |
| Stripped arm64 binary | 8,029,776 bytes (7.66 MiB) | 30 MiB | Pass |

The startup and memory misses are release exceptions, not hidden passes. Startup profiling points to macOS native window creation rather than path loading or Metal initialization. They should be remeasured from a packaged app outside the Codex desktop host before a v1 release is approved. Windows/D3D12 and Linux/Vulkan runtime baselines likewise require their native release runners.

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

Metal was launched and rendered on the reference Mac. The DX12 module passes a Windows-target Clippy build locally; its native build/test is encoded in CI. Vulkan source type-checking was exercised on the host, while the normal macOS-to-Linux check is blocked by the absence of a Linux Wayland `pkg-config` sysroot; the Ubuntu CI job is the authoritative native build. Native keyboard-only GUI smoke tests and complete resource baselines remain release-approval checks on Windows and Linux.
