# wisp

Author: Jake Abendroth

Tailscale-native, agentless infrastructure control plane.

`wisp` defaults to Tailscale SSH (recommended), polls Docker + optional Azure DB telemetry, and renders:
- a local TUI dashboard
- a local web dashboard (`http://127.0.0.1:8080` by default)

## Inspiration

This project started with a very specific, very real problem:

"I want container management and telemetry, but I do **not** want to babysit another heavyweight control-plane app just to click Restart."

Portainer is powerful, but for this environment it felt like bringing a cruise ship to cross a puddle.

So `wisp` is the opposite philosophy:
- no agent to install
- no always-on management stack to maintain
- no giant UI framework tax
- just direct, secure control over the boxes you already run

In short: less dashboard theater, more useful buttons.

## What it shows

### Docker
- Container name, state, status
- CPU %, memory usage
- Network I/O (`docker stats` `NetIO`)
- On-demand actions:
  - Start container
  - Stop container
  - Restart container
  - Inspect container (`docker inspect <name>`)
  - View logs (`docker logs -n 50 <name>`)
  - Disk usage (`docker system df`)
  - Guarded prune of stopped containers (`docker container prune -f`)

Interactive shells are intentionally out of scope for `wisp` reliability; use direct SSH when you need a shell session.

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
- Docker installed on target host
- Transport requirements (choose one):
  - Tailscale mode (default): Tailscale with SSH enabled between your machine and target host
  - SSH mode (`--ssh`): OpenSSH client (`ssh`) and reachable SSH service on target host
- `tailscale` CLI if using Tailscale mode
- Optional Azure monitoring:
  - Azure CLI (`az`) installed and logged in, or `AZURE_ACCESS_TOKEN`

## Install from zero

### 0) Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

### 1) Clone

```bash
git clone <your-repo-url>
cd wisp
```

### 2) Set up Tailscale

Follow official docs:
- https://tailscale.com/download
- https://tailscale.com/kb/1193/tailscale-ssh

### 3) Build + run

```bash
cargo build
cargo run -- -H <tailscale-ip> -u <user>
```

### 4) Install to PATH (optional)

```bash
cargo install --path .
wisp --version
```

If `wisp` is not found, ensure Cargo bin dir is on your PATH:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

## Alternative: SSH mode

Use when you explicitly do not want Tailscale transport:

```bash
cargo run -- --ssh -H <host> -u <user> [-p <port>]
```

⚠ `--ssh` is less safe by default because it often implies opening SSH ports.
- Keep SSH ports closed to public internet.
- Prefer Tailscale mode when possible.
- If SSH is required, restrict ingress aggressively (CIDRs/firewall/bastion).

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
- `a`: start selected container
- `x`: stop selected container
- `r`: restart selected container
- `Enter`: inspect selected container (`docker inspect <name>`)
- `l`: open log viewer for selected container (last 50 lines)
- `d`: open Docker disk usage (`docker system df`)
- `p` then `p` (within 5s): guarded prune of stopped containers

Popup controls:
- `Esc` / `Enter` / `q`: close popup
- `j/k` or `↑/↓`: scroll 1 line
- `PgUp` / `PgDn`: scroll by page chunk
- `Home` / `End`: jump to top/bottom
- Mouse wheel: scroll

## Web controls

- `disk usage` button: runs `docker system df`
- `prune stopped` button (with confirm): runs `docker container prune -f`
- Per-container `start` button: runs `docker start <name>`
- Per-container `stop` button: runs `docker stop <name>`
- Per-container `inspect` button: runs `docker inspect <name>`
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
