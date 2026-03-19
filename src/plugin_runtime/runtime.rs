use std::path::PathBuf;

use anyhow::anyhow;
use rquickjs::promise::MaybePromise;
use rquickjs::{Context, Runtime, Value};
use serde::{Deserialize, Serialize};

use super::host_api;
use super::manifest::LoadedPlugin;

/// Mirrors the MetricLine union type from the robinebers/openusage plugin contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MetricLine {
    Progress {
        label: String,
        used: f64,
        limit: f64,
        #[serde(default)]
        format: Option<ProgressFormat>,
        #[serde(rename = "resetsAt", default)]
        resets_at: Option<String>,
        #[serde(rename = "periodDurationMs", default)]
        period_duration_ms: Option<u64>,
        #[serde(default)]
        color: Option<String>,
    },
    Text {
        label: String,
        value: String,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        subtitle: Option<String>,
    },
    Badge {
        label: String,
        text: String,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        subtitle: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProgressFormat {
    Percent,
    Dollars,
    Count {
        #[serde(default)]
        suffix: String,
    },
}

/// The full output of a plugin's probe() function.
#[derive(Debug, Clone)]
pub struct PluginOutput {
    pub provider_id: String,
    pub display_name: String,
    pub lines: Vec<MetricLine>,
    pub error: Option<String>,
    pub plan: Option<String>,
}

/// Run a single plugin's probe() function and return its output.
///
/// Uses a fresh QuickJS Runtime + Context per invocation (matching the reference
/// implementation's isolation model).
pub fn run_probe(plugin: &LoadedPlugin, app_data_dir: &PathBuf, app_version: &str) -> PluginOutput {
    match run_probe_inner(plugin, app_data_dir, app_version) {
        Ok((lines, plan)) => PluginOutput {
            provider_id: plugin.manifest.id.clone(),
            display_name: plugin.manifest.name.clone(),
            lines,
            error: None,
            plan,
        },
        Err(e) => {
            tracing::warn!("plugin '{}' failed: {e}", plugin.manifest.id);
            PluginOutput {
                provider_id: plugin.manifest.id.clone(),
                display_name: plugin.manifest.name.clone(),
                lines: vec![],
                error: Some(e.to_string()),
                plan: None,
            }
        }
    }
}

fn run_probe_inner(
    plugin: &LoadedPlugin,
    _app_data_dir: &PathBuf,
    app_version: &str,
) -> anyhow::Result<(Vec<MetricLine>, Option<String>)> {
    let rt = Runtime::new().map_err(|e| anyhow!("failed to create QuickJS runtime: {e}"))?;
    let ctx = Context::full(&rt).map_err(|e| anyhow!("failed to create QuickJS context: {e}"))?;

    ctx.with(|ctx| -> anyhow::Result<(Vec<MetricLine>, Option<String>)> {
        // 1. Inject host API
        host_api::inject(&ctx, &plugin.dir, app_version)
            .map_err(|e| anyhow!("host API injection failed: {e}"))?;

        // 2. Run patch scripts
        ctx.eval::<(), _>(host_api::HTTP_PATCH_SCRIPT)
            .map_err(|e| anyhow!("http patch failed: {e}"))?;
        ctx.eval::<(), _>(host_api::SQLITE_PATCH_SCRIPT)
            .map_err(|e| anyhow!("sqlite patch failed: {e}"))?;
        ctx.eval::<(), _>(host_api::UTILS_SCRIPT)
            .map_err(|e| anyhow!("utils inject failed: {e}"))?;

        // 3. Evaluate the plugin script
        ctx.eval::<(), _>(plugin.script.as_str())
            .map_err(|e| anyhow!("plugin script eval failed: {e}"))?;

        // 4. Look up __openusage_plugin.probe
        let globals = ctx.globals();
        let plugin_obj: rquickjs::Object = globals
            .get("__openusage_plugin")
            .map_err(|_| anyhow!("plugin did not set globalThis.__openusage_plugin"))?;

        let probe_fn: rquickjs::Function = plugin_obj
            .get("probe")
            .map_err(|_| anyhow!("plugin missing probe() function"))?;

        let ctx_arg: Value = globals
            .get("__openusage_ctx")
            .map_err(|_| anyhow!("__openusage_ctx not set"))?;

        // 5. Call probe(ctx) — result may be a plain object or a Promise
        let raw_result: Value = probe_fn.call((ctx_arg,)).map_err(|_| {
            // Extract the actual JS exception message instead of the opaque QuickJS error
            let exc = ctx.catch();
            let msg: String = if exc.is_object() {
                exc.as_object()
                    .and_then(|o| o.get::<_, String>("message").ok())
                    .unwrap_or_else(|| "unknown JS error".to_string())
            } else {
                exc.as_string()
                    .and_then(|s| s.to_string().ok())
                    .unwrap_or_else(|| "unknown JS error".to_string())
            };
            anyhow!("probe() call failed: {msg}")
        })?;

        // 6. Resolve through MaybePromise::finish — handles both sync and async plugins
        let maybe: MaybePromise = MaybePromise::from_value(raw_result);
        let resolved: Value = maybe
            .finish()
            .map_err(|e| anyhow!("probe() promise resolution failed: {e}"))?;

        // 7. JSON-stringify and parse into MetricLine[]
        parse_probe_result(&ctx, resolved)
    })
}

/// Extract (MetricLine[], plan) from the probe() result value.
/// Handles both `{ lines: [...], plan: "..." }` and a bare array `[...]`.
fn parse_probe_result<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: Value<'js>,
) -> anyhow::Result<(Vec<MetricLine>, Option<String>)> {
    let json_str = ctx
        .json_stringify(value)
        .map_err(|e| anyhow!("could not JSON-stringify probe result: {e}"))?
        .map(|s| s.to_string().unwrap_or_default())
        .unwrap_or_default();

    if json_str.is_empty() || json_str == "null" {
        return Ok((vec![], None));
    }

    let obj: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| anyhow!("could not parse probe result JSON: {e}"))?;

    // Try { lines: MetricLine[], plan?: string }
    if let Some(lines_val) = obj.get("lines") {
        if let Ok(lines) = serde_json::from_value::<Vec<MetricLine>>(lines_val.clone()) {
            let plan = obj
                .get("plan")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            return Ok((lines, plan));
        }
    }

    // Try bare array
    if obj.is_array() {
        if let Ok(lines) = serde_json::from_value::<Vec<MetricLine>>(obj) {
            return Ok((lines, None));
        }
    }

    Ok((vec![], None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_runtime::manifest::load_plugin;

    #[test]
    fn mock_plugin_runs() {
        let plugin_dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bundled_plugins/mock");
        if !plugin_dir.exists() {
            eprintln!("skipping mock_plugin_runs: bundled_plugins/mock not found");
            return;
        }
        let plugin = load_plugin(&plugin_dir).expect("load mock plugin");
        let output = run_probe(&plugin, &std::env::temp_dir(), "0.1.0");
        assert!(
            output.error.is_none(),
            "mock plugin error: {:?}",
            output.error
        );
        assert!(!output.lines.is_empty(), "mock plugin returned no lines");
    }
}
