use serde::Serialize;

use crate::snapshot::{ProviderRec, ProviderSnapshot, ProviderStatus};

#[derive(Debug, Clone, Serialize)]
pub struct RecommendationBlock {
    pub best_provider: String,
    pub best_provider_period: String,
    pub reason: String,
}

/// Compute the best provider recommendation from a slice of snapshots.
///
/// Algorithm:
/// 1. Consider only snapshots with status Ok.
/// 2. Score each (provider, period) pair by remaining_fraction.
/// 3. Prefer session over weekly as a tiebreaker.
/// 4. Return the winner, or None if no providers are available.
pub fn compute(snapshots: &[ProviderSnapshot]) -> Option<RecommendationBlock> {
    struct Candidate<'a> {
        snapshot: &'a ProviderSnapshot,
        period: &'static str,
        remaining_fraction: f64,
    }

    let mut candidates: Vec<Candidate> = Vec::new();

    for snap in snapshots {
        if snap.status != ProviderStatus::Ok {
            continue;
        }
        if snap.recommendation == ProviderRec::Exhausted {
            continue;
        }
        if let Some(session) = &snap.session {
            if session.remaining_fraction > 0.05 {
                candidates.push(Candidate {
                    snapshot: snap,
                    period: "session",
                    remaining_fraction: session.remaining_fraction,
                });
            }
        }
        if let Some(weekly) = &snap.weekly {
            if weekly.remaining_fraction > 0.05 {
                candidates.push(Candidate {
                    snapshot: snap,
                    period: "weekly",
                    remaining_fraction: weekly.remaining_fraction,
                });
            }
        }
    }

    if candidates.is_empty() {
        return None;
    }

    // Sort: descending remaining_fraction, prefer "session" over "weekly" on tie
    candidates.sort_by(|a, b| {
        b.remaining_fraction
            .partial_cmp(&a.remaining_fraction)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                if a.period == "session" && b.period != "session" {
                    std::cmp::Ordering::Less // a wins
                } else if b.period == "session" && a.period != "session" {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
    });

    let winner = candidates.first()?;
    let pct = (winner.remaining_fraction * 100.0).round() as u32;
    let reason = format!(
        "highest remaining_fraction ({:.0}%) with sufficient headroom",
        winner.remaining_fraction * 100.0
    );

    Some(RecommendationBlock {
        best_provider: winner.snapshot.id.clone(),
        best_provider_period: winner.period.to_string(),
        reason,
    })
}
