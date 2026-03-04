pub mod azure;
pub mod docker;

use anyhow::Result;
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;

use crate::ssh::RemoteSession;

/// A point-in-time snapshot of all telemetry from a single host.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub host: String,
    pub containers: Vec<docker::ContainerInfo>,
    pub stats: Vec<docker::ContainerStats>,
    pub azure_db: Option<azure::DbMetrics>,
    pub azure_db_name: Option<String>,
    pub azure_db_type: Option<String>,
    /// Seconds since UNIX epoch — used by the web frontend for "refreshed N ago".
    pub collected_at: u64,
}

/// Collect Docker telemetry over `session`. Azure is polled separately in main.
pub async fn collect_docker(host: &str, session: &mut RemoteSession) -> Result<Snapshot> {
    debug!("collecting docker ps");
    let ps_raw = session.exec(r#"docker ps --format "{{json .}}""#).await?;
    let containers = docker::parse_ps(&ps_raw)?;

    debug!("collecting docker stats");
    let stats_raw = session
        .exec(r#"docker stats --no-stream --format "{{json .}}""#)
        .await?;
    let stats = docker::parse_stats(&stats_raw)?;

    Ok(Snapshot {
        host: host.to_string(),
        containers,
        stats,
        azure_db: None,
        azure_db_name: None,
        azure_db_type: None,
        collected_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    })
}
