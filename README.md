# wisp

Tailscale-native, agentless infrastructure control plane.

`wisp` defaults to Tailscale SSH (recommended), polls Docker + optional Azure DB telemetry, and renders:
- a local TUI dashboard
- a local web dashboard (`http://127.0.0.1:8080` by default)

## What it shows

### Docker
- Container name, state, status
- CPU %, memory usage
- Network I/O (`docker stats` `NetIO`)
- On-demand actions:
  - Restart container
  - View logs (`docker logs -n 50 <name>`)
  - Disk usage (`docker system df`)

These actions are available in both TUI and web dashboard.

### Azure DB (optional)
For Azure Database for PostgreSQL Flexible Server or MySQL Flexible Server:
- DB server name
- DB type (PostgreSQL Flexible / MySQL)
- CPU %
- Memory %
- Storage %
- `ACT CONN` (active connections)

`ACT CONN` comes from Azure Monitor metric `active_connections`.

## Requirements

- Rust toolchain (edition 2024 project)
- Tailscale with SSH enabled between your machine and target host
- Docker installed on target host
- Optional standard SSH client (`ssh`) only if using `--ssh` mode
- Optional Azure monitoring:
  - Azure CLI (`az`) installed and logged in, or `AZURE_ACCESS_TOKEN`

## Build

```bash
cargo build
```

## Run

```bash
cargo run -- -H <tailscale-ip>
```

Optional flags:
- `-p, --port <port>`: SSH port (default 22)
- `-u, --user <user>`: SSH user (default from config)
- `-i, --interval <seconds>`: Docker poll interval
- `--web-port <port>`: local web dashboard port (default 8080)
- `--ssh`: use standard SSH transport instead of Tailscale SSH

### Security note on `--ssh`

`--ssh` is less safe by default because it often implies opening SSH ports.

Recommendation:
- keep SSH ports closed to the public internet
- prefer Tailscale mode whenever possible
- if SSH must be used, restrict ingress aggressively (CIDRs, firewall rules, bastion)

## Setup wizard (Azure)

Interactive auto-discovery via Azure CLI:

```bash
cargo run -- --setup
```

Writes config to:
- `~/.config/wisp/config.toml`

Project-local `wisp.toml` is also supported and takes priority when present.

Host transport can also be set in config:

```toml
[host]
transport = "tailscale" # or "ssh"
```

## TUI controls

Global:
- `q` / `Ctrl+C`: quit
- `j/k` or `↑/↓`: move selection

Container actions:
- `r`: restart selected container
- `l`: open log viewer for selected container (last 50 lines)
- `d`: open Docker disk usage (`docker system df`)

Popup controls:
- `Esc` / `Enter` / `q`: close popup
- `j/k` or `↑/↓`: scroll 1 line
- `PgUp` / `PgDn`: scroll by page chunk
- `Home` / `End`: jump to top/bottom
- Mouse wheel: scroll

## Web controls

- `disk usage` button: runs `docker system df`
- Per-container `logs` button: runs `docker logs -n 50 <name>`
- Per-container `restart` button: runs `docker restart <name>`
- Action output opens in a modal pane with scroll

## Logging behavior

Runtime logs are disabled by default to keep the TUI clean.

To enable debug logs explicitly:

```bash
RUST_LOG=wisp=debug cargo run -- -H <tailscale-ip>
```

## Architecture (high level)

- `src/main.rs`: runtime wiring, channels, poll loop, TUI/web launch
- `src/ssh.rs`: Tailscale SSH session and remote command execution
- `src/telemetry/docker.rs`: Docker JSON parsing models
- `src/telemetry/azure.rs`: Azure Monitor metric fetch + token flow
- `src/tui/`: terminal UI rendering + input handling
- `src/web/`: Axum router + websocket snapshot stream

## Notes

- Commands execute on the remote host over SSH.
- Log output is normalized for TUI display (`\r` converted to newlines).
- If Azure is not configured, Docker telemetry still works fully.
