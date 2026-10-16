//! Wires the muxer into the Android app: a Tauri plugin that talks to the
//! Kotlin `UsbPlugin`, a watcher that attaches iPhones as they are plugged
//! in, and the local usbmuxd socket the rest of iloader connects to.

use std::collections::{HashMap, HashSet};
use std::os::fd::{BorrowedFd, RawFd};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use idevice::usbmuxd::UsbmuxdAddr;
use serde::Deserialize;
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{Emitter, Manager, Runtime};
use tracing::{debug, info, warn};

use super::hub::{AttachedDevice, DeviceInfo, Hub, location_id, udid_from_serial};
use super::pairing;
use super::pair_store::FilePairStore;
use super::server;
use super::usbfs::UsbfsPipe;

/// How often the list of attached USB devices is checked.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long to leave a device alone after attaching it failed.
const RETRY_AFTER: Duration = Duration::from_secs(5);
/// Paths of unix sockets have to fit in `sockaddr_un.sun_path`.
const MAX_SOCKET_PATH: usize = 100;

#[derive(Deserialize)]
struct ListResponse {
    devices: Vec<UsbDevice>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsbDevice {
    name: String,
    has_permission: bool,
}

#[derive(Deserialize)]
struct PermissionResponse {
    granted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenedDevice {
    fd: RawFd,
    serial: String,
    product_id: u16,
    ep_in: u8,
    ep_out: u8,
    max_packet_out: usize,
}

/// The Tauri plugin; register it with `Builder::plugin`.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("usbmux")
        .setup(|app, api| {
            let handle = Arc::new(api.register_android_plugin("me.nabdev.iloader", "UsbPlugin")?);
            let data_dir = app.path().app_data_dir()?;
            let hub = Hub::new(Box::new(FilePairStore::new(data_dir.join("lockdown"))));
            let socket = data_dir.join("usbmuxd.sock");
            let listener = bind_private_socket(&socket)?;
            super::set_addr_override(UsbmuxdAddr::UnixSocket(
                socket.to_string_lossy().into_owned(),
            ));

            let serving = hub.clone();
            tauri::async_runtime::spawn(async move {
                match tokio::net::UnixListener::from_std(listener) {
                    Ok(listener) => server::serve_unix(listener, serving).await,
                    Err(err) => warn!("could not start the usbmux server: {err}"),
                }
            });
            // Tells the UI to ask the user to tap "Trust" on the iPhone.
            let mut pairing_events = hub.subscribe_pairing();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match pairing_events.recv().await {
                        Ok(event) => {
                            let _ = app.emit("usbmux-pairing", event);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            tauri::async_runtime::spawn(watch_devices(handle, hub));
            Ok(())
        })
        .build()
}

/// Other apps cannot reach a socket in our private data directory, which
/// matters because pairing records give full control of the iPhone.
fn bind_private_socket(path: &Path) -> std::io::Result<std::os::unix::net::UnixListener> {
    use std::os::unix::fs::PermissionsExt;

    if path.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "socket path is too long",
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(path);
    let listener = std::os::unix::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

async fn watch_devices<R: Runtime>(handle: Arc<PluginHandle<R>>, hub: Hub) {
    // Kotlin device name -> hub id
    let mut attached: HashMap<String, u32> = HashMap::new();
    let mut asked: HashSet<String> = HashSet::new();
    let mut retry_at: HashMap<String, Instant> = HashMap::new();

    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let list: ListResponse = match handle
            .run_mobile_plugin_async("listDevices", serde_json::json!({}))
            .await
        {
            Ok(list) => list,
            Err(err) => {
                warn!("could not list USB devices: {err}");
                continue;
            }
        };
        let present: HashSet<&str> = list.devices.iter().map(|d| d.name.as_str()).collect();

        // Unplugged, or dropped by the hub because the link failed.
        let live: HashSet<u32> = hub.devices().iter().map(|d| d.id).collect();
        let stale: Vec<String> = attached
            .iter()
            .filter(|(name, id)| !present.contains(name.as_str()) || !live.contains(id))
            .map(|(name, _)| name.clone())
            .collect();
        for name in stale {
            if let Some(id) = attached.remove(&name) {
                hub.remove_device(id);
            }
            close_device(&handle, &name).await;
            if present.contains(name.as_str()) {
                retry_at.insert(name, Instant::now() + RETRY_AFTER);
            }
        }
        asked.retain(|name| present.contains(name.as_str()));
        retry_at.retain(|name, _| present.contains(name.as_str()));

        for device in &list.devices {
            if attached.contains_key(&device.name) {
                continue;
            }
            if retry_at
                .get(&device.name)
                .is_some_and(|at| *at > Instant::now())
            {
                continue;
            }
            if !device.has_permission {
                // Asking blocks until the user answered the dialog, so do it
                // off the polling loop; the next poll sees the result.
                if asked.insert(device.name.clone()) {
                    let handle = handle.clone();
                    let name = device.name.clone();
                    tauri::async_runtime::spawn(async move {
                        info!("asking for permission to use {name}");
                        let answer: Result<PermissionResponse, _> = handle
                            .run_mobile_plugin_async(
                                "requestPermission",
                                serde_json::json!({ "name": name }),
                            )
                            .await;
                        match answer {
                            Ok(answer) if !answer.granted => info!("USB permission denied"),
                            Ok(_) => {}
                            Err(err) => warn!("USB permission request failed: {err}"),
                        }
                    });
                }
                continue;
            }
            match attach(&handle, &hub, &device.name).await {
                Ok(id) => {
                    attached.insert(device.name.clone(), id);
                }
                Err(err) => {
                    warn!("could not attach {}: {err}", device.name);
                    close_device(&handle, &device.name).await;
                    retry_at.insert(device.name.clone(), Instant::now() + RETRY_AFTER);
                }
            }
        }
    }
}

async fn close_device<R: Runtime>(handle: &PluginHandle<R>, name: &str) {
    let result: Result<serde_json::Value, _> = handle
        .run_mobile_plugin_async("closeDevice", serde_json::json!({ "name": name }))
        .await;
    if let Err(err) = result {
        debug!("closing {name}: {err}");
    }
}

async fn attach<R: Runtime>(
    handle: &PluginHandle<R>,
    hub: &Hub,
    name: &str,
) -> Result<u32, String> {
    let opened: OpenedDevice = handle
        .run_mobile_plugin_async("openDevice", serde_json::json!({ "name": name }))
        .await
        .map_err(|err| err.to_string())?;

    // Kotlin keeps the connection (and with it its own descriptor) open;
    // work on a private copy so the two lifetimes stay independent.
    // SAFETY: the descriptor was just returned by `UsbDeviceConnection` and
    // is open for as long as that object lives, which includes this call.
    let fd = unsafe { BorrowedFd::borrow_raw(opened.fd) }
        .try_clone_to_owned()
        .map_err(|err| format!("could not duplicate the USB descriptor: {err}"))?;
    let pipe = UsbfsPipe::new(fd, opened.ep_in, opened.ep_out, opened.max_packet_out);

    let info = DeviceInfo {
        udid: udid_from_serial(&opened.serial),
        product_id: opened.product_id,
        location_id: location_id(name),
        speed: 480_000_000,
    };
    info!("attaching {} ({name})", info.udid);
    let id = hub
        .add_device(Arc::new(pipe), info.clone())
        .await
        .map_err(|err| err.to_string())?;
    pairing::spawn(hub, AttachedDevice { id, info });
    Ok(id)
}
