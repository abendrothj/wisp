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

- Docker on the target host
- One transport mode:
  - **Default (recommended):** Tailscale SSH
  - **Fallback:** standard OpenSSH via `--ssh`
- `tailscale` CLI when using Tailscale mode
- Optional Azure monitoring:
  - Azure CLI (`az`) logged in, or
  - `AZURE_ACCESS_TOKEN` environment variable

Rust is only required when building from source.

## Quickstart

### 1) Download from GitHub Releases

Release page:
- https://github.com/abendrothj/wisp/releases

Pick the artifact for your platform:
- macOS Intel: `wisp-vX.Y.Z-x86_64-apple-darwin.tar.gz`
- macOS Apple Silicon: `wisp-vX.Y.Z-aarch64-apple-darwin.tar.gz`
- Linux x86_64: `wisp-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- Windows x86_64: `wisp-vX.Y.Z-x86_64-pc-windows-msvc.zip`

### 2) Verify checksum (recommended)

Download `SHA256SUMS` from the same release and verify your archive:

```bash
shasum -a 256 <artifact-file>
```

Compare output with the matching line in `SHA256SUMS`.

### 3) Install binary

macOS (Apple Silicon):

```bash
VERSION=v1.0.0
curl -LO "https://github.com/abendrothj/wisp/releases/download/${VERSION}/wisp-${VERSION}-aarch64-apple-darwin.tar.gz"
tar -xzf "wisp-${VERSION}-aarch64-apple-darwin.tar.gz"
chmod +x wisp
sudo mv wisp /usr/local/bin/wisp
```

macOS (Intel):

```bash
VERSION=v1.0.0
curl -LO "https://github.com/abendrothj/wisp/releases/download/${VERSION}/wisp-${VERSION}-x86_64-apple-darwin.tar.gz"
tar -xzf "wisp-${VERSION}-x86_64-apple-darwin.tar.gz"
chmod +x wisp
sudo mv wisp /usr/local/bin/wisp
```

Linux (x86_64):

```bash
VERSION=v1.0.0
curl -LO "https://github.com/abendrothj/wisp/releases/download/${VERSION}/wisp-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
tar -xzf "wisp-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
chmod +x wisp
sudo mv wisp /usr/local/bin/wisp
```

Windows (PowerShell):

```powershell
$Version = "v1.0.0"
Invoke-WebRequest -Uri "https://github.com/abendrothj/wisp/releases/download/$Version/wisp-$Version-x86_64-pc-windows-msvc.zip" -OutFile "wisp.zip"
Expand-Archive -Path "wisp.zip" -DestinationPath "." -Force
New-Item -ItemType Directory -Force -Path "$env:USERPROFILE\bin" | Out-Null
Move-Item -Force ".\wisp.exe" "$env:USERPROFILE\bin\wisp.exe"
```

### 4) Run with Tailscale (default)

```bash
wisp -H <tailscale-ip> -u <user>
```

### 5) Open web dashboard (optional)

By default: `http://127.0.0.1:8080`

## Build from source (optional)

```bash
git clone <your-repo-url>
cd wisp
cargo build
cargo run -- -H <tailscale-ip> -u <user>
```

## Transport modes

### Tailscale mode (default)

- Preferred security posture
- Uses `tailscale ssh`

### SSH mode (`--ssh`)

```bash
wisp --ssh -H <host> -u <user> [-p <port>]
```

Use only when Tailscale is not available.

- Keep SSH closed to the public internet
- Restrict ingress with firewall/CIDR/bastion controls

## Setup wizard (Azure)

Use the guided setup to discover subscription + DB server and save config:

```bash
wisp --setup
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
RUST_LOG=wisp=debug wisp -H <tailscale-ip>
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

### More services (multi-cloud)

Goal: make `wisp` a practical cross-cloud control plane while staying lightweight.

Planned:
- AWS support (first target):
  - ECS service/task health + rollout status
  - RDS metrics (CPU, memory, storage, connections)
  - CloudWatch-backed service telemetry summaries
- Expanded Azure coverage beyond DB metrics:
  - App Service / Container Apps health and restart actions
  - AKS node + workload high-level telemetry
- Additional providers:
  - GCP Cloud SQL + Cloud Run telemetry/actions
  - Optional DigitalOcean/Linode basic compute monitoring

### Feature expansion

Goal: increase operator leverage without turning `wisp` into dashboard bloat.

Planned:
- Alerts + thresholds in config (CPU/memory/storage/connection guardrails)
- Event timeline (container restarts, deploys, failures, prune operations)
- Safer action controls (dry-run previews, optional approval prompts per action class)
- Better web parity and workflows:
  - live log streaming mode
  - richer inspect rendering (structured sections)
  - keyboard shortcuts matching TUI behavior
- Extensible service adapters so new providers can be added with minimal core changes

## Release process

Tag-based release publishing is automated via GitHub Actions.

When you push a tag like `v1.0.0`, CI builds and attaches:
- `wisp-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- `wisp-vX.Y.Z-x86_64-apple-darwin.tar.gz`
- `wisp-vX.Y.Z-aarch64-apple-darwin.tar.gz`
- `wisp-vX.Y.Z-x86_64-pc-windows-msvc.zip`
- `SHA256SUMS`

Typical release commands:

```bash
git checkout main
git pull --ff-only
git tag -a v1.0.0 -m "wisp v1.0.0"
git push origin main
git push origin v1.0.0
```

## License

MIT
