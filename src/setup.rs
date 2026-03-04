/// Interactive Azure setup wizard.
///
/// Calls the `az` CLI to auto-detect the user's subscription and PostgreSQL /
/// MySQL Flexible Servers, lets them pick one, then writes `~/.config/wisp/config.toml`.
use anyhow::{Context, Result, bail};
use std::io::{Write, stdin, stdout};

use crate::config::{AzureSection, Config};

pub async fn run(host: Option<&str>) -> Result<()> {
    println!("\n  wisp — Azure setup wizard\n");

    // ── 1. Check az CLI ───────────────────────────────────────────────────────
    check_az_cli().await?;

    // ── 2. Resolve subscription ───────────────────────────────────────────────
    let (sub_id, sub_name) = get_subscription().await?;
    println!("  Subscription : {} ({})\n", sub_name, sub_id);

    // ── 3. Discover DB servers ────────────────────────────────────────────────
    let servers = discover_servers(&sub_id).await?;

    if servers.is_empty() {
        bail!(
            "no PostgreSQL or MySQL Flexible Servers found in subscription '{sub_name}'.\n\
             Check that you're logged in to the right account: `az account show`"
        );
    }

    println!("  Found {} server(s):\n", servers.len());
    for (i, s) in servers.iter().enumerate() {
        println!("  [{:>2}]  {}  (rg: {},  type: {})", i + 1, s.name, s.resource_group, s.kind);
    }

    // ── 4. Select ─────────────────────────────────────────────────────────────
    let idx = prompt_index("\n  Select server", servers.len())?;
    let chosen = &servers[idx];
    println!("\n  Selected: {}", chosen.name);

    // ── 5. Write config ───────────────────────────────────────────────────────
    let mut config = Config::load().unwrap_or_default();

    config.azure = Some(AzureSection {
        subscription_id: sub_id,
        resource_group: chosen.resource_group.clone(),
        db_server: chosen.name.clone(),
        db_type: chosen.kind.clone(),
    });

    // Preserve or set host section if provided
    if let Some(h) = host
        && config.host.address.is_empty() {
        config.host.address = h.to_string();
    }

    let path = config.save_global().context("failed to write config")?;
    println!("\n  Config written to {}\n", path.display());
    println!("  Run: wisp -H {}\n", config.host.address.trim_matches(|c: char| c == ' '));

    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

async fn check_az_cli() -> Result<()> {
    let out = tokio::process::Command::new("az")
        .args(["--version"])
        .output()
        .await
        .context("`az` CLI not found — install it from https://aka.ms/installazureclimacos")?;

    if !out.status.success() {
        bail!("`az --version` failed — is the Azure CLI installed?");
    }
    Ok(())
}

async fn get_subscription() -> Result<(String, String)> {
    let out = tokio::process::Command::new("az")
        .args(["account", "show", "--output", "json"])
        .output()
        .await
        .context("`az account show` failed")?;

    if !out.status.success() {
        bail!(
            "not logged in to Azure — run `az login` first.\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let id   = v["id"].as_str().unwrap_or("").to_string();
    let name = v["name"].as_str().unwrap_or("(unknown)").to_string();
    Ok((id, name))
}

struct Server {
    name: String,
    resource_group: String,
    kind: String, // "postgresql-flexible" | "mysql"
}

async fn discover_servers(subscription_id: &str) -> Result<Vec<Server>> {
    let mut servers = Vec::new();

    // PostgreSQL Flexible
    let pg = az_list(
        &["postgres", "flexible-server", "list", "--subscription", subscription_id],
        "postgresql-flexible",
    )
    .await;

    // MySQL Flexible
    let my = az_list(
        &["mysql", "flexible-server", "list", "--subscription", subscription_id],
        "mysql",
    )
    .await;

    if let Ok(mut pg) = pg { servers.append(&mut pg); }
    if let Ok(mut my) = my { servers.append(&mut my); }

    Ok(servers)
}

async fn az_list(args: &[&str], kind: &str) -> Result<Vec<Server>> {
    let mut cmd_args: Vec<&str> = args.to_vec();
    cmd_args.extend_from_slice(&["--output", "json"]);

    let out = tokio::process::Command::new("az")
        .args(&cmd_args)
        .output()
        .await?;

    if !out.status.success() {
        return Ok(vec![]);
    }

    let items: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let mut servers = Vec::new();

    if let Some(arr) = items.as_array() {
        for item in arr {
            let name = item["name"].as_str().unwrap_or("").to_string();
            let rg   = item["resourceGroup"].as_str().unwrap_or("").to_string();
            if !name.is_empty() {
                servers.push(Server { name, resource_group: rg, kind: kind.to_string() });
            }
        }
    }

    Ok(servers)
}

fn prompt_index(prompt: &str, max: usize) -> Result<usize> {
    loop {
        print!("{} [1-{}]: ", prompt, max);
        stdout().flush()?;

        let mut line = String::new();
        stdin().read_line(&mut line)?;

        match line.trim().parse::<usize>() {
            Ok(n) if n >= 1 && n <= max => return Ok(n - 1),
            _ => println!("  Please enter a number between 1 and {max}."),
        }
    }
}
