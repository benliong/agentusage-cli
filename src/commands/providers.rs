use anyhow::Result;

use crate::config;
use crate::credential;
use crate::plugin_runtime;

pub fn run() -> Result<()> {
    config::ensure_bundled_plugins_installed()?;

    let plugins_dir = config::plugins_dir()?;
    let plugins = plugin_runtime::load_plugins(&plugins_dir);

    if plugins.is_empty() {
        println!("No plugins found. Run `au plugins list` to see available plugins.");
        return Ok(());
    }

    use owo_colors::OwoColorize;

    println!("{:<20} {:<30} {:<12}", "ID".bold(), "Name".bold(), "Configured".bold());
    println!("{}", "─".repeat(64));

    for plugin in &plugins {
        let configured = if credential::exists(&plugin.manifest.id) {
            "✓ yes".green().to_string()
        } else {
            "✗ no".bright_black().to_string()
        };
        println!(
            "{:<20} {:<30} {}",
            plugin.manifest.id,
            plugin.manifest.name,
            configured
        );
    }

    println!();
    println!(
        "  {} provider(s). Run `au configure` to set up credentials.",
        plugins.len()
    );

    Ok(())
}
