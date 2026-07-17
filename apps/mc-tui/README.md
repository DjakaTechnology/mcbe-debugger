# mc-tui

Workspace-local terminal UI for the Minecraft debugger; it is intentionally not published.

## Build and run

```text
cargo build -p mc-tui
cargo run -p mc-tui -- --mode connect --host 127.0.0.1 --port 19144
cargo run -p mc-tui -- --mode listen --port 19144
```

Flags: `--mode {connect|listen}` (default `connect`), `--host` (default
`127.0.0.1`), `--port` (default `19144`), `--target-module-uuid`, `--passcode`,
and `--config PATH`. CLI connection values override config values.

## Configuration

The default file is `%APPDATA%/minecraft-debugger/settings.json` on Windows, or
`$XDG_CONFIG_HOME/minecraft-debugger/settings.json` (falling back to
`$HOME/.config/minecraft-debugger/settings.json`) on Unix. The versioned schema is
JSON with top-level `version`, `known_plugins`, `last_target_uuid`, `passcode`, and
`filters` fields; `filters` contains `search`, `kinds`, and `levels`.
filter booleans default to true. Config can contain a passcode: treat it as a
plaintext credential, protect the file, and never put real secrets in source control,
tickets, or this documentation.

## Keymap

`q` quits when focus is outside an editable field or input popup; while editing,
printable `q` is entered into the field. `Esc` cancels or closes a popup, `Tab`/`BackTab` move focus, arrows
navigate, and `Enter`/Space activates. `h` opens Help. In the connection form,
focus the Listen or Connect mode control and press Enter/Space to choose it;
focus Advanced and press Enter/Space to toggle its fields. `x` cancels or
disconnects only when a field is not being edited; while a field is focused,
printable `x` is entered into that field. When the main pane is focused and the
connection is idle/disconnected, `l`/`c` start Listen/Connect directly. Debug
controls are `F5` continue, `F6` pause, `F10` step over, `F11` step in,
`Shift+F11` step out, `:` Minecraft command (connected), `e` evaluate (stopped),
and `1`/`2` for Log/Stats.

Log: `j`/`k` or arrows scroll, PageUp/PageDown page, `g`/`G` jump ends, `/` search,
`f` filters, `r` resets filters. Filter popup: arrows/`j`/`k` select and Space/Enter
toggle; `r` resets. Stats: arrows/`j`/`k`, PageUp/PageDown, `g`/`G`, Left/Right
categories, `[`/`]` clients, and `r` clears data. Narrow terminals automatically
use compact layout/labels without changing behavior or filtering.

Listen mode auto-relistens after disconnect. Starting Connect, cancelling, or quitting
cancels the pending retry. Normal quit and panic cleanup restore terminal raw mode and
screen state; use `q` rather than killing the process.

## Protocol v7 live smoke

The fixture proves the accepted v7 handshake and nested request shape only; it does
not claim live Minecraft parity. For live smoke use a plugin explicitly reporting v7,
a disposable test world and backup. Checklist: start plugin; verify reported v7;
connect with host/port and target UUID if needed; issue harmless pause/continue or
Minecraft command; observe event/log roundtrip; disconnect and verify terminal restore.
Do not use production credentials.

## Troubleshooting

- Connection refused: start the plugin and check host, port, firewall, mode, and UUID.
- Handshake rejected: confirm supported v7–v9 and provide a required passcode.
- No rows: press `r`, inspect `f`, and clear `/` search; stats still accumulate when
  the Stat log filter is disabled.
- Garbled terminal: quit normally, resize, and rerun; panic cleanup restores raw mode.
