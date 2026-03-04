use anyhow::{Context, Result};
use clap::Parser;
use russh::{client, client::AuthResult, ChannelMsg};
use ssh_key::PublicKey;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tracing::info;

/// Lightcontain — Tailscale-native infrastructure control plane
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Tailscale IP of the target host (100.x.x.x)
    #[arg(short = 'H', long)]
    host: String,

    /// SSH port (default: 22)
    #[arg(short, long, default_value_t = 22)]
    port: u16,

    /// Remote user to connect as
    #[arg(short, long, default_value = "root")]
    user: String,

    /// Command to run on the remote host
    #[arg(short, long, default_value = "uptime && docker ps --format '{{.Names}}\\t{{.Status}}'")]
    command: String,
}

/// Minimal russh client handler.
/// Tailscale SSH validates identity via the mesh — we just need to handle
/// the server's host key check (trust-on-first-use within Tailscale network).
struct TailscaleHandler;

impl client::Handler for TailscaleHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        // Within a Tailscale mesh, nodes are authenticated by Tailscale identity.
        // TOFU is acceptable here; Phase 5 will pin keys per-host in config.
        Ok(true)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("lightcontain=info".parse()?),
        )
        .init();

    let args = Args::parse();

    info!("connecting to {}:{} as '{}'", args.host, args.port, args.user);

    let config = Arc::new(client::Config::default());

    let mut session = client::connect(config, (args.host.as_str(), args.port), TailscaleHandler)
        .await
        .with_context(|| format!("failed to connect to {}:{}", args.host, args.port))?;

    // Tailscale SSH accepts the `none` auth method — its daemon validates the
    // connecting Tailscale identity against your ACL policy server-side.
    match session
        .authenticate_none(&args.user)
        .await
        .context("none auth failed — is tailscale ssh enabled on the remote host?")?
    {
        AuthResult::Success => {
            info!("authenticated via Tailscale mesh identity");
        }
        AuthResult::Failure { remaining_methods, .. } => {
            anyhow::bail!(
                "authentication rejected (remaining methods: {:?}). \
                 Ensure `tailscale ssh` is enabled on the remote host and your \
                 ACL policy allows this identity.",
                remaining_methods
            );
        }
    }

    let mut channel = session.channel_open_session().await?;
    channel.exec(true, args.command.as_bytes()).await?;

    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    let mut exit_code = 0u32;

    loop {
        let Some(msg) = channel.wait().await else {
            break;
        };
        match msg {
            ChannelMsg::Data { data } => {
                stdout.write_all(&data).await?;
            }
            ChannelMsg::ExtendedData { data, .. } => {
                stderr.write_all(&data).await?;
            }
            ChannelMsg::ExitStatus { exit_status } => {
                exit_code = exit_status;
            }
            ChannelMsg::Eof => break,
            _ => {}
        }
    }

    stdout.flush().await?;
    stderr.flush().await?;

    if exit_code != 0 {
        anyhow::bail!("remote command exited with status {}", exit_code);
    }

    Ok(())
}
