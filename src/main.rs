mod config;
mod setup;
mod ssh;
mod telemetry;
mod tui;
mod web;

use anyhow::{Context, Result};
use clap::Parser;
use std::time::Duration;
use telemetry::azure;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{error, warn};

use config::Config;

#[derive(Debug, Clone)]
pub enum RemoteAction {
    Start { name: String },
    Stop { name: String },
    Restart { name: String },
    Logs { name: String },
    Inspect { name: String },
    Prune,
    SystemDf,
}

#[derive(Debug)]
pub struct RemoteActionRequest {
    pub action: RemoteAction,
    pub respond_to: oneshot::Sender<RemoteActionResult>,
}

#[derive(Debug, Clone)]
pub struct RemoteActionResult {
    pub title: String,
    pub output: String,
    pub is_error: bool,
}

/// wisp — Tailscale-native, agentless infrastructure control plane
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Interactive Azure setup wizard — auto-detects resources via `az` CLI
    #[arg(long)]
    setup: bool,

    /// Tailscale IP of the target host (overrides wisp.toml)
    #[arg(short = 'H', long)]
    host: Option<String>,

    /// SSH port
    #[arg(short, long)]
    port: Option<u16>,

    /// Remote user
    #[arg(short, long)]
    user: Option<String>,

    /// Docker poll interval in seconds
    #[arg(short = 'i', long)]
    interval: Option<u64>,

    /// Web dashboard port (localhost only)
    #[arg(long)]
    web_port: Option<u16>,

    /// Use standard SSH instead of Tailscale SSH.
    ///
    /// Unsafe default posture for internet-facing hosts: requires open SSH ports.
    /// Prefer Tailscale mode unless absolutely necessary.
    #[arg(long)]
    ssh: bool,

    #[arg(long, env = "AZURE_SUBSCRIPTION_ID")]
    azure_subscription_id: Option<String>,

    #[arg(long, env = "AZURE_RESOURCE_GROUP")]
    azure_resource_group: Option<String>,

    #[arg(long, env = "AZURE_DB_SERVER")]
    azure_db_server: Option<String>,

    #[arg(long, env = "AZURE_DB_TYPE")]
    azure_db_type: Option<String>,
}

// ── resolved runtime config ───────────────────────────────────────────────────

struct Resolved {
    host:     String,
    port:     u16,
    user:     String,
    interval: Duration,
    web_port: u16,
    azure:    Option<azure::AzureConfig>,
    transport: ssh::Transport,
}

struct PollLoopConfig {
    host: String,
    port: u16,
    user: String,
    transport: ssh::Transport,
    docker_interval: Duration,
    azure_cfg: Option<azure::AzureConfig>,
}

impl Resolved {
    fn build(args: &Args, file: Option<Config>) -> Result<Self> {
        let file = file.unwrap_or_default();

        let host = args.host.clone()
            .or_else(|| Some(file.host.address.clone()).filter(|s| !s.is_empty()))
            .context(
                "no host specified — pass -H <tailscale-ip> or run `wisp --setup` to write a config file",
            )?;

        let port     = args.port.unwrap_or(file.host.port);
        let user     = args.user.clone().unwrap_or_else(|| file.host.user.clone());
        let interval = Duration::from_secs(args.interval.unwrap_or(file.host.interval));
        let web_port = args.web_port.unwrap_or(file.web.port);
        let transport = if args.ssh {
            ssh::Transport::Ssh
        } else {
            file.host.transport()
        };

        // CLI Azure args take priority over config file
        let azure = if args.azure_subscription_id.is_some() {
            args_azure_config(args)
        } else {
            file.azure_config().or_else(azure::AzureConfig::from_env)
        };

        Ok(Self { host, port, user, interval, web_port, azure, transport })
    }
}

fn args_azure_config(args: &Args) -> Option<azure::AzureConfig> {
    let sub  = args.azure_subscription_id.clone()?;
    let rg   = args.azure_resource_group.clone()?;
    let name = args.azure_db_server.clone()?;
    let kind = if args.azure_db_type.as_deref() == Some("mysql") {
        azure::ServerType::MySQL
    } else {
        azure::ServerType::PostgreSQLFlexible
    };
    Some(azure::AzureConfig {
        subscription_id: sub,
        resource_group: rg,
        server_name: name,
        server_type: kind,
    })
}

// ── entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("off"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    if args.setup {
        return setup::run(args.host.as_deref()).await;
    }

    let file_config = Config::load();
    let cfg = Resolved::build(&args, file_config)?;

    if cfg.transport == ssh::Transport::Ssh {
        eprintln!(
            "⚠ standard SSH mode enabled (--ssh): this usually requires open SSH ports and is less safe than Tailscale SSH."
        );
        eprintln!("⚠ recommendation: keep SSH ports closed to public internet and prefer Tailscale mode.");
    }

    if cfg.azure.is_some() {
        tracing::info!("azure db monitoring enabled");
    } else {
        tracing::info!("azure not configured — run `wisp --setup` to link");
    }

    let (snap_tx, snap_rx)     = watch::channel::<Option<telemetry::Snapshot>>(None);
    let (action_tx, action_rx) = mpsc::channel::<RemoteActionRequest>(16);

    // ── polling task ──────────────────────────────────────────────────────────
    {
        let poll_cfg = PollLoopConfig {
            host: cfg.host.clone(),
            port: cfg.port,
            user: cfg.user.clone(),
            transport: cfg.transport,
            docker_interval: cfg.interval,
            azure_cfg: cfg.azure.clone(),
        };
        let tx = snap_tx.clone();
        tokio::spawn(async move {
            // restart_rx is consumed on first successful connection.
            // After a crash + reconnect the channel is gone — the stale banner
            // in the TUI signals to the user that a reconnect is in progress.
            let _ = poll_loop(poll_cfg, tx, action_rx).await
                .map_err(|e| error!("poll_loop exited: {e:#}"));
        });
    }

    // ── web server ────────────────────────────────────────────────────────────
    {
        let state = web::WebState {
            snapshot_rx: snap_rx.clone(),
            action_tx: action_tx.clone(),
        };
        let port  = cfg.web_port;
        tokio::spawn(async move {
            let addr = format!("127.0.0.1:{port}");
            tracing::info!("web dashboard → http://{addr}");
            let listener = tokio::net::TcpListener::bind(&addr).await
                .expect("failed to bind web port");
            axum::serve(listener, web::router(state)).await
                .expect("web server error");
        });
    }

    // ── TUI (blocks; tokio keeps polling + web tasks alive) ──────────────────
    tokio::task::block_in_place(|| tui::run(&cfg.host, snap_rx, action_tx))?;

    Ok(())
}

async fn poll_loop(
    cfg: PollLoopConfig,
    tx: watch::Sender<Option<telemetry::Snapshot>>,
    mut action_rx: mpsc::Receiver<RemoteActionRequest>,
) -> Result<()> {
    let mut session = ssh::RemoteSession::connect(
        &cfg.host,
        cfg.port,
        &cfg.user,
        cfg.transport,
    ).await?;
    let http = reqwest::Client::new();

    let mut azure_token: Option<String> = if cfg.azure_cfg.is_some() {
        azure::access_token().await.map_err(|e| warn!("azure token: {e:#}")).ok()
    } else {
        None
    };
    let mut current_azure: Option<azure::DbMetrics> = None;

    let mut docker_tick = tokio::time::interval(cfg.docker_interval);
    let mut azure_tick  = tokio::time::interval(Duration::from_secs(30));
    azure_tick.reset(); // skip first Azure tick; Docker data is priority on startup

    loop {
        tokio::select! {
            _ = docker_tick.tick() => {
                let mut snap = telemetry::collect_docker(&cfg.host, &mut session).await?;
                snap.azure_db = current_azure.clone();
                snap.azure_db_name = cfg.azure_cfg.as_ref().map(|az| az.server_name.clone());
                snap.azure_db_type = cfg.azure_cfg.as_ref().map(|az| match az.server_type {
                    azure::ServerType::MySQL => "MySQL".to_string(),
                    azure::ServerType::PostgreSQLFlexible => "PostgreSQL Flexible".to_string(),
                });
                if tx.send(Some(snap)).is_err() {
                    return Ok(()); // all receivers dropped — clean exit
                }
            }

            _ = azure_tick.tick() => {
                if let (Some(az_cfg), Some(token)) = (cfg.azure_cfg.as_ref(), &azure_token) {
                    match tokio::time::timeout(
                        Duration::from_secs(10),
                        azure::fetch(az_cfg, &http, token),
                    ).await {
                        Ok(Ok(m))  => current_azure = Some(m),
                        Ok(Err(e)) => {
                            warn!("azure fetch: {e:#}");
                            azure::refresh_token(&mut azure_token).await;
                        }
                        Err(_) => warn!("azure fetch timed out"),
                    }
                }
            }

            Some(request) = action_rx.recv() => {
                let result = match request.action {
                    RemoteAction::Start { name } => {
                        tracing::debug!("starting container: {name}");
                        match tokio::time::timeout(
                            Duration::from_secs(30),
                            session.exec(&format!("docker start {}", shell_quote(&name))),
                        ).await {
                            Ok(Ok(output)) => RemoteActionResult {
                                title: format!("Start: {name}"),
                                output: if output.trim().is_empty() {
                                    format!("{name} started")
                                } else {
                                    sanitize_for_tui(output)
                                },
                                is_error: false,
                            },
                            Ok(Err(e)) => RemoteActionResult {
                                title: format!("Start: {name}"),
                                output: format!("{e:#}"),
                                is_error: true,
                            },
                            Err(_) => RemoteActionResult {
                                title: format!("Start: {name}"),
                                output: "start timed out after 30s".to_string(),
                                is_error: true,
                            },
                        }
                    }

                    RemoteAction::Stop { name } => {
                        tracing::debug!("stopping container: {name}");
                        match tokio::time::timeout(
                            Duration::from_secs(30),
                            session.exec(&format!("docker stop {}", shell_quote(&name))),
                        ).await {
                            Ok(Ok(output)) => RemoteActionResult {
                                title: format!("Stop: {name}"),
                                output: if output.trim().is_empty() {
                                    format!("{name} stopped")
                                } else {
                                    sanitize_for_tui(output)
                                },
                                is_error: false,
                            },
                            Ok(Err(e)) => RemoteActionResult {
                                title: format!("Stop: {name}"),
                                output: format!("{e:#}"),
                                is_error: true,
                            },
                            Err(_) => RemoteActionResult {
                                title: format!("Stop: {name}"),
                                output: "stop timed out after 30s".to_string(),
                                is_error: true,
                            },
                        }
                    }

                    RemoteAction::Restart { name } => {
                        tracing::debug!("restarting container: {name}");
                        match tokio::time::timeout(
                            Duration::from_secs(30),
                            session.exec(&format!("docker restart {}", shell_quote(&name))),
                        ).await {
                            Ok(Ok(output)) => RemoteActionResult {
                                title: format!("Restart: {name}"),
                                output: if output.trim().is_empty() {
                                    format!("{name} restarted")
                                } else {
                                    sanitize_for_tui(output)
                                },
                                is_error: false,
                            },
                            Ok(Err(e)) => RemoteActionResult {
                                title: format!("Restart: {name}"),
                                output: format!("{e:#}"),
                                is_error: true,
                            },
                            Err(_) => RemoteActionResult {
                                title: format!("Restart: {name}"),
                                output: "restart timed out after 30s".to_string(),
                                is_error: true,
                            },
                        }
                    }

                    RemoteAction::Logs { name } => {
                        tracing::debug!("fetching logs for container: {name}");
                        let command = format!("docker logs -n 50 {}", shell_quote(&name));
                        match tokio::time::timeout(
                            Duration::from_secs(30),
                            session.exec(&command),
                        ).await {
                            Ok(Ok(output)) => RemoteActionResult {
                                title: format!("Logs: {name} (last 50 lines)"),
                                output: if output.trim().is_empty() {
                                    "no log output returned".to_string()
                                } else {
                                    sanitize_for_tui(output)
                                },
                                is_error: false,
                            },
                            Ok(Err(e)) => RemoteActionResult {
                                title: format!("Logs: {name}"),
                                output: format!("{e:#}"),
                                is_error: true,
                            },
                            Err(_) => RemoteActionResult {
                                title: format!("Logs: {name}"),
                                output: "log fetch timed out after 30s".to_string(),
                                is_error: true,
                            },
                        }
                    }

                    RemoteAction::Inspect { name } => {
                        tracing::debug!("inspecting container: {name}");
                        let command = format!("docker inspect {}", shell_quote(&name));
                        match tokio::time::timeout(
                            Duration::from_secs(30),
                            session.exec(&command),
                        ).await {
                            Ok(Ok(output)) => RemoteActionResult {
                                title: format!("Inspect: {name}"),
                                output: sanitize_for_tui(output),
                                is_error: false,
                            },
                            Ok(Err(e)) => RemoteActionResult {
                                title: format!("Inspect: {name}"),
                                output: format!("{e:#}"),
                                is_error: true,
                            },
                            Err(_) => RemoteActionResult {
                                title: format!("Inspect: {name}"),
                                output: "docker inspect timed out after 30s".to_string(),
                                is_error: true,
                            },
                        }
                    }

                    RemoteAction::SystemDf => {
                        tracing::debug!("fetching docker disk usage");
                        match tokio::time::timeout(
                            Duration::from_secs(20),
                            session.exec("docker system df"),
                        ).await {
                            Ok(Ok(output)) => RemoteActionResult {
                                title: "Docker Disk Usage".to_string(),
                                output: sanitize_for_tui(output),
                                is_error: false,
                            },
                            Ok(Err(e)) => RemoteActionResult {
                                title: "Docker Disk Usage".to_string(),
                                output: format!("{e:#}"),
                                is_error: true,
                            },
                            Err(_) => RemoteActionResult {
                                title: "Docker Disk Usage".to_string(),
                                output: "docker system df timed out after 20s".to_string(),
                                is_error: true,
                            },
                        }
                    }

                    RemoteAction::Prune => {
                        tracing::debug!("pruning stopped containers");
                        match tokio::time::timeout(
                            Duration::from_secs(45),
                            session.exec("docker container prune -f"),
                        ).await {
                            Ok(Ok(output)) => RemoteActionResult {
                                title: "Prune: stopped containers".to_string(),
                                output: sanitize_for_tui(output),
                                is_error: false,
                            },
                            Ok(Err(e)) => RemoteActionResult {
                                title: "Prune: stopped containers".to_string(),
                                output: format!("{e:#}"),
                                is_error: true,
                            },
                            Err(_) => RemoteActionResult {
                                title: "Prune: stopped containers".to_string(),
                                output: "docker container prune timed out after 45s".to_string(),
                                is_error: true,
                            },
                        }
                    }
                };

                let _ = request.respond_to.send(result);
            }
        }
    }
}

fn shell_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn sanitize_for_tui(output: String) -> String {
    output.replace("\r\n", "\n").replace('\r', "\n")
}
