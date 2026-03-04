use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::Serialize;
use tracing::{info, warn};

// ── config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AzureConfig {
    pub subscription_id: String,
    pub resource_group: String,
    pub server_name: String,
    pub server_type: ServerType,
}

#[derive(Debug, Clone)]
pub enum ServerType {
    PostgreSQLFlexible,
    MySQL,
}

impl AzureConfig {
    /// Build from environment variables. Returns `None` if any required var is absent.
    pub fn from_env() -> Option<Self> {
        let subscription_id = std::env::var("AZURE_SUBSCRIPTION_ID").ok()?;
        let resource_group = std::env::var("AZURE_RESOURCE_GROUP").ok()?;
        let server_name = std::env::var("AZURE_DB_SERVER").ok()?;
        let server_type = match std::env::var("AZURE_DB_TYPE").as_deref() {
            Ok("mysql") => ServerType::MySQL,
            _ => ServerType::PostgreSQLFlexible,
        };
        Some(Self { subscription_id, resource_group, server_name, server_type })
    }

    fn provider(&self) -> &str {
        match self.server_type {
            ServerType::PostgreSQLFlexible => "Microsoft.DBforPostgreSQL/flexibleServers",
            ServerType::MySQL => "Microsoft.DBforMySQL/flexibleServers",
        }
    }

    fn metrics_url(&self) -> String {
        format!(
            "https://management.azure.com/subscriptions/{sub}/resourceGroups/{rg}/providers/{prov}/{name}/providers/microsoft.insights/metrics\
             ?api-version=2021-05-01\
             &metricnames=cpu_percent,memory_percent,storage_percent,active_connections\
             &timespan=PT5M\
             &interval=PT5M\
             &aggregation=Average",
            sub = self.subscription_id,
            rg = self.resource_group,
            prov = self.provider(),
            name = self.server_name,
        )
    }
}

// ── metrics ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct DbMetrics {
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub storage_percent: f64,
    pub connections: f64,
}

// ── fetch ─────────────────────────────────────────────────────────────────────

pub async fn fetch(cfg: &AzureConfig, client: &Client, token: &str) -> Result<DbMetrics> {
    info!("fetching azure db metrics for {}", cfg.server_name);

    let body: serde_json::Value = client
        .get(cfg.metrics_url())
        .bearer_auth(token)
        .send()
        .await
        .context("azure monitor request failed")?
        .error_for_status()
        .context("azure monitor returned an error status")?
        .json()
        .await?;

    let mut cpu = 0.0f64;
    let mut memory = 0.0f64;
    let mut storage = 0.0f64;
    let mut connections = 0.0f64;

    if let Some(values) = body["value"].as_array() {
        for m in values {
            let name = m["name"]["value"].as_str().unwrap_or("");
            let v = latest_average(m);
            match name {
                "cpu_percent" => cpu = v,
                "memory_percent" => memory = v,
                "storage_percent" => storage = v,
                "active_connections" => connections = v,
                _ => {}
            }
        }
    }

    Ok(DbMetrics { cpu_percent: cpu, memory_percent: memory, storage_percent: storage, connections })
}

/// Pull the most recent `average` data point from a metric's timeseries.
fn latest_average(metric: &serde_json::Value) -> f64 {
    metric["timeseries"]
        .as_array()
        .and_then(|ts| ts.first())
        .and_then(|t| t["data"].as_array())
        .and_then(|data| data.last())
        .and_then(|pt| pt["average"].as_f64())
        .unwrap_or(0.0)
}

// ── token ─────────────────────────────────────────────────────────────────────

/// Resolve an Azure management-plane bearer token.
///
/// Priority:
/// 1. `AZURE_ACCESS_TOKEN` env var (CI / manual override)
/// 2. `az account get-access-token` (local dev, auto-refreshes via the CLI cache)
pub async fn access_token() -> Result<String> {
    if let Ok(t) = std::env::var("AZURE_ACCESS_TOKEN") {
        return Ok(t);
    }

    info!("fetching azure access token via `az` CLI");
    let out = tokio::process::Command::new("az")
        .args([
            "account",
            "get-access-token",
            "--resource",
            "https://management.azure.com/",
            "--query",
            "accessToken",
            "-o",
            "tsv",
        ])
        .output()
        .await
        .context("`az account get-access-token` failed — install Azure CLI or set AZURE_ACCESS_TOKEN")?;

    if !out.status.success() {
        bail!(
            "az account get-access-token exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if token.is_empty() {
        bail!("az returned an empty token — are you logged in? (`az login`)");
    }

    Ok(token)
}

/// Attempt to refresh the token; log a warning and keep the old one on failure.
pub async fn refresh_token(current: &mut Option<String>) {
    match access_token().await {
        Ok(t) => *current = Some(t),
        Err(e) => warn!("azure token refresh failed: {e:#}"),
    }
}
