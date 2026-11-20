use super::Provider;
use anyhow::{Result, ensure};
use voai_l10n::{tf, tr};
use zeroize::Zeroizing;

#[cfg(target_os = "macos")]
mod secrets {
    use super::*;
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };
    const SERVICE: &str = "Voai.CloudPostprocessing";
    const NOT_FOUND: i32 = -25300;

    pub fn exists(account: &str) -> Result<bool> {
        use security_framework::item::{ItemClass, ItemSearchOptions};
        match ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(SERVICE)
            .account(account)
            .load_attributes(true)
            .search()
        {
            Ok(items) => Ok(!items.is_empty()),
            Err(err) if err.code() == NOT_FOUND => Ok(false),
            Err(err) => Err(anyhow::anyhow!(tf!("keychain.read", code = err.code()))),
        }
    }

    pub fn read(account: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        match get_generic_password(SERVICE, account) {
            Ok(key) => Ok(Some(Zeroizing::new(key))),
            Err(err) if err.code() == NOT_FOUND => Ok(None),
            Err(err) => Err(anyhow::anyhow!(tf!("keychain.read", code = err.code()))),
        }
    }

    pub fn save(account: &str, key: &str) -> Result<()> {
        let result = if key.is_empty() {
            delete_generic_password(SERVICE, account)
        } else {
            set_generic_password(SERVICE, account, key.as_bytes())
        };
        match result {
            Ok(()) => Ok(()),
            Err(err) if key.is_empty() && err.code() == NOT_FOUND => Ok(()),
            Err(err) => Err(anyhow::anyhow!(tf!("keychain.write", code = err.code()))),
        }
    }
}
#[cfg(not(target_os = "macos"))]
mod secrets {
    use super::*;
    pub fn exists(_: &str) -> Result<bool> {
        anyhow::bail!(tr("keychain.macos"))
    }
    pub fn read(_: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        anyhow::bail!(tr("keychain.macos"))
    }
    pub fn save(_: &str, _: &str) -> Result<()> {
        anyhow::bail!(tr("keychain.macos"))
    }
}

pub fn has_api_key(provider: Provider) -> Result<bool> {
    secrets::exists(provider.name())
}

/// An empty value removes the stored credential.
pub fn set_api_key(provider: Provider, key: &str) -> Result<()> {
    set_key(provider.name(), key)
}

pub(super) fn set_key(account: &str, key: &str) -> Result<()> {
    let key = key.trim();
    ensure!(
        key.len() <= 4096 && key.bytes().all(|c| c.is_ascii_graphic()),
        tr("cloud.invalid.key")
    );
    secrets::save(account, key)
}

pub(super) fn read(provider: Provider) -> Result<Option<Zeroizing<Vec<u8>>>> {
    secrets::read(provider.name())
}

pub(super) use secrets::{exists as has_account, read as read_account};
