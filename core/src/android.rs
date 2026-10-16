//! The built-in usbmuxd for Android: a hub of attached iPhones served on a
//! private unix socket the rest of the core connects to. The app's USB host
//! code opens the device and hands the descriptor to `Core::attach_usb_device`.

use std::collections::HashMap;
use std::os::fd::{BorrowedFd, RawFd};
use std::sync::{Arc, Mutex};

use idevice::usbmuxd::UsbmuxdAddr;
use tracing::{info, warn};

use crate::usbmux::hub::{AttachedDevice, DeviceInfo, Hub, location_id, udid_from_serial};
use crate::usbmux::pair_store::FilePairStore;
use crate::usbmux::usbfs::UsbfsPipe;
use crate::usbmux::{self, pairing, server};
use crate::{AppError, Core, CoreEvent, Host};

pub(crate) struct AndroidState {
    rt: tokio::runtime::Runtime,
    hub: Hub,
    /// Name the app gave the USB device -> hub id.
    attached: Mutex<HashMap<String, u32>>,
}

impl AndroidState {
    pub(crate) fn start(host: &Arc<dyn Host>) -> AndroidState {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("iloader-usbmux")
            .enable_all()
            .build()
            .expect("failed to start the usbmux runtime");
        let data_dir = host.data_dir();
        let hub = Hub::new(Box::new(FilePairStore::new(data_dir.join("lockdown"))));

        // Something else (the Tauri plugin) may have started a muxer already.
        if usbmux::addr_override().is_none() {
            let socket = data_dir.join("usbmuxd.sock");
            match usbmux::bind_private_socket(&socket) {
                Ok(listener) => {
                    usbmux::set_addr_override(UsbmuxdAddr::UnixSocket(
                        socket.to_string_lossy().into_owned(),
                    ));
                    let serving = hub.clone();
                    rt.spawn(async move {
                        match tokio::net::UnixListener::from_std(listener) {
                            Ok(listener) => server::serve_unix(listener, serving).await,
                            Err(err) => warn!("could not start the usbmux server: {err}"),
                        }
                    });
                }
                Err(err) => warn!("could not bind the usbmux socket: {err}"),
            }
        }

        // Tells the UI to ask the user to tap "Trust" on the iPhone.
        let mut pairing_events = hub.subscribe_pairing();
        let host = host.clone();
        rt.spawn(async move {
            loop {
                match pairing_events.recv().await {
                    Ok(event) => host.emit(CoreEvent::UsbPairing(event)),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        AndroidState {
            rt,
            hub,
            attached: Mutex::new(HashMap::new()),
        }
    }
}

impl Core {
    /// Adds an iPhone the app opened through `UsbManager`. `name` identifies
    /// the device in `detach_usb_device`; the core works on a private copy of
    /// `fd`, so the caller keeps ownership of its own descriptor.
    pub async fn attach_usb_device(
        &self,
        fd: i32,
        serial: String,
        product_id: u16,
        ep_in: u8,
        ep_out: u8,
        max_packet_out: u32,
        name: String,
    ) -> Result<(), AppError> {
        let state = &self.android;
        let hub = state.hub.clone();
        let stale = state.attached.lock().unwrap().remove(&name);
        if let Some(id) = stale {
            hub.remove_device(id);
        }

        // SAFETY: the caller guarantees `fd` is open for the duration of this
        // call; it is duplicated right away so the lifetimes stay independent.
        let fd = unsafe { BorrowedFd::borrow_raw(fd as RawFd) }
            .try_clone_to_owned()
            .map_err(|err| {
                AppError::Usbmuxd(
                    "Could not duplicate the USB descriptor".into(),
                    err.to_string(),
                )
            })?;
        let pipe = UsbfsPipe::new(fd, ep_in, ep_out, max_packet_out as usize);
        let info = DeviceInfo {
            udid: udid_from_serial(&serial),
            product_id,
            location_id: location_id(&name),
            speed: 480_000_000,
        };
        info!("attaching {} ({name})", info.udid);

        // The hub starts tasks, which need the runtime that serves the muxer.
        let added = {
            let hub = hub.clone();
            let info = info.clone();
            state
                .rt
                .spawn(async move {
                    let id = hub.add_device(Arc::new(pipe), info.clone()).await?;
                    pairing::spawn(&hub, AttachedDevice { id, info });
                    Ok::<u32, crate::usbmux::hub::HubError>(id)
                })
                .await
        };
        let id = added
            .map_err(|err| {
                AppError::Usbmuxd("Attaching the device failed".into(), err.to_string())
            })?
            .map_err(|err| {
                AppError::Usbmuxd("Could not attach the device".into(), err.to_string())
            })?;
        state.attached.lock().unwrap().insert(name, id);
        Ok(())
    }

    /// Removes a device added with `attach_usb_device`, for example when it
    /// was unplugged.
    pub fn detach_usb_device(&self, name: String) {
        let id = self.android.attached.lock().unwrap().remove(&name);
        if let Some(id) = id {
            self.android.hub.remove_device(id);
        }
    }
}
