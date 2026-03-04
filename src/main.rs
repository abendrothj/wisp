mod ssh;
mod telemetry;

use anyhow::Result;
use clap::Parser;

/// Lightcontain — Tailscale-native, agentless infrastructure control plane
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Tailscale IP of the target host (100.x.x.x)
    #[arg(short = 'H', long)]
    host: String,

    /// SSH port
    #[arg(short, long, default_value_t = 22)]
    port: u16,

    /// Remote user to connect as
    #[arg(short, long, default_value = "deploy")]
    user: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("lightcontain=info".parse()?),
        )
        .init();

    let args = Args::parse();

    let mut session = ssh::TailscaleSession::connect(&args.host, args.port, &args.user).await?;
    let snapshot = telemetry::collect(&args.host, &mut session).await?;

    print_snapshot(&snapshot);

    Ok(())
}

fn print_snapshot(snapshot: &telemetry::Snapshot) {
    println!("\n  host: {}\n", snapshot.host);

    // Build a stats lookup by container name for the joined display
    let stats_map: std::collections::HashMap<&str, &telemetry::docker::ContainerStats> = snapshot
        .stats
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect();

    // Header
    println!(
        "  {:<24} {:<10} {:<8} {:<16} {:<20}",
        "CONTAINER", "STATE", "CPU", "MEM", "STATUS"
    );
    println!("  {}", "─".repeat(82));

    for c in &snapshot.containers {
        let (cpu, mem) = stats_map
            .get(c.names.as_str())
            .map(|s| (s.cpu_perc.as_str(), s.mem_usage.as_str()))
            .unwrap_or(("–", "–"));

        println!(
            "  {:<24} {:<10} {:<8} {:<16} {:<20}",
            c.names, c.state, cpu, mem, c.status
        );
    }

    println!();
}
