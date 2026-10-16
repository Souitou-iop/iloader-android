//! The iloader backend without any UI framework: Apple ID sign-in,
//! sideloading, device pairing and (on Android) a built-in usbmuxd.
//!
//! Everything the UI needs is a method on [`Core`]. The embedding app
//! implements [`Host`] to tell the core where to keep its files, how to
//! store secrets and how to deliver [`CoreEvent`]s.

mod account;
#[cfg(target_os = "android")]
mod android;
mod device;
mod error;
mod logging;
mod operation;
mod pairing;
mod secure_storage;
mod sideload;
mod store;
pub mod usbmux;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, Weak, atomic::AtomicBool},
};

use tokio::sync::oneshot;

pub use account::CertificateInfo;
pub use device::{DeviceInfo, DeviceInfoWithPairing};
pub use error::AppError;
pub use isideload::{
    auth::apple_account::TwoFactorCallbackParams as TwoFactorParams,
    dev::app_ids::ListAppIdsResponse,
};
pub use logging::LogRecord;
pub use operation::{OperationUpdate, UpdateType};
pub use pairing::{PairingAppInfo, PairingExport};
pub use secure_storage::{KeyringSecretStore, SecretStore};
pub use store::{JsonFileKvStore, KvStore};
pub use usbmux::pairing::{PairEvent, PairState};

/// What the embedding app provides to the core.
pub trait Host: Send + Sync {
    /// App private directory (stores, logs, usbmux socket, caches).
    fn data_dir(&self) -> PathBuf;

    /// Where downloads are kept. Defaults to a folder in `data_dir`.
    fn cache_dir(&self) -> PathBuf {
        self.data_dir().join("cache")
    }

    /// Secret storage for saved Apple ID passwords and the anisette state.
    /// Desktop: the OS keyring. Android: Keystore backed.
    fn secrets(&self) -> Option<Arc<dyn SecretStore>>;

    /// Store for non-secret settings. Defaults to `data.json` in `data_dir`.
    fn kv(&self) -> Option<Arc<dyn KvStore>> {
        None
    }

    /// Called from any thread; must not block.
    fn emit(&self, event: CoreEvent);
}

#[derive(Debug, Clone)]
pub enum CoreEvent {
    /// Ask the user for a 2FA code, then call [`Core::submit_two_factor`].
    TwoFactorRequired(TwoFactorParams),
    /// Ask the user which certificates to revoke, then call
    /// [`Core::respond_max_certs`].
    MaxCertsReached(Vec<CertificateInfo>),
    Operation {
        operation_id: String,
        update: OperationUpdate,
    },
    Log(LogRecord),
    UsbPairing(PairEvent),
}

type SideloaderMutex = Mutex<
    Option<
        isideload::sideload::sideloader::Sideloader<
            isideload::util::callbacks::MaxCertsCallbackBox,
        >,
    >,
>;

pub struct Core {
    pub(crate) host: Arc<dyn Host>,
    pub(crate) kv: Arc<dyn KvStore>,
    pub(crate) this: Weak<Core>,
    pub(crate) sideloader: SideloaderMutex,
    pub(crate) device: device::DeviceInfoMutex,
    pub(crate) pairing_cancel: device::PairingCancelToken,
    pub(crate) pending_two_factor: Mutex<Option<oneshot::Sender<String>>>,
    pub(crate) pending_max_certs: Mutex<Option<oneshot::Sender<Option<Vec<String>>>>>,
    pub(crate) force_disable_keyring: AtomicBool,
    pub(crate) pairing_storage: Mutex<Option<pairing::PairingStorageEntry>>,
    #[cfg(target_os = "android")]
    pub(crate) android: android::AndroidState,
}

impl Core {
    pub fn new(host: Arc<dyn Host>) -> Arc<Core> {
        logging::init(&host);
        let kv = host
            .kv()
            .unwrap_or_else(|| Arc::new(JsonFileKvStore::open(host.data_dir().join("data.json"))));
        Arc::new_cyclic(|this| Core {
            #[cfg(target_os = "android")]
            android: android::AndroidState::start(&host),
            host,
            kv,
            this: this.clone(),
            sideloader: Mutex::new(None),
            device: Mutex::new(None),
            pairing_cancel: Mutex::new(None),
            pending_two_factor: Mutex::new(None),
            pending_max_certs: Mutex::new(None),
            force_disable_keyring: AtomicBool::new(false),
            pairing_storage: Mutex::new(None),
        })
    }

    /// The 2FA code the user entered; ignored when nothing is waiting.
    pub fn submit_two_factor(&self, code: String) {
        if let Some(tx) = self.pending_two_factor.lock().unwrap().take() {
            let _ = tx.send(code.trim().to_string());
        }
    }

    /// The certificates to revoke, or `None` to cancel; ignored when nothing
    /// is waiting.
    pub fn respond_max_certs(&self, serials: Option<Vec<String>>) {
        if let Some(tx) = self.pending_max_certs.lock().unwrap().take() {
            let _ = tx.send(serials);
        }
    }
}
