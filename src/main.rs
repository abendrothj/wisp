mod config;
mod setup;
mod ssh;
mod telemetry;
mod tui;
mod web;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::time::Duration;
use telemetry::azure;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{error, warn};

use config::{AlertsSection, Config};

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

/// Request to stream docker logs for a container.
pub struct LogStreamRequest {
    pub name: String,
    /// Send the streaming receiver back to the caller.
    pub response_tx: oneshot::Sender<mpsc::Receiver<String>>,
}

/// wisp — Tailscale-native, agentless infrastructure control plane
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Option<SubCommand>,

    /// Interactive Azure setup wizard — auto-detects resources via `az` CLI
    #[arg(long)]
    setup: bool,

    /// Tailscale IP of the target host (overrides config)
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

#[derive(Subcommand, Debug)]
enum SubCommand {
    /// Launch with a named profile from config
    Up {
        /// Profile name defined under [profiles.<name>] in wisp.toml
        profile: String,
    },
}

// ── resolved runtime config ───────────────────────────────────────────────────

struct Resolved {
    host:      String,
    port:      u16,
    user:      String,
    interval:  Duration,
    web_port:  u16,
    azure:     Option<azure::AzureConfig>,
    transport: ssh::Transport,
    theme:     config::ThemeSection,
    alerts:    AlertsSection,
}

struct PollLoopConfig {
    host:            String,
    port:            u16,
    user:            String,
    transport:       ssh::Transport,
    docker_interval: Duration,
    azure_cfg:       Option<azure::AzureConfig>,
}

impl Resolved {
    fn build(
        args: &Args,
        file: Option<Config>,
        profile: Option<&config::Profile>,
    ) -> Result<Self> {
        let file = file.unwrap_or_default();

        // Resolution order: CLI flag > profile > global config > default
        let host = args.host.clone()
            .or_else(|| profile.map(|p| p.address.clone()).filter(|s| !s.is_empty()))
            .or_else(|| Some(file.host.address.clone()).filter(|s| !s.is_empty()))
            .context(
                "no host specified — pass -H <tailscale-ip>, use `wisp up <profile>`, \
                 or run `wisp --setup` to write a config file",
            )?;

        let port = args.port
            .or_else(|| profile.and_then(|p| p.port))
            .unwrap_or(file.host.port);

        let user = args.user.clone()
            .or_else(|| profile.and_then(|p| p.user.clone()))
            .unwrap_or_else(|| file.host.user.clone());

        let interval = Duration::from_secs(
            args.interval
                .or_else(|| profile.and_then(|p| p.interval))
                .unwrap_or(file.host.interval),
        );

        let web_port = args.web_port
            .or_else(|| profile.and_then(|p| p.web_port))
            .unwrap_or(file.web.port);

        let transport = if args.ssh {
            ssh::Transport::Ssh
        } else {
            profile.and_then(|p| p.transport())
                .unwrap_or_else(|| file.host.transport())
        };

        let azure = if args.azure_subscription_id.is_some() {
            args_azure_config(args)
        } else {
            profile.and_then(|p| p.azure_config())
                .or_else(|| file.azure_config())
                .or_else(azure::AzureConfig::from_env)
        };

        let theme = profile.and_then(|p| p.theme.clone())
            .unwrap_or(file.theme);

        let alerts = profile.and_then(|p| p.alerts.clone())
            .or_else(|| file.alerts.clone())
            .unwrap_or_default();

        Ok(Self { host, port, user, interval, web_port, azure, transport, theme, alerts })
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

    // ── profile resolution ────────────────────────────────────────────────────
    //
    // Priority:
    //   1. `wisp up <profile>`  — explicit subcommand
    //   2. `-H <host>`          — bare host flag, skip picker
    //   3. Single profile       — auto-selected silently
    //   4. Multiple profiles    — interactive TUI picker
    //   5. No profiles / no host → Resolved::build will surface a clear error
    let profile: Option<config::Profile> = match &args.command {
        Some(SubCommand::Up { profile: name }) => {
            let cfg = file_config.as_ref()
                .context("no config file found — run `wisp --setup` or create wisp.toml")?;
            let p = cfg.get_profile(name)
                .with_context(|| format!("profile '{name}' not found in config"))?;
            Some(p.clone())
        }
        None if args.host.is_none() => {
            // Collect available profiles (sorted alphabetically for stable order).
            let mut profiles: Vec<(String, config::Profile)> = file_config
                .as_ref()
                .map(|c| {
                    let mut v: Vec<_> = c.profiles.iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    v.sort_by(|a, b| a.0.cmp(&b.0));
                    v
                })
                .unwrap_or_default();

            match profiles.len() {
                0 => {
                    // No profiles and no host — show the onboarding screen then exit.
                    if file_config.is_none() || file_config.as_ref().map(|c| c.host.address.is_empty()).unwrap_or(true) {
                        let theme_cfg = file_config.as_ref()
                            .map(|c| c.theme.clone())
                            .unwrap_or_default();
                        tokio::task::block_in_place(|| tui::picker::run_welcome(&theme_cfg))?;
                        return Ok(());
                    }
                    None // has a [host] section — fall through to flag-based resolution
                }
                1 => {
                    // Single profile: auto-select without showing the picker.
                    Some(profiles.remove(0).1)
                }
                _ => {
                    // Multiple profiles: show the interactive picker.
                    let theme_cfg = file_config.as_ref()
                        .map(|c| c.theme.clone())
                        .unwrap_or_default();

                    let selected = tokio::task::block_in_place(|| {
                        tui::picker::run(&profiles, &theme_cfg)
                    })?;

                    match selected {
                        None => return Ok(()), // user quit
                        Some(name) => profiles.into_iter()
                            .find(|(k, _)| k == &name)
                            .map(|(_, p)| p),
                    }
                }
            }
        }
        None => None,
    };

    let cfg = Resolved::build(&args, file_config, profile.as_ref())?;

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
    let (stream_tx, stream_rx) = mpsc::channel::<LogStreamRequest>(4);

    // ── polling task ──────────────────────────────────────────────────────────
    {
        let poll_cfg = PollLoopConfig {
            host:            cfg.host.clone(),
            port:            cfg.port,
            user:            cfg.user.clone(),
            transport:       cfg.transport,
            docker_interval: cfg.interval,
            azure_cfg:       cfg.azure.clone(),
        };
        let tx = snap_tx.clone();
        tokio::spawn(async move {
            let _ = poll_loop(poll_cfg, tx, action_rx, stream_rx).await
                .map_err(|e| error!("poll_loop exited: {e:#}"));
        });
    }

    // ── web server ────────────────────────────────────────────────────────────
    {
        let state = web::WebState {
            snapshot_rx: snap_rx.clone(),
            action_tx: action_tx.clone(),
        };
        let port = cfg.web_port;
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
    tokio::task::block_in_place(|| {
        tui::run(
            &cfg.host,
            snap_rx,
            action_tx,
            stream_tx,
            cfg.theme.clone(),
            cfg.alerts.clone(),
            cfg.web_port,
        )
    })?;

    Ok(())
}

async fn poll_loop(
    cfg: PollLoopConfig,
    tx: watch::Sender<Option<telemetry::Snapshot>>,
    mut action_rx: mpsc::Receiver<RemoteActionRequest>,
    mut stream_rx: mpsc::Receiver<LogStreamRequest>,
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
    azure_tick.reset();

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
                    return Ok(());
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

            Some(req) = stream_rx.recv() => {
                let cmd = format!("docker logs -f --tail 200 {}", shell_quote(&req.name));
                match session.exec_streaming(&cmd).await {
                    Ok(chunk_rx) => { let _ = req.response_tx.send(chunk_rx); }
                    Err(e) => warn!("log stream failed for {}: {e:#}", req.name),
                }
            }

            Some(request) = action_rx.recv() => {
                let result = dispatch_action(&mut session, request.action).await;
                let _ = request.respond_to.send(result);
            }
        }
    }
}

async fn dispatch_action(session: &mut ssh::RemoteSession, action: RemoteAction) -> RemoteActionResult {
    match action {
        RemoteAction::Start { name } => {
            tracing::debug!("starting container: {name}");
            timed_exec(session, &format!("docker start {}", shell_quote(&name)), 30,
                format!("Start: {name}"),
                Some(format!("{name} started")),
            ).await
        }

        RemoteAction::Stop { name } => {
            tracing::debug!("stopping container: {name}");
            timed_exec(session, &format!("docker stop {}", shell_quote(&name)), 30,
                format!("Stop: {name}"),
                Some(format!("{name} stopped")),
            ).await
        }

        RemoteAction::Restart { name } => {
            tracing::debug!("restarting container: {name}");
            timed_exec(session, &format!("docker restart {}", shell_quote(&name)), 30,
                format!("Restart: {name}"),
                Some(format!("{name} restarted")),
            ).await
        }

        RemoteAction::Logs { name } => {
            tracing::debug!("fetching logs for container: {name}");
            timed_exec(session, &format!("docker logs -n 50 {}", shell_quote(&name)), 30,
                format!("Logs: {name} (last 50 lines)"),
                None,
            ).await
        }

        RemoteAction::Inspect { name } => {
            tracing::debug!("inspecting container: {name}");
            timed_exec(session, &format!("docker inspect {}", shell_quote(&name)), 30,
                format!("Inspect: {name}"),
                None,
            ).await
        }

        RemoteAction::SystemDf => {
            tracing::debug!("fetching docker disk usage");
            timed_exec(session, "docker system df", 20,
                "Docker Disk Usage".to_string(),
                None,
            ).await
        }

        RemoteAction::Prune => {
            tracing::debug!("pruning stopped containers");
            timed_exec(session, "docker container prune -f", 45,
                "Prune: stopped containers".to_string(),
                None,
            ).await
        }
    }
}

async fn timed_exec(
    session: &mut ssh::RemoteSession,
    command: &str,
    timeout_secs: u64,
    title: String,
    empty_ok: Option<String>,
) -> RemoteActionResult {
    match tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        session.exec(command),
    ).await {
        Ok(Ok(output)) => RemoteActionResult {
            title,
            output: if output.trim().is_empty() {
                empty_ok.unwrap_or_else(|| "no output".to_string())
            } else {
                sanitize_for_tui(output)
            },
            is_error: false,
        },
        Ok(Err(e)) => RemoteActionResult {
            title,
            output: format!("{e:#}"),
            is_error: true,
        },
        Err(_) => RemoteActionResult {
            title,
            output: format!("command timed out after {timeout_secs}s"),
            is_error: true,
        },
    }
}

fn shell_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn sanitize_for_tui(output: String) -> String {
    output.replace("\r\n", "\n").replace('\r', "\n")
}
