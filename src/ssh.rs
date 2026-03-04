use anyhow::{Context, Result, bail};
use russh::{client, client::AuthResult, ChannelMsg};
use ssh_key::PublicKey;
use std::{sync::Arc, time::Duration};
use tracing::info;

const EXEC_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

struct TailscaleHandler;

impl client::Handler for TailscaleHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// An authenticated SSH session to a Tailscale node.
pub struct TailscaleSession {
    handle: client::Handle<TailscaleHandler>,
}

impl TailscaleSession {
    pub async fn connect(host: &str, port: u16, user: &str) -> Result<Self> {
        info!("connecting to {}:{} as '{}'", host, port, user);

        let config = Arc::new(client::Config::default());

        let mut handle = tokio::time::timeout(
            CONNECT_TIMEOUT,
            client::connect(config, (host, port), TailscaleHandler),
        )
        .await
        .context("connection timed out")?
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
                     Ensure `tailscale ssh` is enabled and your ACL allows this identity.",
                    remaining_methods
                );
            }
        }

        Ok(Self { handle })
    }

    /// Execute a remote command and return its stdout.
    /// Times out after 30 s so a dropped Tailscale link doesn't hang the poll loop.
    pub async fn exec(&mut self, command: &str) -> Result<String> {
        tokio::time::timeout(EXEC_TIMEOUT, self.exec_inner(command))
            .await
            .with_context(|| format!("SSH exec timed out: `{command}`"))?
    }

    async fn exec_inner(&mut self, command: &str) -> Result<String> {
        let mut channel = self.handle.channel_open_session().await?;
        channel.exec(true, command.as_bytes()).await?;

        let mut stdout = Vec::new();
        let mut exit_code = 0u32;

        loop {
            let Some(msg) = channel.wait().await else { break };
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
