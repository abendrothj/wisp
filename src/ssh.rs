use anyhow::{Context, Result, bail};
use std::time::Duration;
use tokio::process::Command;
use tracing::debug;

const EXEC_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Tailscale,
    Ssh,
}

pub struct RemoteSession {
    host: String,
    port: u16,
    user: String,
    transport: Transport,
}

impl RemoteSession {
    pub async fn connect(host: &str, port: u16, user: &str, transport: Transport) -> Result<Self> {
        debug!("connecting {:?} to {}:{} as '{}'", transport, host, port, user);

        let mut session = Self {
            host: host.to_string(),
            port,
            user: user.to_string(),
            transport,
        };

        let _ = session.exec("true").await.with_context(|| {
            format!(
                "failed to establish remote session to {}@{}:{} via {:?}",
                session.user, session.host, session.port, session.transport
            )
        })?;

        Ok(session)
    }

    pub async fn exec(&mut self, command: &str) -> Result<String> {
        let output = match self.transport {
            Transport::Tailscale => {
                let mut cmd = Command::new("tailscale");
                cmd.args([
                    "ssh",
                    &format!("{}@{}", self.user, self.host),
                    "--",
                    command,
                ]);
                tokio::time::timeout(EXEC_TIMEOUT, cmd.output())
                    .await
                    .with_context(|| format!("Tailscale SSH exec timed out: `{command}`"))?
                    .context("failed to run `tailscale ssh` command")?
            }
            Transport::Ssh => {
                let mut cmd = Command::new("ssh");
                cmd.args([
                    "-p",
                    &self.port.to_string(),
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "ConnectTimeout=10",
                    "-o",
                    "StrictHostKeyChecking=yes",
                    &format!("{}@{}", self.user, self.host),
                    command,
                ]);
                tokio::time::timeout(EXEC_TIMEOUT, cmd.output())
                    .await
                    .with_context(|| format!("SSH exec timed out: `{command}`"))?
                    .context("failed to run local `ssh` command")?
            }
        };

        if !output.status.success() {
            bail!(
                "remote command `{}` failed: {}",
                command,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}
