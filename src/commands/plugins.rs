use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;

use crate::config;
use crate::plugin_runtime;

#[derive(Subcommand, Debug)]
pub enum PluginsCommand {
    /// List all installed plugins
    List,
    /// Install a plugin from a local directory
    Add {
        /// Path to the plugin directory (must contain plugin.json)
        path: String,
    },
    /// Remove an installed plugin by ID
    Remove {
        /// Plugin ID to remove
        id: String,
    },
}

pub fn run(cmd: PluginsCommand) -> Result<()> {
    config::ensure_bundled_plugins_installed()?;

    match cmd {
        PluginsCommand::List => list(),
        PluginsCommand::Add { path } => add(&path),
        PluginsCommand::Remove { id } => remove(&id),
    }
}

fn list() -> Result<()> {
    use owo_colors::OwoColorize;

    let plugins_dir = config::effective_plugins_dir()?;
    let plugins = plugin_runtime::load_plugins(&plugins_dir);

    if plugins.is_empty() {
        println!("No plugins installed.");
        return Ok(());
    }

    println!(
        "{:<20} {:<30} {:<10} {}",
        "ID".bold(),
        "Name".bold(),
        "Version".bold(),
        "Schema".bold()
    );
    println!("{}", "─".repeat(72));

    for plugin in &plugins {
        println!(
            "{:<20} {:<30} {:<10} v{}",
            plugin.manifest.id,
            plugin.manifest.name,
            plugin.manifest.version,
            plugin.manifest.schema_version
        );
    }

    println!();
    println!("  {} plugin(s) in {}", plugins.len(), plugins_dir.display());

    Ok(())
}

fn add(path: &str) -> Result<()> {
    let src = PathBuf::from(path)
        .canonicalize()
        .with_context(|| format!("path not found: {path}"))?;

    // Validate it looks like a plugin directory
    let manifest_path = src.join("plugin.json");
    if !manifest_path.exists() {
        anyhow::bail!("no plugin.json found in {}", src.display());
    }

    let plugin = plugin_runtime::manifest::load_plugin(&src)
        .with_context(|| format!("invalid plugin at {}", src.display()))?;

    let plugins_dir = config::plugins_dir()?;
    let dst = plugins_dir.join(&plugin.manifest.id);

    if dst.exists() {
        anyhow::bail!(
            "plugin '{}' is already installed. Remove it first with `au plugins remove {}`",
            plugin.manifest.id,
            plugin.manifest.id
        );
    }

    copy_dir_recursive(&src, &dst)?;
    println!(
        "Installed plugin '{}' ({})",
        plugin.manifest.name, plugin.manifest.id
    );

    Ok(())
}

fn remove(id: &str) -> Result<()> {
    let plugins_dir = config::plugins_dir()?;
    let target = plugins_dir.join(id);

    if !target.exists() {
        anyhow::bail!("plugin '{}' is not installed", id);
    }

    // Sanity check: make sure it has a plugin.json before deleting
    if !target.join("plugin.json").exists() {
        anyhow::bail!(
            "'{}' does not appear to be a plugin directory",
            target.display()
        );
    }

    fs::remove_dir_all(&target).with_context(|| format!("could not remove plugin '{id}'"))?;

    println!("Removed plugin '{id}'.");
    Ok(())
}

fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)?.flatten() {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
