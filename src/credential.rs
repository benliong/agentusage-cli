use anyhow::{Context, Result};

const KEYRING_SERVICE: &str = "agentusage";

/// Store a provider credential in the OS keyring.
/// service = KEYRING_SERVICE, account = provider_id.
/// This matches the lookup done by plugin host_api ctx.host.keychain.readGenericPassword(provider_id).
pub fn store(provider_id: &str, key: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, provider_id)
        .with_context(|| format!("could not create keyring entry for provider '{provider_id}'"))?;
    entry
        .set_password(key)
        .with_context(|| format!("could not store credential for provider '{provider_id}'"))?;
    Ok(())
}

/// Read a provider credential from the OS keyring.
pub fn read(provider_id: &str) -> Result<Option<String>> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, provider_id)
        .with_context(|| format!("could not create keyring entry for provider '{provider_id}'"))?;
    match entry.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).with_context(|| format!("could not read credential for provider '{provider_id}'")),
    }
}

/// Delete a provider credential from the OS keyring.
pub fn delete(provider_id: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, provider_id)
        .with_context(|| format!("could not create keyring entry for provider '{provider_id}'"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // already gone
        Err(e) => Err(e).with_context(|| format!("could not delete credential for provider '{provider_id}'")),
    }
}

/// Returns true if a credential exists for the given provider.
pub fn exists(provider_id: &str) -> bool {
    read(provider_id).ok().flatten().is_some()
}
