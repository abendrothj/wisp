#![allow(dead_code)] // fields consumed in Phase 3 TUI / Phase 4 web UI

use anyhow::Result;
use serde::Deserialize;

// ── docker ps --format "{{json .}}" ──────────────────────────────────────────

/// One entry from `docker ps --format "{{json .}}"`.
/// Docker uses PascalCase field names in its JSON output.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct ContainerInfo {
    #[serde(rename = "Names")]
    pub names: String,
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Image")]
    pub image: String,
    #[serde(rename = "State")]
    pub state: String,
    /// Human-readable status, e.g. "Up 3 hours (healthy)"
    #[serde(rename = "Status")]
    pub status: String,
    #[serde(rename = "Ports")]
    pub ports: String,
}

// ── docker stats --no-stream --format "{{json .}}" ───────────────────────────

/// One entry from `docker stats --no-stream --format "{{json .}}"`.
/// CPU/mem/net values arrive as strings ("0.12%", "123MiB / 1GiB", etc.)
/// and will be parsed for display in the TUI (Phase 3).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct ContainerStats {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "CPUPerc")]
    pub cpu_perc: String,
    #[serde(rename = "MemUsage")]
    pub mem_usage: String,
    #[serde(rename = "MemPerc")]
    pub mem_perc: String,
    #[serde(rename = "NetIO")]
    pub net_io: String,
    #[serde(rename = "BlockIO")]
    pub block_io: String,
    #[serde(rename = "PIDs")]
    pub pids: String,
}

// ── parsers ───────────────────────────────────────────────────────────────────

/// Parse newline-delimited JSON output from `docker ps --format "{{json .}}"`.
pub fn parse_ps(raw: &str) -> Result<Vec<ContainerInfo>> {
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

/// Parse newline-delimited JSON output from `docker stats --no-stream --format "{{json .}}"`.
pub fn parse_stats(raw: &str) -> Result<Vec<ContainerStats>> {
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}
