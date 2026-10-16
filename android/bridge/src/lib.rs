//! UniFFI exports of `iloader-core` for the native Android app. The Kotlin
//! bindings are generated from the compiled library (see `android/README.md`).
//!
//! Types are flat records and enums so they map cleanly to Kotlin; the
//! `RealCore` class on the Kotlin side converts them to the UI models.

use std::path::PathBuf;
use std::sync::Arc;

use iloader_core::{
    AppError, CertificateInfo, Core, CoreEvent, DeviceInfo, Host, PairState, SecretStore,
    UpdateType,
};

uniffi::setup_scaffolding!();

/// `AppError` as it crosses the bridge: the snake_case variant name and the
/// display message.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum BridgeError {
    #[error("{detail}")]
    App { kind: String, detail: String },
}

impl From<AppError> for BridgeError {
    fn from(err: AppError) -> Self {
        BridgeError::App {
            kind: err.as_ref().to_string(),
            detail: err.to_string(),
        }
    }
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum SecretStoreError {
    #[error("{reason}")]
    Failed { reason: String },
    #[error("unexpected secret store error: {reason}")]
    Unexpected { reason: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for SecretStoreError {
    fn from(err: uniffi::UnexpectedUniFFICallbackError) -> Self {
        SecretStoreError::Unexpected {
            reason: err.reason,
        }
    }
}

/// Secret storage implemented in Kotlin (Android Keystore).
#[uniffi::export(with_foreign)]
pub trait BridgeSecretStore: Send + Sync {
    fn get(&self, key: String) -> Result<Option<String>, SecretStoreError>;
    fn set(&self, key: String, value: String) -> Result<(), SecretStoreError>;
    /// Returns whether there was something to delete.
    fn delete(&self, key: String) -> Result<bool, SecretStoreError>;
}

struct SecretAdapter(Arc<dyn BridgeSecretStore>);

fn reason(err: SecretStoreError) -> String {
    err.to_string()
}

impl SecretStore for SecretAdapter {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        self.0.get(key.to_string()).map_err(reason)
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.set(key.to_string(), value.to_string()).map_err(reason)
    }

    fn delete(&self, key: &str) -> Result<bool, String> {
        self.0.delete(key.to_string()).map_err(reason)
    }
}

/// Receives every `CoreEvent`; called from any thread and must not block.
#[uniffi::export(with_foreign)]
pub trait CoreListener: Send + Sync {
    fn on_event(&self, event: BridgeEvent);
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BridgeDevice {
    pub name: String,
    pub id: i32,
    pub udid: String,
    pub connection_type: String,
    pub version: String,
}

impl From<DeviceInfo> for BridgeDevice {
    fn from(d: DeviceInfo) -> Self {
        BridgeDevice {
            name: d.name,
            id: d.id as i32,
            udid: d.udid,
            connection_type: d.connection_type,
            version: d.version,
        }
    }
}

impl From<BridgeDevice> for DeviceInfo {
    fn from(d: BridgeDevice) -> Self {
        DeviceInfo {
            name: d.name,
            id: d.id as u32,
            udid: d.udid,
            connection_type: d.connection_type,
            version: d.version,
        }
    }
}

/// One entry of the device list: a device or the error that device produced.
#[derive(Debug, Clone, uniffi::Record)]
pub struct BridgeDeviceResult {
    pub device: Option<BridgeDevice>,
    pub error: Option<BridgeErrorInfo>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BridgeErrorInfo {
    pub kind: String,
    pub detail: String,
}

impl From<&AppError> for BridgeErrorInfo {
    fn from(err: &AppError) -> Self {
        BridgeErrorInfo {
            kind: err.as_ref().to_string(),
            detail: err.to_string(),
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BridgeCertificate {
    pub name: Option<String>,
    pub certificate_id: Option<String>,
    pub serial_number: Option<String>,
    pub machine_name: Option<String>,
    pub machine_id: Option<String>,
}

impl From<CertificateInfo> for BridgeCertificate {
    fn from(c: CertificateInfo) -> Self {
        BridgeCertificate {
            name: c.name,
            certificate_id: c.certificate_id,
            serial_number: c.serial_number,
            machine_name: c.machine_name,
            machine_id: c.machine_id,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BridgeAppId {
    pub app_id_id: String,
    pub identifier: String,
    pub name: String,
    /// ISO-8601 date time; `None` when the App ID never expires.
    pub expiration_date: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BridgeAppIds {
    pub app_ids: Vec<BridgeAppId>,
    pub max_quantity: i64,
    pub available_quantity: i64,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BridgePairingApp {
    pub name: String,
    pub bundle_id: String,
    pub path: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BridgePairingExport {
    pub bytes: Vec<u8>,
    pub file_name: String,
}

#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum BridgeUpdateType {
    Started,
    Finished,
    Failed,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum BridgePairState {
    Pairing,
    WaitingForTrust,
    Paired,
    Denied,
    Failed { message: String },
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum BridgeEvent {
    TwoFactorRequired,
    MaxCertsReached {
        certificates: Vec<BridgeCertificate>,
    },
    Operation {
        operation_id: String,
        update_type: BridgeUpdateType,
        step_id: String,
        error: Option<BridgeErrorInfo>,
    },
    Log {
        /// 1 trace, 2 debug, 3 info, 4 warn, 5 error.
        level: i32,
        message: String,
        target: Option<String>,
        timestamp: String,
    },
    UsbPairing {
        udid: String,
        state: BridgePairState,
    },
}

impl From<CoreEvent> for BridgeEvent {
    fn from(event: CoreEvent) -> Self {
        match event {
            CoreEvent::TwoFactorRequired(_) => BridgeEvent::TwoFactorRequired,
            CoreEvent::MaxCertsReached(certs) => BridgeEvent::MaxCertsReached {
                certificates: certs.into_iter().map(Into::into).collect(),
            },
            CoreEvent::Operation {
                operation_id,
                update,
            } => BridgeEvent::Operation {
                operation_id,
                update_type: match update.update_type {
                    UpdateType::Started => BridgeUpdateType::Started,
                    UpdateType::Finished => BridgeUpdateType::Finished,
                    UpdateType::Failed => BridgeUpdateType::Failed,
                },
                step_id: update.step_id,
                error: update.extra_details.as_ref().map(Into::into),
            },
            CoreEvent::Log(record) => BridgeEvent::Log {
                level: record.level as i32,
                message: record.message,
                target: record.target,
                timestamp: record.timestamp,
            },
            CoreEvent::UsbPairing(event) => BridgeEvent::UsbPairing {
                udid: event.udid,
                state: match event.state {
                    PairState::Pairing => BridgePairState::Pairing,
                    PairState::WaitingForTrust => BridgePairState::WaitingForTrust,
                    PairState::Paired => BridgePairState::Paired,
                    PairState::Denied => BridgePairState::Denied,
                    PairState::Failed(message) => BridgePairState::Failed { message },
                },
            },
        }
    }
}

struct BridgeHost {
    data_dir: PathBuf,
    cache_dir: PathBuf,
    secrets: Arc<dyn SecretStore>,
    listener: Arc<dyn CoreListener>,
}

impl Host for BridgeHost {
    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    fn cache_dir(&self) -> PathBuf {
        self.cache_dir.clone()
    }

    fn secrets(&self) -> Option<Arc<dyn SecretStore>> {
        Some(self.secrets.clone())
    }

    fn emit(&self, event: CoreEvent) {
        self.listener.on_event(event.into());
    }
}

/// The handle Kotlin holds; one method per `Core` method.
#[derive(uniffi::Object)]
pub struct BridgeCore {
    core: Arc<Core>,
}

type Res<T> = Result<T, BridgeError>;

#[uniffi::export(async_runtime = "tokio")]
impl BridgeCore {
    #[uniffi::constructor]
    pub fn new(
        data_dir: String,
        cache_dir: String,
        secrets: Arc<dyn BridgeSecretStore>,
        listener: Arc<dyn CoreListener>,
    ) -> Arc<Self> {
        let host = BridgeHost {
            data_dir: data_dir.into(),
            cache_dir: cache_dir.into(),
            secrets: Arc::new(SecretAdapter(secrets)),
            listener,
        };
        Arc::new(BridgeCore {
            core: Core::new(Arc::new(host)),
        })
    }

    // account
    pub async fn login_new(
        &self,
        email: String,
        password: String,
        anisette_server: String,
        save_credentials: bool,
    ) -> Res<()> {
        Ok(self
            .core
            .login_new(email, password, anisette_server, save_credentials)
            .await?)
    }

    pub async fn login_stored(&self, email: String, anisette_server: String) -> Res<()> {
        Ok(self.core.login_stored(email, anisette_server).await?)
    }

    pub fn logged_in_as(&self) -> Option<String> {
        self.core.logged_in_as()
    }

    pub fn invalidate_account(&self) {
        self.core.invalidate_account()
    }

    pub fn stored_accounts(&self) -> Vec<String> {
        self.core.stored_accounts()
    }

    pub fn delete_account(&self, email: String) -> Res<()> {
        Ok(self.core.delete_account(email)?)
    }

    pub fn submit_two_factor(&self, code: String) {
        self.core.submit_two_factor(code)
    }

    pub fn respond_max_certs(&self, serials: Option<Vec<String>>) {
        self.core.respond_max_certs(serials)
    }

    pub async fn certificates(&self) -> Res<Vec<BridgeCertificate>> {
        Ok(self
            .core
            .certificates()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn revoke_certificate(&self, serial_number: String) -> Res<()> {
        Ok(self.core.revoke_certificate(serial_number).await?)
    }

    pub async fn list_app_ids(&self) -> Res<BridgeAppIds> {
        let res = self.core.list_app_ids().await?;
        Ok(BridgeAppIds {
            app_ids: res
                .app_ids
                .into_iter()
                .map(|a| BridgeAppId {
                    app_id_id: a.app_id_id,
                    identifier: a.identifier,
                    name: a.name,
                    expiration_date: a.expiration_date.map(|d| d.to_xml_format()),
                })
                .collect(),
            max_quantity: res.max_quantity.map_or(0, |v| v as i64),
            available_quantity: res.available_quantity.unwrap_or(0),
        })
    }

    pub async fn delete_app_id(&self, app_id_id: String) -> Res<()> {
        Ok(self.core.delete_app_id(app_id_id).await?)
    }

    // devices
    pub async fn list_devices(&self) -> Res<Vec<BridgeDeviceResult>> {
        Ok(self
            .core
            .list_devices()
            .await?
            .into_iter()
            .map(|r| match r {
                Ok(d) => BridgeDeviceResult {
                    device: Some(d.into()),
                    error: None,
                },
                Err(e) => BridgeDeviceResult {
                    device: None,
                    error: Some((&e).into()),
                },
            })
            .collect())
    }

    pub async fn select_device(&self, device: Option<BridgeDevice>) -> Res<()> {
        Ok(self.core.select_device(device.map(Into::into)).await?)
    }

    pub fn cancel_pairing(&self) {
        self.core.cancel_pairing()
    }

    // operations
    pub async fn sideload(&self, app_path: String) -> Res<()> {
        Ok(self.core.sideload(app_path).await?)
    }

    pub async fn install_sidestore(&self, nightly: bool, live_container: bool) -> Res<()> {
        Ok(self.core.install_sidestore(nightly, live_container).await?)
    }

    // pairing files
    pub async fn installed_pairing_apps(&self) -> Res<Vec<BridgePairingApp>> {
        Ok(self
            .core
            .installed_pairing_apps()
            .await?
            .into_iter()
            .map(|a| BridgePairingApp {
                name: a.name,
                bundle_id: a.bundle_id,
                path: a.path,
            })
            .collect())
    }

    pub async fn place_pairing(&self, bundle_id: String, path: String) -> Res<()> {
        Ok(self.core.place_pairing(bundle_id, path).await?)
    }

    pub async fn export_pairing(&self) -> Res<BridgePairingExport> {
        let export = self.core.export_pairing().await?;
        Ok(BridgePairingExport {
            bytes: export.bytes,
            file_name: export.file_name,
        })
    }

    pub async fn has_stored_rppairing(&self, device: BridgeDevice) -> Res<bool> {
        Ok(self.core.has_stored_rppairing(device.into()).await?)
    }

    pub async fn delete_stored_rppairing(&self) -> Res<()> {
        Ok(self.core.delete_stored_rppairing().await?)
    }

    // settings related
    pub fn keyring_available(&self) -> bool {
        self.core.keyring_available()
    }

    pub fn force_disable_keyring(&self, force: bool) {
        self.core.force_disable_keyring(force)
    }

    pub fn reset_anisette_state(&self) -> Res<bool> {
        Ok(self.core.reset_anisette_state()?)
    }

    // USB host (Android only)
    pub async fn attach_usb_device(
        &self,
        fd: i32,
        serial: String,
        product_id: i32,
        ep_in: i32,
        ep_out: i32,
        max_packet_out: i32,
        name: String,
    ) -> Res<()> {
        #[cfg(target_os = "android")]
        {
            Ok(self
                .core
                .attach_usb_device(
                    fd,
                    serial,
                    product_id as u16,
                    ep_in as u8,
                    ep_out as u8,
                    max_packet_out as u32,
                    name,
                )
                .await?)
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = (fd, serial, product_id, ep_in, ep_out, max_packet_out, name);
            Err(AppError::Misc("USB host is only available on Android".into()).into())
        }
    }

    pub fn detach_usb_device(&self, name: String) {
        #[cfg(target_os = "android")]
        self.core.detach_usb_device(name);
        #[cfg(not(target_os = "android"))]
        let _ = name;
    }
}
