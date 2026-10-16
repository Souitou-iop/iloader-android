use std::{path::PathBuf, sync::Arc};

use iloader_core::{CoreEvent, Host, KeyringSecretStore, KvStore, SecretStore};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_store::StoreExt;
use tracing::debug;

/// Connects the core to the Tauri app: files in the app data directory, the
/// OS keyring, the shared `data.json` store and events to the webview.
pub struct TauriHost {
    app: AppHandle,
}

impl TauriHost {
    pub fn new(app: AppHandle) -> Self {
        TauriHost { app }
    }
}

impl Host for TauriHost {
    fn data_dir(&self) -> PathBuf {
        self.app
            .path()
            .app_data_dir()
            .expect("failed to get app data dir")
    }

    fn cache_dir(&self) -> PathBuf {
        self.app
            .path()
            .temp_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
    }

    fn secrets(&self) -> Option<Arc<dyn SecretStore>> {
        Some(Arc::new(KeyringSecretStore))
    }

    // The webview reads and writes the same file through the store plugin.
    fn kv(&self) -> Option<Arc<dyn KvStore>> {
        Some(Arc::new(PluginKv {
            app: self.app.clone(),
        }))
    }

    fn emit(&self, event: CoreEvent) {
        let result = match event {
            CoreEvent::TwoFactorRequired(params) => self.app.emit("2fa-required", params),
            CoreEvent::MaxCertsReached(certs) => self.app.emit("max-certs-reached", certs),
            CoreEvent::Operation {
                operation_id,
                update,
            } => self.app.emit(&format!("operation_{operation_id}"), update),
            CoreEvent::Log(record) => self.app.emit("log-record", record),
            CoreEvent::UsbPairing(event) => self.app.emit("usbmux-pairing", event),
        };
        if let Err(err) = result {
            debug!("failed to emit event: {err}");
        }
    }
}

struct PluginKv {
    app: AppHandle,
}

impl KvStore for PluginKv {
    fn get(&self, key: &str) -> Option<serde_json::Value> {
        self.app.store("data.json").ok()?.get(key)
    }

    fn set(&self, key: &str, value: serde_json::Value) -> Result<(), String> {
        let store = self
            .app
            .store("data.json")
            .map_err(|e| format!("Failed to get store: {e:?}"))?;
        store.set(key, value);
        Ok(())
    }
}
