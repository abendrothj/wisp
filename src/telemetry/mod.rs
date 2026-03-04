pub mod docker;

use anyhow::Result;
use docker::{ContainerInfo, ContainerStats};
use tracing::info;

use crate::ssh::TailscaleSession;

/// A point-in-time snapshot of all telemetry from a single host.
#[derive(Debug)]
pub struct Snapshot {
    pub host: String,
    pub containers: Vec<ContainerInfo>,
    pub stats: Vec<ContainerStats>,
}

/// Collect a full telemetry snapshot from `session`.
/// Runs `docker ps` and `docker stats` in sequence over the same SSH session.
pub async fn collect(host: &str, session: &mut TailscaleSession) -> Result<Snapshot> {
    info!("collecting docker ps");
    let ps_raw = session
        .exec(r#"docker ps --format "{{json .}}""#)
        .await?;
    let containers = docker::parse_ps(&ps_raw)?;

    info!("collecting docker stats");
    let stats_raw = session
        .exec(r#"docker stats --no-stream --format "{{json .}}""#)
        .await?;
    let stats = docker::parse_stats(&stats_raw)?;

    Ok(Snapshot {
        host: host.to_string(),
        containers,
        stats,
    })
}
