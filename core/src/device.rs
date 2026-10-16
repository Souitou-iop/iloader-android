use std::sync::Mutex;

use idevice::{
    IdeviceService,
    lockdown::LockdownClient,
    provider::UsbmuxdProvider,
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection},
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{Core, error::AppError};

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub name: String,
    pub id: u32,
    pub udid: String,
    pub connection_type: String,
    pub version: String,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfoWithPairing {
    pub info: DeviceInfo,
    pub pairing: Vec<u8>,
}

pub type DeviceInfoMutex = Mutex<Option<DeviceInfoWithPairing>>;
pub type PairingCancelToken = Mutex<Option<CancellationToken>>;

pub async fn list_devices() -> Result<Vec<Result<DeviceInfo, AppError>>, AppError> {
    let mut usbmuxd = get_usbmuxd().await?;

    let devs = usbmuxd.get_devices().await.map_err(|e| {
        AppError::Usbmuxd("Failed to list devices from usbmuxd".into(), e.to_string())
    })?;
    if devs.is_empty() {
        return Ok(vec![]);
    }

    let usbmuxd_addr = usbmuxd_addr().map_err(|e| {
        AppError::Usbmuxd(
            "Invalid usbmuxd address from environment".into(),
            e.to_string(),
        )
    })?;

    let device_info_futures: Vec<_> = devs
        .iter()
        .map(|d| {
            let usbmuxd_addr = usbmuxd_addr.clone();
            async move {
                let provider = d.to_provider(usbmuxd_addr, "iloader");
                let device_uid = d.device_id;
                let connection_type = match d.connection_type {
                    Connection::Usb => "USB",
                    Connection::Network(_) => "Network",
                    Connection::Unknown(_) => "Unknown",
                }
                .to_string();

                let mut lockdown_client =
                    LockdownClient::connect(&provider).await.map_err(|e| {
                        eprintln!("Unable to connect to lockdown for {}: {e:?}", d.udid);
                        AppError::DeviceComsWithMessage(
                            "Unable to connect to lockdown".into(),
                            e.to_string(),
                        )
                    })?;

                let device_name_value = lockdown_client
                    .get_value(Some("DeviceName"), None)
                    .await
                    .map_err(|e| {
                    eprintln!("Failed to fetch DeviceName for {}: {e:?}", d.udid);
                    AppError::DeviceComsWithMessage(
                        "Failed to fetch DeviceName".into(),
                        e.to_string(),
                    )
                })?;

                let device_name = device_name_value.as_string().ok_or_else(|| {
                    eprintln!("DeviceName for {} was not a string", d.udid);
                    AppError::DeviceComs("DeviceName was not a string".into())
                })?;

                let version_value = lockdown_client
                    .get_value(Some("ProductVersion"), None)
                    .await
                    .map_err(|e| {
                        eprintln!("Failed to fetch ProductVersion for {}: {e:?}", d.udid);
                        AppError::DeviceComsWithMessage(
                            "Failed to fetch ProductVersion".into(),
                            e.to_string(),
                        )
                    })?;

                let version = version_value.as_string().ok_or_else(|| {
                    eprintln!("ProductVersion for {} was not a string", d.udid);
                    AppError::DeviceComs("Product version was not a string".into())
                })?;

                Ok::<DeviceInfo, AppError>(DeviceInfo {
                    name: device_name.to_string(),
                    id: device_uid,
                    udid: d.udid.clone(),
                    connection_type,
                    version: version.to_string(),
                })
            }
        })
        .collect();

    let device_infos = futures::future::join_all(device_info_futures).await;
    Ok(device_infos)
}

impl Core {
    pub async fn list_devices(&self) -> Result<Vec<Result<DeviceInfo, AppError>>, AppError> {
        list_devices().await
    }

    pub async fn select_device(&self, device: Option<DeviceInfo>) -> Result<(), AppError> {
        let Some(device) = device else {
            *self.device.lock().unwrap() = None;
            return Ok(());
        };

        let mut usbmuxd = get_usbmuxd().await?;

        let token = tokio_util::sync::CancellationToken::new();
        {
            let mut guard = self.pairing_cancel.lock().unwrap();
            if let Some(old) = guard.replace(token.clone()) {
                old.cancel();
            }
        }

        let pairing_result = self
            .pairing_file(&device, &mut usbmuxd, token.clone())
            .await;

        if !token.is_cancelled() {
            *self.pairing_cancel.lock().unwrap() = None;
        }

        let pairing = pairing_result?;

        *self.device.lock().unwrap() = Some(DeviceInfoWithPairing {
            info: device,
            pairing,
        });
        Ok(())
    }

    pub fn cancel_pairing(&self) {
        if let Some(token) = self.pairing_cancel.lock().unwrap().take() {
            token.cancel();
        }
    }

    pub(crate) fn selected_device(&self) -> Result<DeviceInfoWithPairing, AppError> {
        self.device
            .lock()
            .unwrap()
            .clone()
            .ok_or(AppError::NoDeviceSelected)
    }
}

/// Address providers use to reach a device through usbmuxd. On Android that
/// is the muxer built into the app.
fn usbmuxd_addr() -> Result<UsbmuxdAddr, std::net::AddrParseError> {
    match crate::usbmux::addr_override() {
        Some(addr) => Ok(addr),
        None => UsbmuxdAddr::from_env_var(),
    }
}

pub async fn get_usbmuxd() -> Result<UsbmuxdConnection, AppError> {
    let connection = match crate::usbmux::addr_override() {
        Some(addr) => addr.connect(0).await,
        None => UsbmuxdConnection::default().await,
    };
    connection.map_err(|e| AppError::Usbmuxd("Failed to connect to usbmuxd".into(), e.to_string()))
}

pub async fn get_provider(device_info: &DeviceInfo) -> Result<UsbmuxdProvider, AppError> {
    get_provider_from_connection(device_info, &mut (get_usbmuxd().await?)).await
}

pub async fn get_provider_from_connection(
    device_info: &DeviceInfo,
    connection: &mut UsbmuxdConnection,
) -> Result<UsbmuxdProvider, AppError> {
    let device = connection
        .get_device(&device_info.udid)
        .await
        .map_err(|e| {
            AppError::DeviceComsWithMessage("Failed to get device".into(), e.to_string())
        })?;

    let provider = device.to_provider(usbmuxd_addr().unwrap(), "iloader");
    Ok(provider)
}
