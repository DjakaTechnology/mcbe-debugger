# Minecraft Debugger

Minecraft Debugger is a Rust workspace for inspecting and controlling Minecraft Bedrock debugger connections, with a Tauri desktop application and a Ratatui terminal UI (TUI) sharing the same session and protocol layers.

> **Current status:** The desktop application, TUI, and shared session layer are implemented and heavily tested. DAP, source maps, and Zed integration are incomplete/experimental, and live protocol-v7 parity remains environment-dependent.

## Features

Both frontends provide:

- Listen for or connect to a debugger, including passcode handling and target selection when multiple plugins are advertised.
- Continue, pause, step over, step in, and step out controls.
- Minecraft commands and expression evaluation.
- Event/log filtering and resettable views.
- Statistics with client and addon filters.
- Persistent connection and filter settings.
- Automatic re-listening after an unexpected disconnect in listen mode.

The desktop UI adds graphical event, evaluation, control, and statistics panels. The TUI provides keyboard-driven views and compact terminal layouts; see the [TUI guide](apps/mc-tui/README.md) for the complete keymap and configuration schema.

## Architecture

```text
mc-desktop ──▶ mc-tauri ──▶ mc-session ──▶ mc-protocol
mc-tui     ────────────────▶ mc-session ──▶ mc-protocol
```

`mc-session` owns the framework-neutral connection lifecycle and command/event flow. The protocol crate handles framing, handshakes, events, and wire encoding; frontend adapters translate that model for Tauri or the terminal UI.

## Workspace

| Member | Path | Status |
| --- | --- | --- |
| `mc-protocol` | `crates/mc-protocol` | Core protocol implementation; v8+ flat/Cereal encoding is active, with legacy v5–v7 nested encoding/decoding still TODO. |
| `mc-session` | `crates/mc-session` | Implemented shared connection/session controller with controls, evaluation, plugin selection, cancellation, and lifecycle events. |
| `mc-source-maps` | `crates/mc-source-maps` | Stub foundation; source-map resolution is not implemented. |
| `mc-tauri` | `adapters/mc-tauri` | Implemented Tauri-facing adapter over the shared session. |
| `mc-dap-server` | `adapters/mc-dap-server` | Experimental/incomplete DAP sidecar stub. |
| `mc-desktop` | `apps/mc-desktop` and `apps/mc-desktop/src-tauri` | Implemented Tauri + Svelte desktop debugger UI. |
| `mc-tui` | `apps/mc-tui` | Implemented Ratatui terminal debugger UI; workspace-local and unpublished. |
| `zed-extension` | `apps/zed-extension` | Experimental Zed extension; DAP attach/integration remains incomplete. |

## Protocol status

- Declared protocol range: **v7–v9**; current default: **v9**.
- Default debugger port: **19144**.
- A mock v7 fixture exists and exercises the accepted handshake and nested request shape, but live v7 parity with Minecraft/debugger plugins is not established.
- Legacy nested v5–v7 encode/decode functions remain TODO; do not treat v7 as production-ready.

## Prerequisites

- Rust **1.85+**.
- [Bun](https://bun.sh/) for the desktop frontend.
- Tauri's [OS prerequisites](https://v2.tauri.app/start/prerequisites/) for the target platform.
- A compatible Minecraft Bedrock debugger plugin, with a reachable host/port and any required passcode.

## Build and run

### Desktop

```text
cd apps/mc-desktop
bun install --frozen-lockfile
bun run tauri dev
bun run tauri build
```

### TUI

From the repository root:

```text
cargo build -p mc-tui
cargo run -p mc-tui -- --mode connect --host 127.0.0.1 --port 19144
cargo run -p mc-tui -- --mode listen --port 19144
```

Useful options are `--mode {connect|listen}` (default `connect`), `--host` (default `127.0.0.1`), `--port` (default `19144`), `--target-module-uuid`, `--passcode`, and `--config PATH`. CLI connection values override configuration values. See the [full TUI README](apps/mc-tui/README.md) for key bindings and configuration details.

## Configuration and security

The TUI uses `%APPDATA%/minecraft-debugger/settings.json` on Windows and `$XDG_CONFIG_HOME/minecraft-debugger/settings.json`, falling back to `$HOME/.config/minecraft-debugger/settings.json`, on Unix. The desktop stores settings through the Tauri Store plugin in `settings.json`. Settings can include a passcode and are plaintext credentials: protect the file, avoid source control, and never use real secrets in examples or issue reports.

## Verification

Known working checks from the repository root:

```text
cargo check --workspace
cargo test --workspace
cargo clippy -p mc-protocol -p mc-session -p mc-tauri -p mc-tui -p mc-desktop --all-targets -- -D warnings
cargo fmt -p mc-session -p mc-tauri -p mc-tui -p mc-desktop -- --check
```

Desktop frontend checks:

```text
cd apps/mc-desktop
bun run check
bun run build
```

## Known limitations and direction

The protocol's legacy nested v5–v7 codecs are explicit stubs, and live v7 compatibility depends on the plugin and environment. Source-map lookup is a stub. The DAP server and Zed extension are experimental scaffolding rather than a complete editor integration; attach behavior and source-level debugging are not finished. Near-term work is to complete protocol parity, then build out source maps, DAP translation, and reliable Zed workflows while preserving the tested desktop/TUI session behavior.

## Repository tree

```text
crates/
  mc-protocol/       Wire protocol, framing, events, and codecs
  mc-session/        Shared connection/session controller
  mc-source-maps/    Source-map foundation (stub)
adapters/
  mc-tauri/          Tauri session adapter
  mc-dap-server/     DAP sidecar (experimental)
apps/
  mc-desktop/        Tauri + Svelte desktop application
  mc-tui/            Ratatui terminal application
  zed-extension/    Zed extension (experimental)
```

## License

MIT. See the workspace manifests for package licensing metadata.
