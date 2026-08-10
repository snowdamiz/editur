# Language-server native smoke records

Run the protocol portion on the native release target with:

```sh
cargo run --release --locked --example lsp_native_smoke -- <preset-id>
```

Preset IDs are `rust-analyzer`, `typescript-language-server`, `pyright`, `gopls`, and `clangd`. The runner uses the user-installed executable, verifies diagnostics, completion, hover, definition, custom restart, disable, re-enable, graceful shutdown, and prints a JSON record. Complete each record in the Editur UI by accepting and undoing the completion, exercising the Settings controls, and confirming that no descendant remains after exit.

Release approval requires all five presets on native macOS, Linux, and Windows. Records must contain the real server version; unavailable servers and cross-compiled binaries do not count as a pass.

| Target | Rust | TypeScript / JavaScript | Python | Go | C / C++ |
| --- | --- | --- | --- | --- | --- |
| macOS arm64 | Protocol, Settings/diagnostics UI, and Editur exit passed with rust-analyzer 0.3.2989 on 2026-08-10; completion/hover/definition UI checks required | Protocol passed with TypeScript Language Server 5.3.0 on 2026-08-10; UI checks required | Protocol passed with Pyright 1.1.411 on 2026-08-10; UI checks required | Protocol passed with gopls 0.23.0 on 2026-08-10; UI checks required | Protocol passed with Apple clangd 21.0.0 on 2026-08-10; UI checks required |
| Linux x86_64 | Required | Required | Required | Required | Required |
| Windows x86_64 | Required | Required | Required | Required | Required |
