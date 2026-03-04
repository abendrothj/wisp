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
use tokio::sync::{mpsc, watch};
use tracing::{error, warn};

use config::Config;

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

        // CLI Azure args take priority over config file
        let azure = if args.azure_subscription_id.is_some() {
            args_azure_config(args)
        } else {
            file.azure_config()
        };

        Ok(Self { host, port, user, interval, web_port, azure })
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("wisp=info".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    if args.setup {
        return setup::run(args.host.as_deref()).await;
    }

    let file_config = Config::load();
    let cfg = Resolved::build(&args, file_config)?;

    if cfg.azure.is_some() {
        tracing::info!("azure db monitoring enabled");
    } else {
        tracing::info!("azure not configured — run `wisp --setup` to link");
    }

    let (snap_tx, snap_rx)     = watch::channel::<Option<telemetry::Snapshot>>(None);
    let (restart_tx, restart_rx) = mpsc::channel::<String>(8);

    // ── polling task ──────────────────────────────────────────────────────────
    {
        let host = cfg.host.clone();
        let user = cfg.user.clone();
        let (port, interval, azure) = (cfg.port, cfg.interval, cfg.azure.clone());
        let tx = snap_tx.clone();
        tokio::spawn(async move {
            // restart_rx is consumed on first successful connection.
            // After a crash + reconnect the channel is gone — the stale banner
            // in the TUI signals to the user that a reconnect is in progress.
            let _ = poll_loop(&host, port, &user, interval, &azure, &tx, restart_rx).await
                .map_err(|e| error!("poll_loop exited: {e:#}"));
        });
    }

    // ── web server ────────────────────────────────────────────────────────────
    {
        let state = web::WebState { snapshot_rx: snap_rx.clone() };
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
    tokio::task::block_in_place(|| tui::run(&cfg.host, snap_rx, restart_tx))?;

    Ok(())
}

async fn poll_loop(
    host: &str,
    port: u16,
    user: &str,
    docker_interval: Duration,
    azure_cfg: &Option<azure::AzureConfig>,
    tx: &watch::Sender<Option<telemetry::Snapshot>>,
    mut restart_rx: mpsc::Receiver<String>,
) -> Result<()> {
    let mut session = ssh::TailscaleSession::connect(host, port, user).await?;
    let http = reqwest::Client::new();

    let mut azure_token: Option<String> = if azure_cfg.is_some() {
        azure::access_token().await.map_err(|e| warn!("azure token: {e:#}")).ok()
    } else {
        None
    };
    let mut current_azure: Option<azure::DbMetrics> = None;

    let mut docker_tick = tokio::time::interval(docker_interval);
    let mut azure_tick  = tokio::time::interval(Duration::from_secs(30));
    azure_tick.reset(); // skip first Azure tick; Docker data is priority on startup

    loop {
        tokio::select! {
            _ = docker_tick.tick() => {
                let mut snap = telemetry::collect_docker(host, &mut session).await?;
                snap.azure_db = current_azure.clone();
                if tx.send(Some(snap)).is_err() {
                    return Ok(()); // all receivers dropped — clean exit
                }
            }

            _ = azure_tick.tick() => {
                if let (Some(cfg), Some(token)) = (azure_cfg, &azure_token) {
                    match tokio::time::timeout(
                        Duration::from_secs(10),
                        azure::fetch(cfg, &http, token),
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

            Some(name) = restart_rx.recv() => {
                tracing::info!("restarting container: {name}");
                match tokio::time::timeout(
                    Duration::from_secs(30),
                    session.exec(&format!("docker restart {name}")),
                ).await {
                    Ok(Ok(_))  => tracing::info!("{name} restarted"),
                    Ok(Err(e)) => warn!("restart {name}: {e:#}"),
                    Err(_)     => warn!("restart {name} timed out"),
                }
            }
        }
    }
}
