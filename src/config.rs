use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    #[serde(default)]
    pub theme: ThemeSection,
    pub alerts: Option<AlertsSection>,
    #[serde(default)]
    pub profiles: HashMap<String, Profile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HostSection {
    pub address: String,
    pub port: u16,
    pub user: String,
    pub interval: u64,
    #[serde(default = "default_transport")]
    pub transport: String,
}

fn default_transport() -> String { "tailscale".into() }

impl Default for HostSection {
    fn default() -> Self {
        Self {
            address: String::new(),
            port: 22,
            user: "deploy".into(),
            interval: 5,
            transport: default_transport(),
        }
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ThemeSection {
    pub accent: String,
    pub border: String,
    pub muted: String,
    pub text: String,
    pub success: String,
    pub warning: String,
    pub danger: String,
    pub panel: String,
    pub selection_fg: String,
    pub selection_bg: String,
}

impl Default for ThemeSection {
    fn default() -> Self {
        Self {
            accent: "cyan".into(),
            border: "blue".into(),
            muted: "darkgray".into(),
            text: "white".into(),
            success: "green".into(),
            warning: "yellow".into(),
            danger: "red".into(),
            panel: "black".into(),
            selection_fg: "black".into(),
            selection_bg: "cyan".into(),
        }
    }
}

/// CPU / memory alert thresholds used to colour-code metrics and surface banners.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AlertsSection {
    #[serde(default = "default_cpu_warn")]
    pub cpu_warn: f64,
    #[serde(default = "default_cpu_crit")]
    pub cpu_crit: f64,
    #[serde(default = "default_mem_warn")]
    pub mem_warn: f64,
    #[serde(default = "default_mem_crit")]
    pub mem_crit: f64,
}

fn default_cpu_warn() -> f64 { 50.0 }
fn default_cpu_crit() -> f64 { 80.0 }
fn default_mem_warn() -> f64 { 50.0 }
fn default_mem_crit() -> f64 { 80.0 }

impl Default for AlertsSection {
    fn default() -> Self {
        Self {
            cpu_warn: default_cpu_warn(),
            cpu_crit: default_cpu_crit(),
            mem_warn: default_mem_warn(),
            mem_crit: default_mem_crit(),
        }
    }
}

/// A named server profile.  All fields are optional; unset values fall back to
/// the top-level config then to compiled defaults.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Profile {
    pub address: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub interval: Option<u64>,
    pub transport: Option<String>,
    pub azure: Option<AzureSection>,
    pub web_port: Option<u16>,
    pub theme: Option<ThemeSection>,
    pub alerts: Option<AlertsSection>,
}

impl Profile {
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

    pub fn transport(&self) -> Option<crate::ssh::Transport> {
        let t = self.transport.as_deref()?;
        if t.eq_ignore_ascii_case("ssh") {
            Some(crate::ssh::Transport::Ssh)
        } else {
            Some(crate::ssh::Transport::Tailscale)
        }
    }
}

// ── load / save ───────────────────────────────────────────────────────────────

impl Config {
    /// Load from `./wisp.toml` (project-local) or `~/.config/wisp/config.toml`.
    pub fn load() -> Option<Self> {
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

    pub fn get_profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }
}

impl HostSection {
    pub fn transport(&self) -> crate::ssh::Transport {
        if self.transport.eq_ignore_ascii_case("ssh") {
            crate::ssh::Transport::Ssh
        } else {
            crate::ssh::Transport::Tailscale
        }
    }
}

fn global_config_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config").join("wisp").join("config.toml"))
}
