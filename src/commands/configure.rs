use anyhow::Result;
use clap::Args;

use crate::config;
use crate::credential;
use crate::plugin_runtime;

#[derive(Args, Debug)]
pub struct ConfigureArgs {
    /// Provider ID to configure (e.g. anthropic, openai)
    #[arg(long)]
    pub provider: Option<String>,

    /// API key / credential value (non-interactive)
    #[arg(long)]
    pub key: Option<String>,

    /// Delete the stored credential for this provider
    #[arg(long)]
    pub delete: bool,
}

pub fn run(args: ConfigureArgs) -> Result<()> {
    config::ensure_bundled_plugins_installed()?;

    match (args.provider, args.key, args.delete) {
        // Non-interactive: au configure --provider anthropic --key sk-...
        (Some(provider_id), Some(key), false) => {
            credential::store(&provider_id, &key)?;
            eprintln!("Stored credential for '{provider_id}'.");
        }

        // Delete: au configure --provider anthropic --delete
        (Some(provider_id), None, true) => {
            credential::delete(&provider_id)?;
            eprintln!("Deleted credential for '{provider_id}'.");
        }

        // Interactive wizard: au configure
        (None, None, false) => {
            run_interactive()?;
        }

        // au configure --provider X (no key, no delete) — prompt for just that provider
        (Some(provider_id), None, false) => {
            prompt_provider(&provider_id)?;
        }

        _ => {
            anyhow::bail!("invalid arguments: use --provider and --key together, or --provider and --delete");
        }
    }

    Ok(())
}

fn run_interactive() -> Result<()> {
    use dialoguer::{Select, theme::ColorfulTheme};

    let plugins_dir = config::plugins_dir()?;
    let plugins = plugin_runtime::load_plugins(&plugins_dir);

    if plugins.is_empty() {
        eprintln!("No plugins found. Cannot configure any providers.");
        return Ok(());
    }

    let items: Vec<String> = plugins
        .iter()
        .map(|p| {
            let configured = if credential::exists(&p.manifest.id) {
                " ✓"
            } else {
                ""
            };
            format!("{}{}", p.manifest.name, configured)
        })
        .collect();

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select provider to configure")
        .items(&items)
        .interact_opt()?;

    if let Some(idx) = selection {
        prompt_provider(&plugins[idx].manifest.id)?;
    }

    Ok(())
}

fn prompt_provider(provider_id: &str) -> Result<()> {
    use dialoguer::{Password, theme::ColorfulTheme};

    let existing = credential::exists(provider_id);
    let prompt = if existing {
        format!("New credential for '{provider_id}' (leave blank to keep existing)")
    } else {
        format!("Credential for '{provider_id}'")
    };

    let key = Password::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .allow_empty_password(existing)
        .interact()?;

    if key.is_empty() {
        eprintln!("Keeping existing credential for '{provider_id}'.");
    } else {
        credential::store(provider_id, &key)?;
        eprintln!("Stored credential for '{provider_id}'.");
    }

    Ok(())
}
