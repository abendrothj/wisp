# wisp v1.0

Author: Jake Abendroth

Tailscale-native, agentless infrastructure control plane for Docker operations + optional Azure DB telemetry.

`wisp` gives you a fast local control surface with no remote agents and no management stack to babysit:
- Terminal UI (primary operator workflow)
- Local web dashboard at `http://127.0.0.1:8080` (default)

## Why wisp

`wisp` is built for one job: make common infra actions instant, secure, and low-friction.

- No daemon on target hosts
- No always-on control plane
- Tailscale-first transport model
- Practical fallback to native `ssh` when needed

In short: less dashboard theater, more useful buttons.

## v1.0 highlights

- Concurrent architecture using snapshot broadcasting + action channels for responsive UI under remote latency
- Full container action path in both TUI and web: start, stop, restart, inspect, logs, disk usage, guarded prune
- Optional Azure DB metrics panel (PostgreSQL Flexible + MySQL Flexible)
- Embedded static web dashboard and websocket live updates
- Configurable TUI theming via `wisp.toml` / global config

## What wisp shows

### Docker telemetry

- Container name, state, health, and status
- CPU %, memory usage, network I/O
- Real-time table updates

### Azure telemetry (optional)

- DB name and type
- CPU %, memory %, storage %
- `ACT CONN` from Azure Monitor `active_connections`

## Supported actions

Actions execute on the remote host through the selected transport.

- Start: `docker start <name>`
- Stop: `docker stop <name>`
- Restart: `docker restart <name>`
- Inspect: `docker inspect <name>`
- Logs: `docker logs -n 50 <name>`
- Disk usage: `docker system df`
- Guarded prune (stopped containers): `docker container prune -f`

Interactive shell sessions are intentionally out of scope for reliability. Use direct SSH when shell access is needed.

## Requirements

- Rust toolchain (edition 2024)
- Docker on the target host
- One transport mode:
  - **Default (recommended):** Tailscale SSH
  - **Fallback:** standard OpenSSH via `--ssh`
- `tailscale` CLI when using Tailscale mode
- Optional Azure monitoring:
  - Azure CLI (`az`) logged in, or
  - `AZURE_ACCESS_TOKEN` environment variable

## Quickstart

### 1) Clone and build

```bash
git clone <your-repo-url>
cd wisp
cargo build
```

### 2) Run with Tailscale (default)

```bash
cargo run -- -H <tailscale-ip> -u <user>
```

### 3) Open web dashboard (optional)

By default: `http://127.0.0.1:8080`

## Transport modes

### Tailscale mode (default)

- Preferred security posture
- Uses `tailscale ssh`

### SSH mode (`--ssh`)

```bash
cargo run -- --ssh -H <host> -u <user> [-p <port>]
```

Use only when Tailscale is not available.

- Keep SSH closed to the public internet
- Restrict ingress with firewall/CIDR/bastion controls

## Setup wizard (Azure)

Use the guided setup to discover subscription + DB server and save config:

```bash
cargo run -- --setup
```

Global config path:
- `~/.config/wisp/config.toml`

Project-local config path (takes priority when present):
- `./wisp.toml`

## Configuration

### Minimal example

```toml
[host]
address = "100.64.0.10"
port = 22
user = "deploy"
interval = 5
transport = "tailscale" # or "ssh"

[web]
port = 8080
```

### Optional Azure section

```toml
[azure]
subscription_id = "<sub-id>"
resource_group = "<rg>"
db_server = "<server-name>"
db_type = "postgresql-flexible" # or "mysql"
```

### Optional TUI theme section

Theme values support named colors (`red`, `cyan`, `darkgray`, etc.) or hex (`#RRGGBB`).

```toml
[theme]
accent = "cyan"
border = "blue"
muted = "darkgray"
text = "white"
success = "green"
warning = "yellow"
danger = "red"
panel = "black"
selection_fg = "black"
selection_bg = "cyan"
```

## CLI flags

- `-H, --host <host>`: target host (required unless config provides one)
- `-p, --port <port>`: SSH port (default `22`)
- `-u, --user <user>`: remote user (default from config)
- `-i, --interval <seconds>`: Docker poll interval
- `--web-port <port>`: local web dashboard port (default `8080`)
- `--ssh`: force standard SSH transport
- `--setup`: launch Azure setup wizard

## TUI controls

### Global

- `q` / `Ctrl+C`: quit
- `j/k` or `↑/↓`: move selection

### Container actions

- `a`: start
- `x`: stop
- `r`: restart
- `Enter`: inspect
- `l`: logs (last 50 lines)
- `d`: docker disk usage
- `p` then `p` within 5s: guarded prune

### Popup controls

- `Esc` / `Enter` / `q`: close popup
- `j/k` or `↑/↓`: scroll line
- `PgUp` / `PgDn`: scroll chunk
- `Home` / `End`: jump to top/bottom
- Mouse wheel: scroll

## Web controls

Web action parity with TUI includes:

- `disk usage`
- `prune stopped` (with confirmation)
- per-container `start`, `stop`, `restart`, `inspect`, `logs`
- modal action output view

## Logging

Runtime logging is off by default to keep the UI clean.

Enable debug logs:

```bash
RUST_LOG=wisp=debug cargo run -- -H <tailscale-ip>
```

## Architecture

- `src/main.rs`: runtime orchestration, polling loop, action worker, TUI/web startup
- `src/ssh.rs`: remote command execution over Tailscale SSH or native SSH
- `src/telemetry/docker.rs`: Docker parsing and models
- `src/telemetry/azure.rs`: Azure metric collection + token handling
- `src/tui/`: ratatui rendering and input handling
- `src/web/`: axum routes, action handlers, websocket snapshot stream

## Security notes

- Tailscale-first is the default and recommended deployment model
- Web UI binds to localhost by default (`127.0.0.1`)
- Action endpoints require an internal action header for browser-side requests
- Remote commands run only through explicit operator actions

## Roadmap

### Saved + configurable servers

Goal: manage multiple environments without rewriting flags each run.

Planned:
- Named server profiles in config (for example: `prod`, `staging`, `dev`)
- Per-profile host/user/port/transport/interval/web-port/theme overrides
- Simple profile selection at launch
- Safe defaults with explicit profile activation

### One-command TUI entry

Goal: jump straight into operations with one command.

Planned:
- Launch TUI with a single profile-aware command (for example: `wisp up prod`)
- Automatic profile resolution from config when no explicit flags are provided
- Optional first-run bootstrap flow to create initial profile
- Keep advanced flags available, but make them optional for daily use

## License

Add your project license here (for example: MIT).
