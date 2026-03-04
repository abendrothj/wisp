use anyhow::{Context, Result, bail};
use russh::{client, client::AuthResult, ChannelMsg};
use ssh_key::PublicKey;
use std::sync::Arc;
use tracing::info;

struct TailscaleHandler;

impl client::Handler for TailscaleHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        // TOFU within Tailscale mesh — identity is validated by Tailscale ACLs.
        // Phase 5 will add per-host key pinning in config.
        Ok(true)
    }
}

/// An authenticated SSH session to a Tailscale node.
pub struct TailscaleSession {
    handle: client::Handle<TailscaleHandler>,
}

impl TailscaleSession {
    /// Connect to `host:port` and authenticate as `user` via Tailscale `none` auth.
    pub async fn connect(host: &str, port: u16, user: &str) -> Result<Self> {
        info!("connecting to {}:{} as '{}'", host, port, user);

        let config = Arc::new(client::Config::default());
        let mut handle = client::connect(config, (host, port), TailscaleHandler)
            .await
            .with_context(|| format!("failed to connect to {}:{}", host, port))?;

        match handle
            .authenticate_none(user)
            .await
            .context("none auth failed — is `tailscale ssh` enabled on the remote host?")?
        {
            AuthResult::Success => {
                info!("authenticated via Tailscale mesh identity");
            }
            AuthResult::Failure { remaining_methods, .. } => {
                bail!(
                    "authentication rejected (remaining methods: {:?}). \
                     Ensure `tailscale ssh` is enabled on the remote host and your \
                     ACL policy allows this identity.",
                    remaining_methods
                );
            }
        }

        Ok(Self { handle })
    }

    /// Execute a command and return its stdout as a `String`.
    /// Propagates remote exit codes as errors.
    pub async fn exec(&mut self, command: &str) -> Result<String> {
        let mut channel = self.handle.channel_open_session().await?;
        channel.exec(true, command.as_bytes()).await?;

        let mut stdout = Vec::new();
        let mut exit_code = 0u32;

        loop {
            let Some(msg) = channel.wait().await else {
                break;
            };
            match msg {
                ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => exit_code = exit_status,
                ChannelMsg::Eof => break,
                _ => {}
            }
        }

        if exit_code != 0 {
            bail!("remote command `{}` exited with status {}", command, exit_code);
        }

        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }
}
