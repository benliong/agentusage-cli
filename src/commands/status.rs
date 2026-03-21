use anyhow::Result;
use chrono::Utc;
use clap::Args;
use serde::Serialize;

use crate::config;
use crate::plugin_runtime;
use crate::recommendation;
use crate::snapshot;

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Output machine-readable JSON (stable schema for agent skill use)
    #[arg(long)]
    pub json: bool,

    /// Filter to a single provider by ID
    #[arg(long)]
    pub provider: Option<String>,

    /// Output Markdown table
    #[arg(long)]
    pub markdown: bool,
}

/// Top-level JSON output schema (agent skill surface, stable).
#[derive(Serialize)]
struct StatusOutput {
    schema_version: u32,
    fetched_at: String,
    machine_id: String,
    providers: Vec<ProviderOutput>,
    recommendation: Option<recommendation::RecommendationBlock>,
}

#[derive(Serialize)]
struct ProviderOutput {
    id: String,
    display_name: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<PeriodOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weekly: Option<PeriodOutput>,
    recommendation: String,
}

#[derive(Serialize)]
struct PeriodOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens_used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens_limit: Option<f64>,
    remaining_fraction: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_limit_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requests_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resets_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runs_out_at: Option<String>,
}

pub fn run(args: StatusArgs, verbose: bool) -> Result<()> {
    config::ensure_bundled_plugins_installed()?;

    let plugins_dir = config::effective_plugins_dir()?;
    let machine_id = config::machine_id()?;

    let mut plugins = plugin_runtime::load_plugins(&plugins_dir);

    if let Some(ref provider_id) = args.provider {
        plugins.retain(|p| &p.manifest.id == provider_id);
        if plugins.is_empty() {
            anyhow::bail!("provider '{}' not found", provider_id);
        }
    }

    if plugins.is_empty() {
        if args.json {
            let output = StatusOutput {
                schema_version: 1,
                fetched_at: Utc::now().to_rfc3339(),
                machine_id,
                providers: vec![],
                recommendation: None,
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            eprintln!("No plugins found. Run `au plugins list` to see available plugins.");
        }
        return Ok(());
    }

    // Run all probes
    let app_data_dir = config::app_data_dir()?;
    let outputs: Vec<_> = plugins
        .iter()
        .map(|p| {
            let output = plugin_runtime::run_probe(p, &app_data_dir, env!("CARGO_PKG_VERSION"));
            let snap = snapshot::translate(&output, &p.manifest);
            (p.clone(), snap)
        })
        .collect();

    let snapshots: Vec<_> = outputs.iter().map(|(_, s)| s.clone()).collect();
    let rec = recommendation::compute(&snapshots);

    if args.json {
        let providers = outputs
            .iter()
            .map(|(_, snap)| {
                let period_out = |p: &Option<snapshot::UsagePeriod>| {
                    p.as_ref().map(|u| PeriodOutput {
                        tokens_used: u.tokens_used,
                        tokens_limit: u.tokens_limit,
                        remaining_fraction: u.remaining_fraction,
                        cost_usd: u.cost_usd,
                        cost_limit_usd: u.cost_limit_usd,
                        requests_count: u.requests_count,
                        resets_at: u.resets_at.clone(),
                        runs_out_at: compute_runs_out_at(u),
                    })
                };
                ProviderOutput {
                    id: snap.id.clone(),
                    display_name: snap.display_name.clone(),
                    status: serde_json::to_value(&snap.status)
                        .ok()
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                        .unwrap_or_default(),
                    plan: snap.plan.clone(),
                    session: period_out(&snap.session),
                    weekly: period_out(&snap.weekly),
                    recommendation: serde_json::to_value(&snap.recommendation)
                        .ok()
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                        .unwrap_or_default(),
                }
            })
            .collect();

        let output = StatusOutput {
            schema_version: 1,
            fetched_at: Utc::now().to_rfc3339(),
            machine_id,
            providers,
            recommendation: rec,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if args.markdown {
        print_markdown(&outputs);
    } else {
        print_human(&outputs, rec.as_ref(), verbose);
    }

    Ok(())
}

fn print_human(
    outputs: &[(plugin_runtime::LoadedPlugin, snapshot::ProviderSnapshot)],
    rec: Option<&recommendation::RecommendationBlock>,
    verbose: bool,
) {
    use owo_colors::OwoColorize;

    for (_, snap) in outputs {
        if !verbose
            && matches!(
                snap.status,
                snapshot::ProviderStatus::NotConfigured | snapshot::ProviderStatus::Error
            )
        {
            continue;
        }

        let status_icon = match snap.status {
            snapshot::ProviderStatus::Ok => "●".green().to_string(),
            snapshot::ProviderStatus::Error => "●".red().to_string(),
            snapshot::ProviderStatus::NotConfigured => "●".bright_black().to_string(),
        };

        let plan_suffix = snap
            .plan
            .as_ref()
            .map(|p| format!(" {}", format!("[{p}]").bright_black()))
            .unwrap_or_default();

        println!("{status_icon} {}{plan_suffix}", snap.display_name.bold());

        if let Some(session) = &snap.session {
            print_period("  Session", session);
        }
        if let Some(weekly) = &snap.weekly {
            print_period("  Weekly", weekly);
        }
        if snap.session.is_none() && snap.weekly.is_none() {
            println!("  {}", "not configured".bright_black());
        }
        println!();
    }

    if let Some(rec) = rec {
        println!(
            "→ Best provider: {} ({})",
            rec.best_provider.bold(),
            rec.best_provider_period
        );
        println!("  {}", rec.reason.bright_black());
    }
}

fn print_period(label: &str, period: &snapshot::UsagePeriod) {
    use owo_colors::OwoColorize;

    let pct = period.remaining_fraction;
    let bar = render_bar(pct, 20);
    let color_bar = if pct > 0.5 {
        bar.green().to_string()
    } else if pct > 0.2 {
        bar.yellow().to_string()
    } else {
        bar.red().to_string()
    };

    let usage_str = if let (Some(used), Some(limit)) = (period.tokens_used, period.tokens_limit) {
        format!("{:.0}k / {:.0}k tokens", used / 1000.0, limit / 1000.0)
    } else if let (Some(used), Some(limit)) = (period.cost_usd, period.cost_limit_usd) {
        format!("${:.2} / ${:.2}", used, limit)
    } else {
        format!("{:.0}% remaining", pct * 100.0)
    };

    let reset_str = period
        .resets_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|reset_time| {
            let delta = reset_time.signed_duration_since(Utc::now());
            let total_mins = delta.num_minutes();
            if total_mins <= 0 {
                "resets soon".to_string()
            } else {
                let days = total_mins / (60 * 24);
                let hours = (total_mins % (60 * 24)) / 60;
                let mins = total_mins % 60;
                if days > 0 {
                    format!("resets in {days}d {hours}h")
                } else if hours > 0 {
                    format!("resets in {hours}h {mins}m")
                } else {
                    format!("resets in {mins}m")
                }
            }
        });

    let runs_out_str = compute_runs_out_at(period).map(|iso| {
        if let Ok(t) = chrono::DateTime::parse_from_rfc3339(&iso) {
            let mins = t.signed_duration_since(Utc::now()).num_minutes();
            if mins <= 0 {
                "runs out soon".to_string()
            } else if mins < 60 {
                format!("runs out in {mins}m")
            } else {
                let h = mins / 60;
                let m = mins % 60;
                format!("runs out in {h}h {m}m")
            }
        } else {
            String::new()
        }
    });

    let mut annotations = Vec::new();
    if let Some(s) = reset_str {
        annotations.push(s);
    }
    if let Some(s) = runs_out_str {
        if !s.is_empty() {
            annotations.push(s);
        }
    }
    let annotation = if annotations.is_empty() {
        String::new()
    } else {
        format!("  {}", annotations.join("  ").bright_black())
    };

    println!(
        "{label}: {color_bar} {:.0}%  {}{annotation}",
        pct * 100.0,
        usage_str.bright_black()
    );
}

/// Compute the ISO 8601 timestamp when usage will be exhausted at the current burn rate.
/// Returns None if there's no usage yet, no period info, or the period outlasts the reset.
fn compute_runs_out_at(period: &snapshot::UsagePeriod) -> Option<String> {
    let resets_at = period
        .resets_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())?;
    let period_ms = period.period_duration_ms? as f64;

    let used_fraction = 1.0 - period.remaining_fraction;
    if used_fraction <= 0.0 {
        return None; // no usage yet, can't project
    }

    let time_until_reset_ms = resets_at
        .signed_duration_since(Utc::now())
        .num_milliseconds() as f64;
    let time_elapsed_ms = period_ms - time_until_reset_ms;
    if time_elapsed_ms <= 0.0 {
        return None; // period just started
    }

    // At current rate, how long until remaining fraction is exhausted?
    let runs_out_ms = (period.remaining_fraction / used_fraction) * time_elapsed_ms;
    let runs_out_at = Utc::now() + chrono::Duration::milliseconds(runs_out_ms as i64);

    // Only meaningful if exhaustion happens before the period resets
    if runs_out_at >= resets_at {
        return None;
    }

    Some(runs_out_at.to_rfc3339())
}

fn render_bar(filled_fraction: f64, width: usize) -> String {
    let filled = (filled_fraction.clamp(0.0, 1.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

fn print_markdown(outputs: &[(plugin_runtime::LoadedPlugin, snapshot::ProviderSnapshot)]) {
    println!("| Provider | Status | Session remaining | Weekly remaining |");
    println!("|----------|--------|-------------------|-----------------|");
    for (_, snap) in outputs {
        let status = format!("{:?}", snap.status).to_lowercase();
        let session_pct = snap
            .session
            .as_ref()
            .map(|p| format!("{:.0}%", p.remaining_fraction * 100.0))
            .unwrap_or_else(|| "—".to_string());
        let weekly_pct = snap
            .weekly
            .as_ref()
            .map(|p| format!("{:.0}%", p.remaining_fraction * 100.0))
            .unwrap_or_else(|| "—".to_string());
        println!(
            "| {} | {} | {} | {} |",
            snap.display_name, status, session_pct, weekly_pct
        );
    }
}
