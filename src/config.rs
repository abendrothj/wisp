use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::telemetry::azure;

// ── file format ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    pub host: HostSection,
    pub azure: Option<AzureSection>,
    #[serde(default)]
    pub web: WebSection,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HostSection {
    pub address: String,
    pub port: u16,
    pub user: String,
    pub interval: u64,
}

impl Default for HostSection {
    fn default() -> Self {
        Self { address: String::new(), port: 22, user: "deploy".into(), interval: 5 }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AzureSection {
    pub subscription_id: String,
    pub resource_group: String,
    pub db_server: String,
    #[serde(default = "default_db_type")]
    pub db_type: String,
}

fn default_db_type() -> String { "postgresql-flexible".into() }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebSection {
    pub port: u16,
}

impl Default for WebSection {
    fn default() -> Self { Self { port: 8080 } }
}

// ── load / save ───────────────────────────────────────────────────────────────

impl Config {
    /// Load from `./wisp.toml` (project-local) or `~/.config/wisp/config.toml`.
    pub fn load() -> Option<Self> {
        // Project-local takes priority
        if let Ok(s) = std::fs::read_to_string("wisp.toml") {
            return toml::from_str(&s).ok();
        }
        let path = global_config_path()?;
        let s = std::fs::read_to_string(path).ok()?;
        toml::from_str(&s).ok()
    }

    pub fn save_global(&self) -> Result<PathBuf> {
        let path = global_config_path().context("cannot determine home directory")?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(path)
    }

    pub fn azure_config(&self) -> Option<azure::AzureConfig> {
        let az = self.azure.as_ref()?;
        let kind = if az.db_type == "mysql" {
            azure::ServerType::MySQL
        } else {
            azure::ServerType::PostgreSQLFlexible
        };
        Some(azure::AzureConfig {
            subscription_id: az.subscription_id.clone(),
            resource_group: az.resource_group.clone(),
            server_name: az.db_server.clone(),
            server_type: kind,
        })
    }
}

fn global_config_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config").join("wisp").join("config.toml"))
}
