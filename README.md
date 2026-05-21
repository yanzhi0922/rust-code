# rust-code

Rust implementation of a Claude Code-style coding agent, extracted from the completed `remote-code-rust` workspace.

This repository intentionally contains the claudecode rewrite slice only:

- `agents/claudecode`: the `remote-code` CLI/TUI/headless entry point
- `crates/claude/*`: the local Claude runtime crates required by that entry point
- `crates/shared/rc-agent-protocol`, `rc-engine-events`, `rc-remote-transport`: shared protocol/event/transport crates required by the runtime

It excludes the desktop GUI, Codex integration, Roo integration, deployment assets, and other Remote Code platform components.

## Build

```powershell
cargo check -p remote-code
cargo run -p remote-code -- --help
```

For broader validation:

```powershell
cargo fmt --all -- --check
cargo test --workspace
```

The directory layout matches the source workspace so internal path dependencies remain stable.
