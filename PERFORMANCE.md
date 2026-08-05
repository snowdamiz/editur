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
