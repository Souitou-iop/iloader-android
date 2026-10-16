use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};

use serde_json::Value;

/// Small persistent key-value store for non-secret settings, such as the
/// list of saved Apple IDs (`ids`). The desktop app backs this with
/// `tauri-plugin-store` because the webview reads the same `data.json`.
pub trait KvStore: Send + Sync {
    fn get(&self, key: &str) -> Option<Value>;
    fn set(&self, key: &str, value: Value) -> Result<(), String>;
}

/// A JSON object in a single file. Uses the same format as
/// `tauri-plugin-store`, so an existing `data.json` stays readable.
pub struct JsonFileKvStore {
    path: PathBuf,
    values: Mutex<BTreeMap<String, Value>>,
}

impl JsonFileKvStore {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let values = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        JsonFileKvStore {
            path,
            values: Mutex::new(values),
        }
    }
}

impl KvStore for JsonFileKvStore {
    fn get(&self, key: &str) -> Option<Value> {
        self.values.lock().unwrap().get(key).cloned()
    }

    fn set(&self, key: &str, value: Value) -> Result<(), String> {
        let mut values = self.values.lock().unwrap();
        values.insert(key.to_string(), value);
        let bytes = serde_json::to_vec_pretty(&*values).map_err(|e| e.to_string())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tauri_store_files_and_persists() {
        let dir = std::env::temp_dir().join(format!("iloader-kv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.json");
        std::fs::write(&path, br#"{"ids":["a@b.c"],"other":1}"#).unwrap();

        let store = JsonFileKvStore::open(&path);
        assert_eq!(store.get("ids"), Some(serde_json::json!(["a@b.c"])));
        store
            .set("ids", serde_json::json!(["a@b.c", "d@e.f"]))
            .unwrap();

        let reopened = JsonFileKvStore::open(&path);
        assert_eq!(
            reopened.get("ids"),
            Some(serde_json::json!(["a@b.c", "d@e.f"]))
        );
        assert_eq!(reopened.get("other"), Some(serde_json::json!(1)));
        assert_eq!(reopened.get("missing"), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
