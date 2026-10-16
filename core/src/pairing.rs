use std::collections::HashMap;

// used https://github.com/jkcoxson/idevice_pair/ as a guide
use idevice::{
    IdeviceError, IdeviceService,
    afc::AfcClient,
    house_arrest::HouseArrestClient,
    installation_proxy::InstallationProxyClient,
    lockdown::LockdownClient,
    provider::IdeviceProvider,
    remote_pairing::{RemotePairingLockdownService, RpPairingFile},
    usbmuxd::UsbmuxdConnection,
};
use isideload::util::storage::{InMemoryStorage, SideloadingStorage};
use plist_macro::{plist, plist_to_xml_bytes};
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    Core,
    device::{DeviceInfo, get_provider},
    error::AppError,
};

pub(crate) struct PairingStorageEntry {
    keyring_enabled: bool,
    storage: Box<dyn SideloadingStorage>,
}

const PAIRING_APPS: &[(&str, &str)] = &[
    (
        "SideStore",
        "ALTPairingFile.mobiledevicepairing;PairingFile_RemoteRP.plist;PairingFile_Lockdown.plist",
    ),
    (
        "LiveContainer",
        "SideStore/Documents/ALTPairingFile.mobiledevicepairing",
    ),
    ("Feather", "pairingFile.plist"),
    ("StikDebug", "pairingFile.plist"),
    ("StikDebug (Sideloaded)", "pairingFile.plist"),
    ("StikTest", "stiktest_pairing.plist"),
    ("Protokolle", "pairingFile.plist"),
    ("Antrag", "pairingFile.plist"),
    ("SparseBox", "pairingFile.plist"),
    ("StikStore", "pairingFile.plist"),
    ("ByeTunes", "pairing file/pairingFile.plist"),
    ("Reynard", "pairingFile.plist"),
    ("PanicAnalyzer", "pairingFile.plist"),
];

/// A pairing file to save somewhere the user picks.
#[derive(Debug, Clone)]
pub struct PairingExport {
    pub bytes: Vec<u8>,
    pub file_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingAppInfo {
    pub name: String,
    pub bundle_id: String,
    pub path: String,
}

async fn generate_lockdown_plist(
    device: &DeviceInfo,
    provider: &dyn IdeviceProvider,
    usbmuxd: &mut UsbmuxdConnection,
) -> Result<plist::Value, AppError> {
    let mut pairing_file = usbmuxd.get_pair_record(&device.udid).await.map_err(|e| {
        AppError::LockdownPairing(
            "Failed to get pairing record for device".into(),
            e.to_string(),
        )
    })?;

    pairing_file.udid = Some(device.udid.clone());

    let mut lc = LockdownClient::connect(provider).await.map_err(|e| {
        AppError::DeviceComsWithMessage("Failed to connect to lockdown".into(), e.to_string())
    })?;

    lc.start_session(&pairing_file).await.map_err(|e| {
        AppError::DeviceComsWithMessage("Failed to start lockdown session".into(), e.to_string())
    })?;

    lc.set_value(
        "EnableWifiDebugging",
        true.into(),
        Some("com.apple.mobile.wireless_lockdown"),
    )
    .await
    .map_err(|e| {
        AppError::LockdownPairing("Failed to enable wifi debugging".into(), e.to_string())
    })?;

    plist::Value::from_reader_xml(std::io::Cursor::new(pairing_file.serialize().map_err(
        |e| AppError::LockdownPairing("Failed to serialize pairing file".into(), e.to_string()),
    )?))
    .map_err(|e| {
        AppError::LockdownPairing(
            "Failed to parse pairing file as plist".into(),
            e.to_string(),
        )
    })
}

async fn generate_rppairing_plist(
    core: &Core,
    udid: &str,
    provider: &dyn IdeviceProvider,
) -> Result<(plist::Value, Vec<u8>), IdeviceError> {
    let hostname =
        host_label(core, udid).map_err(|e| IdeviceError::InternalError(e.to_string()))?;
    let bytes = generate_rppairing(provider, &hostname).await?.to_bytes();
    let plist = plist::Value::from_reader_xml(std::io::Cursor::new(&bytes))
        .map_err(|e| IdeviceError::InternalError(format!("Invalid RPPairing plist: {}", e)))?;
    Ok((plist, bytes))
}

fn new_host_label() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("iloader-{}", &id[..6])
}

fn host_label(core: &Core, udid: &str) -> Result<String, AppError> {
    core.with_pairing_storage(|storage| {
        let key = format!("host_label_{udid}");
        if let Some(value) = storage.retrieve_data(&key).map_err(|e| {
            AppError::Storage(
                "Failed to get pairing hostname from storage".into(),
                e.to_string(),
            )
        })? {
            return String::from_utf8(value).map_err(|e| {
                AppError::Storage("Stored pairing hostname is invalid".into(), e.to_string())
            });
        }

        let hostname = new_host_label();
        storage.store_data(&key, hostname.as_bytes()).map_err(|e| {
            AppError::Storage("Failed to store pairing hostname".into(), e.to_string())
        })?;
        Ok(hostname)
    })
}

async fn generate_rppairing(
    provider: &dyn IdeviceProvider,
    hostname: &str,
) -> Result<RpPairingFile, IdeviceError> {
    let service = RemotePairingLockdownService::connect(&*provider).await?;
    let mut client = service.into_client(&hostname).expect("no socket");

    let mut pairing_file = RpPairingFile::generate(&hostname);
    client
        .connect(&mut pairing_file, async || "000000".to_string())
        .await?;

    Ok(pairing_file)
}

pub async fn place_file(
    pairing: Vec<u8>,
    provider: &dyn IdeviceProvider,
    bundle_id: String,
    path: String,
) -> Result<(), AppError> {
    let house_arrest_client = HouseArrestClient::connect(provider).await.map_err(|e| {
        AppError::HouseArrest("Failed to connect to house arrest".into(), e.to_string())
    })?;

    let mut afc_client = house_arrest_client
        .vend_documents(bundle_id)
        .await
        .map_err(|e| AppError::HouseArrest("Failed to vend documents".into(), e.to_string()))?;

    let files = path.split(';').map(|s| s.trim()).filter(|s| !s.is_empty());
    for file in files {
        place_individual_file(&mut afc_client, pairing.clone(), file.to_string()).await?;
    }

    Ok(())
}

async fn place_individual_file(
    afc_client: &mut AfcClient,
    pairing: Vec<u8>,
    path: String,
) -> Result<(), AppError> {
    afc_client
        .mk_dir(format!(
            "/Documents/{}",
            path.rsplit_once('/').map(|x| x.0).unwrap_or("")
        ))
        .await
        .map_err(|e| {
            AppError::HouseArrest("Failed to create Documents directory".into(), e.to_string())
        })?;

    let mut file = afc_client
        .open(
            format!("/Documents/{}", path),
            idevice::afc::opcode::AfcFopenMode::Wr,
        )
        .await
        .map_err(|e| {
            AppError::HouseArrest("Failed to open file on device".into(), e.to_string())
        })?;

    file.write_entire(&pairing)
        .await
        .map_err(|e| AppError::HouseArrest("Failed to write pairing file".into(), e.to_string()))?;
    file.close()
        .await
        .map_err(|e| AppError::HouseArrest("Failed to close file".into(), e.to_string()))?;

    Ok(())
}

impl Core {
    pub async fn place_pairing(&self, bundle_id: String, path: String) -> Result<(), AppError> {
        let device = self.selected_device()?;

        let provider = get_provider(&device.info).await?;

        place_file(device.pairing, &provider, bundle_id, path).await
    }

    /// The pairing file of the selected device. Saving it is up to the UI;
    /// this is for advanced users who want to use the file with other tools,
    /// or just want a backup. Normal users should place it into the app they
    /// want to pair with, which transfers it automatically.
    pub async fn export_pairing(&self) -> Result<PairingExport, AppError> {
        let device = self.selected_device()?;
        Ok(PairingExport {
            bytes: device.pairing,
            file_name: "pairingFile.plist".into(),
        })
    }

    fn build_pairing_storage_entry(&self, keyring_enabled: bool) -> PairingStorageEntry {
        let storage = self.create_sideloading_storage().unwrap_or_else(|e| {
            error!(
                "Failed to create sideloading storage, storing pairing file in memory: {}",
                e
            );
            Box::new(InMemoryStorage::new())
        });

        PairingStorageEntry {
            keyring_enabled,
            storage,
        }
    }

    fn with_pairing_storage<T>(
        &self,
        f: impl FnOnce(&dyn SideloadingStorage) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let current_keyring_enabled = self.keyring_available();
        let mut guard = self
            .pairing_storage
            .lock()
            .map_err(|_| AppError::Misc("Failed to lock pairing storage".to_string()))?;

        let entry = match &mut *guard {
            Some(entry) if entry.keyring_enabled != current_keyring_enabled => {
                info!(
                    "Pairing storage backend changed at runtime (keyring_enabled: {} -> {}), recreating storage",
                    entry.keyring_enabled, current_keyring_enabled
                );
                *entry = self.build_pairing_storage_entry(current_keyring_enabled);
                entry
            }
            Some(entry) => entry,
            None => guard.insert(self.build_pairing_storage_entry(current_keyring_enabled)),
        };

        f(entry.storage.as_ref())
    }

    pub(crate) async fn pairing_file(
        &self,
        device: &DeviceInfo,
        usbmuxd: &mut UsbmuxdConnection,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>, AppError> {
        let provider = get_provider(device).await?;

        let lockdown_plist = tokio::select! {
            _ = cancel.cancelled() => {
                return Err(AppError::Canceled("Pairing".into()));
            }
            res = generate_lockdown_plist(device, &provider, usbmuxd) => res?
        };

        // rppairing is 17.4+
        if is_ios_version_below(device.version.as_str(), 17, 4) {
            let lockdown_dict = lockdown_plist.as_dictionary().cloned().ok_or_else(|| {
                AppError::LockdownPairing(
                    "Lockdown plist was not a dictionary".into(),
                    "Invalid lockdown plist".into(),
                )
            })?;
            return Ok(plist_to_xml_bytes(&lockdown_dict));
        }

        let cache_key = format!("rppairing1.1_file_{}", device.udid);

        let cached_rppairing = self.with_pairing_storage(|storage| {
            storage.retrieve_data(&cache_key).map_err(|e| {
                AppError::Storage("Failed to get RPPairing from storage".into(), e.to_string())
            })
        })?;

        let rppairing_plist = if let Some(cached) = cached_rppairing {
            match plist::Value::from_reader_xml(std::io::Cursor::new(&cached)) {
                Ok(plist) => plist,
                Err(e) => {
                    warn!(
                        "Cached RPPairing is invalid for device {}, regenerating: {}",
                        device.name, e
                    );

                    let (generated_plist, generated_bytes) = tokio::select! {
                        _ = cancel.cancelled() => {
                            return Err(AppError::Canceled("Pairing".into()));
                        }
                        res = generate_rppairing_plist(self, &device.udid, &provider) => {
                            res.map_err(|e| AppError::RemotePairing(e.to_string()))?
                        }
                    };

                    self.with_pairing_storage(|storage| {
                        storage
                            .store_data(&cache_key, &generated_bytes)
                            .map_err(|e| {
                                AppError::Storage("Failed to store RPPairing".into(), e.to_string())
                            })
                    })?;

                    generated_plist
                }
            }
        } else {
            info!("Generating new RPPairing for device {}", device.name);

            let (generated_plist, generated_bytes) = tokio::select! {
                _ = cancel.cancelled() => {
                    return Err(AppError::Canceled("Pairing".into()));
                }
                res = generate_rppairing_plist(self, &device.udid, &provider) => {
                    res.map_err(|e| AppError::RemotePairing(e.to_string()))?
                }
            };

            self.with_pairing_storage(|storage| {
                storage
                    .store_data(&cache_key, &generated_bytes)
                    .map_err(|e| {
                        AppError::Storage("Failed to store RPPairing".into(), e.to_string())
                    })
            })?;

            generated_plist
        };

        if cancel.is_cancelled() {
            return Err(AppError::Canceled("Pairing".into()));
        }

        let pairing_plist = plist!(dict {
            :< lockdown_plist,
            :< rppairing_plist,
        });

        Ok(plist_to_xml_bytes(&pairing_plist))
    }

    pub async fn delete_stored_rppairing(&self) -> Result<(), AppError> {
        let device = self.selected_device()?;

        let cache_key = format!("rppairing1.1_file_{}", device.info.udid);
        let host_label_key = format!("host_label_{}", device.info.udid);

        self.with_pairing_storage(|storage| {
            storage.delete(&cache_key).map_err(|e| {
                AppError::Storage("Failed to delete stored RPPairing".into(), e.to_string())
            })?;
            let hostname = new_host_label();
            storage
                .store_data(&host_label_key, hostname.as_bytes())
                .map_err(|e| {
                    AppError::Storage("Failed to rotate pairing hostname".into(), e.to_string())
                })
        })?;

        Ok(())
    }

    pub async fn has_stored_rppairing(&self, device: DeviceInfo) -> Result<bool, AppError> {
        // rppairing not supported <17.4, returning true so the frontend can still automatically select it
        if is_ios_version_below(&device.version, 17, 4) {
            return Ok(true);
        }
        let cache_key = format!("rppairing1.1_file_{}", device.udid);

        self.with_pairing_storage(|storage| {
            storage.retrieve_data(&cache_key).map_err(|e| {
                AppError::Storage("Failed to retrieve stored RPPairing".into(), e.to_string())
            })
        })
        .map(|opt| opt.is_some())
    }

    pub async fn installed_pairing_apps(&self) -> Result<Vec<PairingAppInfo>, AppError> {
        let device = self.selected_device()?;
        let provider = get_provider(&device.info).await?;
        let mut installation_proxy =
            InstallationProxyClient::connect(&provider)
                .await
                .map_err(|e| {
                    AppError::DeviceComsWithMessage(
                        "Failed to connect to installation proxy".into(),
                        e.to_string(),
                    )
                })?;

        let installed_apps = installation_proxy
            .get_apps(Some("User"), None)
            .await
            .map_err(|e| {
                AppError::DeviceComsWithMessage(
                    "Failed to get installed apps".into(),
                    e.to_string(),
                )
            })?;

        let mut installed = HashMap::new();
        for (bundle_id, app) in installed_apps {
            let n = app
                .as_dictionary()
                .and_then(|x| x.get("CFBundleDisplayName").and_then(|x| x.as_string()))
                .ok_or(AppError::Misc("Failed to parse installed apps".to_string()))?;

            if PAIRING_APPS.iter().any(|(name, _)| name == &n) {
                if bundle_id.contains("com.stik.stikdebug") {
                    installed.insert(format!("{} (Sideloaded)", n), bundle_id);
                } else {
                    installed.insert(n.to_string(), bundle_id);
                }
            }
        }

        let mut result = Vec::new();
        for (name, path) in PAIRING_APPS {
            if let Some(bundle_id) = installed.get(*name) {
                result.push(PairingAppInfo {
                    name: name.to_string(),
                    bundle_id: bundle_id.to_string(),
                    path: path.to_string(),
                });
            }
        }
        Ok(result)
    }
}

pub async fn get_sidestore_info(
    device: &DeviceInfo,
    live_container: bool,
) -> Result<Option<PairingAppInfo>, AppError> {
    let provider = get_provider(device).await?;
    let mut installation_proxy =
        InstallationProxyClient::connect(&provider)
            .await
            .map_err(|e| {
                AppError::DeviceComsWithMessage(
                    "Failed to connect to installation proxy".into(),
                    e.to_string(),
                )
            })?;

    let installed_apps = installation_proxy
        .get_apps(Some("User"), None)
        .await
        .map_err(|e| {
            AppError::DeviceComsWithMessage("Failed to get installed apps".into(), e.to_string())
        })?;

    for (bundle_id, app) in installed_apps {
        let n = app
            .as_dictionary()
            .and_then(|x| x.get("CFBundleDisplayName").and_then(|x| x.as_string()))
            .ok_or(AppError::Misc("Failed to parse installed apps".to_string()))?;

        if n == "SideStore" || (live_container && n == "LiveContainer") {
            return Ok(Some(PairingAppInfo {
                name: n.to_string(),
                bundle_id: bundle_id.to_string(),
                path: PAIRING_APPS
                    .iter()
                    .find(|(name, _)| name == &n)
                    .map(|(_, path)| path.to_string())
                    .unwrap_or_default(),
            }));
        }
    }

    Ok(None)
}

fn parse_version_component(segment: Option<&str>) -> u32 {
    segment
        .and_then(|s| {
            let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                None
            } else {
                digits.parse().ok()
            }
        })
        .unwrap_or(0)
}

fn is_ios_version_below(version: &str, target_major: u32, target_minor: u32) -> bool {
    let mut parts = version.split('.');
    let major = parse_version_component(parts.next());
    let minor = parse_version_component(parts.next());
    (major, minor) < (target_major, target_minor)
}
