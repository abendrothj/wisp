mod ssh;
mod telemetry;
mod tui;

use anyhow::Result;
use clap::Parser;
use std::{sync::mpsc, time::Duration};
use tracing::error;

/// Lightcontain — Tailscale-native, agentless infrastructure control plane
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Tailscale IP of the target host (100.x.x.x)
    #[arg(short = 'H', long)]
    host: String,

    /// SSH port
    #[arg(short, long, default_value_t = 22)]
    port: u16,

    /// Remote user to connect as
    #[arg(short, long, default_value = "deploy")]
    user: String,

    /// Telemetry poll interval in seconds
    #[arg(short = 'i', long, default_value_t = 5)]
    interval: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Log to a file so we don't corrupt the TUI — enabled via RUST_LOG env var.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("lightcontain=info".parse()?),
        )
        .with_writer(|| {
            // Write logs to stderr; crossterm's alternate screen hides them.
            std::io::stderr()
        })
        .init();

    let args = Args::parse();
    let interval = Duration::from_secs(args.interval);

    let (tx, rx) = mpsc::channel::<telemetry::Snapshot>();

    // SSH polling task — runs on the tokio thread pool.
    // Reconnects automatically on error.
    let host = args.host.clone();
    let user = args.user.clone();
    let port = args.port;
    tokio::spawn(async move {
        loop {
            match poll_loop(&host, port, &user, &tx, interval).await {
                Ok(()) => break, // channel closed (TUI exited) — clean shutdown
                Err(e) => {
                    error!("polling error: {e:#}, reconnecting in 5s");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });

    // Run the TUI on the current thread.
    // block_in_place lets tokio keep running the polling task above.
    tokio::task::block_in_place(|| tui::run(&args.host, rx))?;

    Ok(())
}

/// Connect once, poll until the channel is closed or an error occurs.
async fn poll_loop(
    host: &str,
    port: u16,
    user: &str,
    tx: &mpsc::Sender<telemetry::Snapshot>,
    interval: Duration,
) -> Result<()> {
    let mut session = ssh::TailscaleSession::connect(host, port, user).await?;

    loop {
        let snapshot = telemetry::collect(host, &mut session).await?;

        // If the receiver (TUI) has dropped, return Ok to signal clean exit.
        if tx.send(snapshot).is_err() {
            return Ok(());
        }

        tokio::time::sleep(interval).await;
    }
}
