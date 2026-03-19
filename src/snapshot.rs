use serde::{Deserialize, Serialize};

use crate::plugin_runtime::manifest::PluginManifest;
use crate::plugin_runtime::runtime::{MetricLine, PluginOutput, ProgressFormat};

/// A structured provider snapshot derived from a plugin's MetricLine[].
/// This is the intermediate representation between raw plugin output and the agent skill JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSnapshot {
    pub id: String,
    pub display_name: String,
    pub status: ProviderStatus,
    pub plan: Option<String>,
    pub session: Option<UsagePeriod>,
    pub weekly: Option<UsagePeriod>,
    pub recommendation: ProviderRec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Ok,
    Error,
    NotConfigured,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRec {
    Sufficient,
    Low,
    Exhausted,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsagePeriod {
    pub tokens_used: Option<f64>,
    pub tokens_limit: Option<f64>,
    pub remaining_fraction: f64,
    pub cost_usd: Option<f64>,
    pub cost_limit_usd: Option<f64>,
    pub requests_count: Option<u64>,
    pub resets_at: Option<String>,
    pub period_duration_ms: Option<u64>,
}

impl UsagePeriod {
    fn from_progress_line(used: f64, limit: f64, format: &Option<ProgressFormat>, resets_at: Option<String>, period_duration_ms: Option<u64>) -> Self {
        let remaining_fraction = if limit > 0.0 {
            ((limit - used) / limit).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let (tokens_used, tokens_limit, cost_usd, cost_limit_usd) =
            match format.as_ref().unwrap_or(&ProgressFormat::Count { suffix: String::new() }) {
                ProgressFormat::Dollars => (
                    None, None,
                    Some(used), Some(limit),
                ),
                ProgressFormat::Percent => (
                    None, None, None, None,
                ),
                ProgressFormat::Count { .. } => (
                    Some(used), Some(limit), None, None,
                ),
            };

        UsagePeriod {
            tokens_used,
            tokens_limit,
            remaining_fraction,
            cost_usd,
            cost_limit_usd,
            requests_count: None,
            resets_at,
            period_duration_ms,
        }
    }
}

/// Translate a PluginOutput into a ProviderSnapshot.
///
/// The session period comes from the ManifestLine with primaryOrder == 1.
/// The weekly period comes from primaryOrder == 2.
/// When the manifest doesn't declare primaryOrder, we fall back to positional order
/// (first progress line = session, second = weekly).
pub fn translate(output: &PluginOutput, manifest: &PluginManifest) -> ProviderSnapshot {
    // Detect not-configured: plugin returned no lines and no explicit error,
    // OR a Badge line with typical "not configured" text
    if output.lines.is_empty() {
        if output.error.is_some() {
            return ProviderSnapshot {
                id: output.provider_id.clone(),
                display_name: output.display_name.clone(),
                status: ProviderStatus::Error,
                plan: output.plan.clone(),
                session: None,
                weekly: None,
                recommendation: ProviderRec::Unknown,
            };
        }
        return ProviderSnapshot {
            id: output.provider_id.clone(),
            display_name: output.display_name.clone(),
            status: ProviderStatus::NotConfigured,
            plan: output.plan.clone(),
            session: None,
            weekly: None,
            recommendation: ProviderRec::Unknown,
        };
    }

    // Detect not-configured via error badge ONLY when there are no progress lines.
    // (A red badge alongside real progress data is valid — e.g. "Rate limited" warning.)
    let has_progress = output.lines.iter().any(|l| matches!(l, MetricLine::Progress { .. }));
    if !has_progress {
        let has_error_badge = output.lines.iter().any(|l| {
            matches!(l, MetricLine::Badge { color: Some(c), .. } if c == "#ef4444" || c == "red")
        });
        if has_error_badge {
            return ProviderSnapshot {
                id: output.provider_id.clone(),
                display_name: output.display_name.clone(),
                status: ProviderStatus::NotConfigured,
                plan: output.plan.clone(),
                session: None,
                weekly: None,
                recommendation: ProviderRec::Unknown,
            };
        }
    }

    // Map manifest lines by label to find primaryOrder
    let primary_order_map: std::collections::HashMap<&str, u32> = manifest
        .lines
        .iter()
        .filter_map(|ml| ml.primary_order.map(|ord| (ml.label.as_str(), ord)))
        .collect();

    // Collect progress lines with their effective primary_order
    struct Candidate {
        used: f64,
        limit: f64,
        format: Option<ProgressFormat>,
        resets_at: Option<String>,
        period_duration_ms: Option<u64>,
        primary_order: u32,
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut positional = 0u32;

    for line in &output.lines {
        if let MetricLine::Progress { label, used, limit, format, resets_at, period_duration_ms, .. } = line {
            positional += 1;
            let order = primary_order_map
                .get(label.as_str())
                .copied()
                .unwrap_or(positional);
            candidates.push(Candidate {
                used: *used,
                limit: *limit,
                format: format.clone(),
                resets_at: resets_at.clone(),
                period_duration_ms: *period_duration_ms,
                primary_order: order,
            });
        }
    }

    let find_period = |order: u32| -> Option<UsagePeriod> {
        candidates.iter()
            .find(|c| c.primary_order == order)
            .map(|c| UsagePeriod::from_progress_line(c.used, c.limit, &c.format, c.resets_at.clone(), c.period_duration_ms))
    };

    let session = find_period(1);
    let weekly = find_period(2);

    // Compute recommendation from the most meaningful period
    let recommendation = session.as_ref()
        .or(weekly.as_ref())
        .map(|p| {
            if p.remaining_fraction > 0.3 {
                ProviderRec::Sufficient
            } else if p.remaining_fraction > 0.05 {
                ProviderRec::Low
            } else {
                ProviderRec::Exhausted
            }
        })
        .unwrap_or(ProviderRec::Unknown);

    ProviderSnapshot {
        id: output.provider_id.clone(),
        display_name: output.display_name.clone(),
        status: ProviderStatus::Ok,
        plan: output.plan.clone(),
        session,
        weekly,
        recommendation,
    }
}
