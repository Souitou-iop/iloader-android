use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where the muxer keeps pairing records and its host id (BUID), like
/// `/var/lib/lockdown` on a computer.
pub trait PairStore: Send + Sync + 'static {
    fn read(&self, udid: &str) -> Option<Vec<u8>>;
    fn save(&self, udid: &str, record: &[u8]) -> io::Result<()>;
    fn delete(&self, udid: &str) -> io::Result<()>;
    /// The host's BUID, created on first use and then kept.
    fn buid(&self) -> String;
}

/// Stores one `<udid>.plist` per device in a directory.
pub struct FilePairStore {
    dir: PathBuf,
}

impl FilePairStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        FilePairStore { dir: dir.into() }
    }

    /// UDIDs come from clients of the local socket, so they must not be able
    /// to name anything but a plain file in the directory.
    fn path(&self, udid: &str) -> io::Result<PathBuf> {
        let valid = !udid.is_empty()
            && udid.len() <= 64
            && udid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !valid {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid pair record id",
            ));
        }
        Ok(self.dir.join(format!("{udid}.plist")))
    }

    fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, data)?;
        fs::rename(&tmp, path)
    }
}

impl PairStore for FilePairStore {
    fn read(&self, udid: &str) -> Option<Vec<u8>> {
        fs::read(self.path(udid).ok()?).ok()
    }

    fn save(&self, udid: &str, record: &[u8]) -> io::Result<()> {
        let path = self.path(udid)?;
        fs::create_dir_all(&self.dir)?;
        Self::write_private(&path, record)
    }

    fn delete(&self, udid: &str) -> io::Result<()> {
        match fs::remove_file(self.path(udid)?) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    fn buid(&self) -> String {
        let path = self.dir.join("SystemBUID");
        if let Ok(existing) = fs::read_to_string(&path) {
            let existing = existing.trim();
            if !existing.is_empty() {
                return existing.to_string();
            }
        }
        let buid = uuid::Uuid::new_v4().to_string().to_uppercase();
        if fs::create_dir_all(&self.dir).is_ok() {
            let _ = Self::write_private(&path, buid.as_bytes());
        }
        buid
    }
}

#[cfg(test)]
pub struct MemoryPairStore {
    records: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

#[cfg(test)]
impl MemoryPairStore {
    pub fn new() -> Self {
        MemoryPairStore {
            records: Default::default(),
        }
    }
}

#[cfg(test)]
impl PairStore for MemoryPairStore {
    fn read(&self, udid: &str) -> Option<Vec<u8>> {
        self.records.lock().unwrap().get(udid).cloned()
    }

    fn save(&self, udid: &str, record: &[u8]) -> io::Result<()> {
        self.records
            .lock()
            .unwrap()
            .insert(udid.to_string(), record.to_vec());
        Ok(())
    }

    fn delete(&self, udid: &str) -> io::Result<()> {
        self.records.lock().unwrap().remove(udid);
        Ok(())
    }

    fn buid(&self) -> String {
        "TEST-BUID".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("iloader-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn records_roundtrip_and_delete() {
        let dir = temp_dir("pairs");
        let store = FilePairStore::new(&dir);
        assert_eq!(store.read("00008030-001C1D2E0A68802E"), None);
        store
            .save("00008030-001C1D2E0A68802E", b"<plist/>")
            .unwrap();
        assert_eq!(
            store.read("00008030-001C1D2E0A68802E").as_deref(),
            Some(&b"<plist/>"[..])
        );
        store.save("00008030-001C1D2E0A68802E", b"new").unwrap();
        assert_eq!(
            store.read("00008030-001C1D2E0A68802E").as_deref(),
            Some(&b"new"[..])
        );
        store.delete("00008030-001C1D2E0A68802E").unwrap();
        store.delete("00008030-001C1D2E0A68802E").unwrap();
        assert_eq!(store.read("00008030-001C1D2E0A68802E"), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ids_cannot_escape_the_directory() {
        let dir = temp_dir("escape");
        let store = FilePairStore::new(&dir);
        for bad in ["", "../x", "a/b", "a\\b", "x.plist", "a b", &"a".repeat(65)] {
            assert!(store.save(bad, b"x").is_err(), "{bad:?}");
            assert!(store.delete(bad).is_err(), "{bad:?}");
            assert_eq!(store.read(bad), None);
        }
        assert!(!dir.exists() || fs::read_dir(&dir).unwrap().next().is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn buid_is_created_once_and_kept() {
        let dir = temp_dir("buid");
        let first = FilePairStore::new(&dir).buid();
        assert_eq!(first.len(), 36);
        assert_eq!(first, first.to_uppercase());
        assert_eq!(FilePairStore::new(&dir).buid(), first);
        let _ = fs::remove_dir_all(dir);
    }
}
