use anyhow::{Context, Result, bail};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
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

    /// Spawn `command` on the remote host and stream its stdout line-by-line into
    /// the returned channel.  When the receiver is dropped the child process is
    /// killed automatically.
    pub async fn exec_streaming(&self, command: &str) -> Result<mpsc::Receiver<String>> {
        let mut child = match self.transport {
            Transport::Tailscale => {
                let mut cmd = Command::new("tailscale");
                cmd.args(["ssh", &format!("{}@{}", self.user, self.host), "--", command]);
                cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
                    .context("failed to spawn tailscale ssh for streaming")?
            }
            Transport::Ssh => {
                let mut cmd = Command::new("ssh");
                cmd.args([
                    "-p", &self.port.to_string(),
                    "-o", "BatchMode=yes",
                    "-o", "ConnectTimeout=10",
                    "-o", "StrictHostKeyChecking=yes",
                    &format!("{}@{}", self.user, self.host),
                    command,
                ]);
                cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
                    .context("failed to spawn ssh for streaming")?
            }
        };

        let stdout = child.stdout.take().context("no stdout handle")?;
        let (tx, rx) = mpsc::channel::<String>(256);

        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if tx.send(line).await.is_err() {
                            // receiver dropped — kill the process
                            let _ = child.kill().await;
                            break;
                        }
                    }
                    Ok(None) => break, // process exited cleanly
                    Err(_) => break,
                }
            }
            let _ = child.wait().await;
        });

        Ok(rx)
    }
}
