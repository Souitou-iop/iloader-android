use std::sync::{Arc, atomic::Ordering};

use base64::prelude::*;
use isideload::util::{fs_storage::FsStorage, storage::SideloadingStorage};
use rootcause::prelude::*;
use tracing::warn;

use crate::{Core, error::AppError};

/// Service name of every entry iloader keeps in the OS keyring.
const KEYRING_SERVICE: &str = "iloader";

/// Where saved Apple ID passwords and the sideloading state (anisette state,
/// certificates, pairing files) are kept. Keys are flat names such as an
/// email address or `anisette_state`.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn set(&self, key: &str, value: &str) -> Result<(), String>;
    /// Returns whether there was something to delete.
    fn delete(&self, key: &str) -> Result<bool, String>;

    /// Binary values. Stores are text based, so the default is base64.
    fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        match self.get(key)? {
            Some(value) => BASE64_STANDARD
                .decode(value)
                .map(Some)
                .map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    fn set_bytes(&self, key: &str, value: &[u8]) -> Result<(), String> {
        self.set(key, &BASE64_STANDARD.encode(value))
    }
}

/// The OS keyring (Keychain, Credential Manager, Secret Service).
pub struct KeyringSecretStore;

impl SecretStore for KeyringSecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, key).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(KEYRING_SERVICE, key)
            .and_then(|entry| entry.set_password(value))
            .map_err(|e| e.to_string())
    }

    fn delete(&self, key: &str) -> Result<bool, String> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, key).map_err(|e| e.to_string())?;
        match entry.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }

    // Windows rejects the base64 form because it is too long, and Linux does
    // not return binary secrets properly; this mirrors isideload's own
    // `KeyringStorage` so existing entries stay readable.
    #[cfg(target_os = "windows")]
    fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, key).map_err(|e| e.to_string())?;
        match entry.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    #[cfg(target_os = "windows")]
    fn set_bytes(&self, key: &str, value: &[u8]) -> Result<(), String> {
        keyring::Entry::new(KEYRING_SERVICE, key)
            .and_then(|entry| entry.set_secret(value))
            .map_err(|e| e.to_string())
    }
}

/// Lets isideload keep its state in a `SecretStore`.
struct SecretSideloadingStorage(Arc<dyn SecretStore>);

impl SideloadingStorage for SecretSideloadingStorage {
    fn store(&self, key: &str, value: &str) -> Result<(), Report> {
        self.0.set(key, value).map_err(|e| report(e))
    }

    fn retrieve(&self, key: &str) -> Result<Option<String>, Report> {
        self.0.get(key).map_err(|e| report(e))
    }

    fn store_data(&self, key: &str, value: &[u8]) -> Result<(), Report> {
        self.0.set_bytes(key, value).map_err(|e| report(e))
    }

    fn retrieve_data(&self, key: &str) -> Result<Option<Vec<u8>>, Report> {
        self.0.get_bytes(key).map_err(|e| report(e))
    }

    fn delete(&self, key: &str) -> Result<(), Report> {
        self.0.delete(key).map(|_| ()).map_err(|e| report(e))
    }
}

impl Core {
    pub fn force_disable_keyring(&self, force: bool) {
        self.force_disable_keyring.store(force, Ordering::Relaxed);

        if force {
            warn!("Keyring has been forcefully disabled by the user.");
        } else if !self.check_keyring_available() {
            warn!("Keyring is not available and cannot be enabled.");
        }
    }

    pub fn keyring_available(&self) -> bool {
        !self.force_disable_keyring.load(Ordering::Relaxed) && self.check_keyring_available()
    }

    fn check_keyring_available(&self) -> bool {
        match self.host.secrets() {
            Some(secrets) => {
                secrets.set("test", "test").is_ok() && matches!(secrets.get("test"), Ok(Some(_)))
            }
            None => false,
        }
    }

    pub(crate) fn create_sideloading_storage(
        &self,
    ) -> Result<Box<dyn SideloadingStorage>, AppError> {
        if self.keyring_available()
            && let Some(secrets) = self.host.secrets()
        {
            Ok(Box::new(SecretSideloadingStorage(secrets)))
        } else {
            warn!(
                "Keyring is not available, falling back to filesystem storage for sideloading data. This is insecure!"
            );
            Ok(Box::new(FsStorage::new(self.host.data_dir())))
        }
    }
}

fn report(message: String) -> Report {
    report!(message).into_dynamic()
}
