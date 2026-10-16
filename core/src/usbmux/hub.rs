use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::broadcast;
use tracing::{info, warn};

use super::MuxError;
use super::link::{ConnectError, DeviceStream, Link};
use super::pair_store::PairStore;
use super::pairing::{PairEvent, PairState};
use super::pipe::UsbPipe;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub udid: String,
    pub product_id: u16,
    pub location_id: u32,
    /// Bits per second.
    pub speed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedDevice {
    pub id: u32,
    pub info: DeviceInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubEvent {
    Attached(AttachedDevice),
    Detached(u32),
}

#[derive(Debug, thiserror::Error)]
pub enum HubError {
    #[error("the device did not complete the usbmux handshake: {0}")]
    Handshake(MuxError),
    #[error("timed out waiting for the usbmux handshake")]
    HandshakeTimeout,
}

struct Entry {
    device: AttachedDevice,
    link: Arc<Link>,
}

struct Inner {
    devices: Mutex<BTreeMap<u32, Entry>>,
    next_id: AtomicU32,
    events: broadcast::Sender<HubEvent>,
    pair_states: Mutex<HashMap<String, PairState>>,
    pair_events: broadcast::Sender<PairEvent>,
    pairs: Box<dyn PairStore>,
    handshake_timeout: Duration,
}

/// The set of attached devices: the part of `usbmuxd` that is independent of
/// how clients talk to it.
#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

/// USB serial numbers of devices with a 24 digit identifier lack the dash
/// that is part of their UDID.
pub fn udid_from_serial(serial: &str) -> String {
    if serial.len() == 24 && serial.is_ascii() && !serial.contains('-') {
        format!("{}-{}", &serial[..8], &serial[8..])
    } else {
        serial.to_string()
    }
}

/// `/dev/bus/usb/001/005` becomes bus 1, address 5, as `bus << 16 | address`
/// (the location id usbmuxd reports).
pub fn location_id(device_path: &str) -> u32 {
    let mut parts = device_path.rsplit('/');
    let address = parts.next().and_then(|s| s.parse::<u32>().ok());
    let bus = parts.next().and_then(|s| s.parse::<u32>().ok());
    match (bus, address) {
        (Some(bus), Some(address)) => (bus << 16) | address,
        _ => 0,
    }
}

impl Hub {
    pub fn new(pairs: Box<dyn PairStore>) -> Self {
        Self::with_handshake_timeout(pairs, HANDSHAKE_TIMEOUT)
    }

    pub fn with_handshake_timeout(pairs: Box<dyn PairStore>, handshake_timeout: Duration) -> Self {
        Hub {
            inner: Arc::new(Inner {
                devices: Mutex::new(BTreeMap::new()),
                next_id: AtomicU32::new(1),
                events: broadcast::channel(64).0,
                pair_states: Mutex::new(HashMap::new()),
                pair_events: broadcast::channel(64).0,
                pairs,
                handshake_timeout,
            }),
        }
    }

    pub fn pairs(&self) -> &dyn PairStore {
        &*self.inner.pairs
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.inner.events.subscribe()
    }

    pub fn subscribe_pairing(&self) -> broadcast::Receiver<PairEvent> {
        self.inner.pair_events.subscribe()
    }

    /// How far pairing a device got, if it was tried since the app started.
    pub fn pair_state(&self, udid: &str) -> Option<PairState> {
        self.inner.pair_states.lock().unwrap().get(udid).cloned()
    }

    pub fn set_pair_state(&self, udid: &str, state: PairState) {
        self.inner
            .pair_states
            .lock()
            .unwrap()
            .insert(udid.to_string(), state.clone());
        let _ = self.inner.pair_events.send(PairEvent {
            udid: udid.to_string(),
            state,
        });
    }

    /// The pair record of a device. While it is being paired this waits (up
    /// to `timeout`) for the user to trust the computer on the device.
    pub async fn wait_for_pair_record(&self, udid: &str, timeout: Duration) -> Option<Vec<u8>> {
        let deadline = tokio::time::Instant::now() + timeout;
        // Subscribe first so that a state change cannot slip past the check.
        let mut events = self.subscribe_pairing();
        loop {
            if let Some(record) = self.pairs().read(udid) {
                return Some(record);
            }
            if !self
                .pair_state(udid)
                .is_some_and(|state| state.in_progress())
            {
                return None;
            }
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => return None,
                Ok(_) => {}
            }
        }
    }

    pub fn devices(&self) -> Vec<AttachedDevice> {
        let devices = self.inner.devices.lock().unwrap();
        devices.values().map(|entry| entry.device.clone()).collect()
    }

    /// Runs the usbmux handshake on `pipe` and, if it succeeds, makes the
    /// device available under a new id. A device with the same UDID that is
    /// already attached is replaced.
    pub async fn add_device(
        &self,
        pipe: Arc<dyn UsbPipe>,
        info: DeviceInfo,
    ) -> Result<u32, HubError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let (link, ready) = Link::start(pipe, move || {
            if let Some(inner) = weak.upgrade() {
                Hub { inner }.remove_device(id);
            }
        });
        match tokio::time::timeout(self.inner.handshake_timeout, ready).await {
            Ok(Ok(Ok(_version))) => {}
            Ok(Ok(Err(err))) => {
                link.shutdown();
                return Err(HubError::Handshake(err));
            }
            Ok(Err(_)) => {
                link.shutdown();
                return Err(HubError::Handshake(MuxError::DeviceGone));
            }
            Err(_) => {
                link.shutdown();
                return Err(HubError::HandshakeTimeout);
            }
        }

        let device = AttachedDevice { id, info };
        let replaced: Vec<u32> = {
            let devices = self.inner.devices.lock().unwrap();
            devices
                .values()
                .filter(|entry| entry.device.info.udid == device.info.udid)
                .map(|entry| entry.device.id)
                .collect()
        };
        for old in replaced {
            self.remove_device(old);
        }
        info!("usbmux device {} attached as id {id}", device.info.udid);
        self.inner.devices.lock().unwrap().insert(
            id,
            Entry {
                device: device.clone(),
                link,
            },
        );
        let _ = self.inner.events.send(HubEvent::Attached(device));
        Ok(id)
    }

    pub fn remove_device(&self, id: u32) {
        let entry = self.inner.devices.lock().unwrap().remove(&id);
        if let Some(entry) = entry {
            info!("usbmux device {} detached", entry.device.info.udid);
            entry.link.shutdown();
            let _ = self.inner.events.send(HubEvent::Detached(id));
        }
    }

    /// Opens a stream to a port of an attached device.
    pub async fn connect(&self, id: u32, port: u16) -> Result<DeviceStream, HubConnectError> {
        let link = {
            let devices = self.inner.devices.lock().unwrap();
            devices.get(&id).map(|entry| entry.link.clone())
        };
        let Some(link) = link else {
            return Err(HubConnectError::NoSuchDevice);
        };
        link.connect(port).await.map_err(|err| {
            warn!("usbmux connect to device {id} port {port} failed: {err}");
            HubConnectError::Connect(err)
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HubConnectError {
    #[error("no such device")]
    NoSuchDevice,
    #[error(transparent)]
    Connect(ConnectError),
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::usbmux::pair_store::MemoryPairStore;
    use crate::usbmux::testutil::{FakeDevice, Service};

    fn info(udid: &str) -> DeviceInfo {
        DeviceInfo {
            udid: udid.into(),
            product_id: 0x12a8,
            location_id: 1,
            speed: 480_000_000,
        }
    }

    fn hub() -> Hub {
        Hub::new(Box::new(MemoryPairStore::new()))
    }

    #[test]
    fn location_ids_come_from_the_device_path() {
        assert_eq!(location_id("/dev/bus/usb/001/005"), (1 << 16) | 5);
        assert_eq!(location_id("/dev/bus/usb/012/120"), (12 << 16) | 120);
        assert_eq!(location_id("weird"), 0);
    }

    #[test]
    fn udid_gets_its_dash_back() {
        assert_eq!(
            udid_from_serial("00008030001C1D2E0A68802E"),
            "00008030-001C1D2E0A68802E"
        );
        // Old style 40 digit UDIDs and ones that already have a dash stay.
        let old = "a".repeat(40);
        assert_eq!(udid_from_serial(&old), old);
        assert_eq!(
            udid_from_serial("00008030-001C1D2E0A68802E"),
            "00008030-001C1D2E0A68802E"
        );
    }

    #[tokio::test]
    async fn device_is_announced_after_the_handshake() {
        let hub = hub();
        let mut events = hub.subscribe();
        let id = hub
            .add_device(Arc::new(FakeDevice::with_services()), info("AAA-1"))
            .await
            .unwrap();
        assert_eq!(id, 1);
        let device = AttachedDevice {
            id,
            info: info("AAA-1"),
        };
        assert_eq!(hub.devices(), vec![device.clone()]);
        assert_eq!(events.recv().await.unwrap(), HubEvent::Attached(device));
    }

    #[tokio::test]
    async fn echo_roundtrip_through_a_connection() {
        let hub = hub();
        let id = hub
            .add_device(Arc::new(FakeDevice::with_services()), info("AAA-1"))
            .await
            .unwrap();
        let mut stream = hub.connect(id, 7).await.unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    }

    #[tokio::test]
    async fn banner_arrives_without_sending_anything() {
        let hub = hub();
        let id = hub
            .add_device(Arc::new(FakeDevice::with_services()), info("AAA-1"))
            .await
            .unwrap();
        let mut stream = hub.connect(id, 9).await.unwrap();
        let mut buf = vec![0u8; 17];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, b"hello from device");
    }

    #[tokio::test]
    async fn closed_port_is_refused() {
        let hub = hub();
        let id = hub
            .add_device(Arc::new(FakeDevice::with_services()), info("AAA-1"))
            .await
            .unwrap();
        let err = hub.connect(id, 1234).await.err().unwrap();
        assert!(matches!(
            err,
            HubConnectError::Connect(ConnectError::Refused)
        ));
        assert!(matches!(
            hub.connect(99, 7).await.err().unwrap(),
            HubConnectError::NoSuchDevice
        ));
    }

    #[tokio::test]
    async fn device_closing_the_connection_ends_the_stream_after_its_data() {
        let hub = hub();
        let id = hub
            .add_device(Arc::new(FakeDevice::with_services()), info("AAA-1"))
            .await
            .unwrap();
        let mut stream = hub.connect(id, 8).await.unwrap();
        stream.write_all(b"last words").await.unwrap();
        let mut all = Vec::new();
        stream.read_to_end(&mut all).await.unwrap();
        assert_eq!(all, b"last words");
    }

    #[tokio::test]
    async fn large_transfers_survive_flow_control_and_fragmentation() {
        let hub = hub();
        let device = Arc::new(FakeDevice::with_services());
        // Segments larger than one USB read force reassembly, and a small
        // window forces the sender to wait for acknowledgements.
        device.set_window(16);
        device.set_segment(40_000);
        let id = hub.add_device(device, info("AAA-1")).await.unwrap();
        let stream = hub.connect(id, 7).await.unwrap();
        let (mut reader, mut writer) = tokio::io::split(stream);

        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let to_send = data.clone();
        let sender = tokio::spawn(async move {
            writer.write_all(&to_send).await.unwrap();
            writer
        });
        let mut received = vec![0u8; data.len()];
        tokio::time::timeout(Duration::from_secs(20), reader.read_exact(&mut received))
            .await
            .expect("transfer stalled")
            .unwrap();
        assert!(received == data, "echoed data differs");
        sender.await.unwrap();
    }

    #[tokio::test]
    async fn unplugging_detaches_and_fails_streams() {
        let hub = hub();
        let device = Arc::new(FakeDevice::with_services());
        let id = hub.add_device(device.clone(), info("AAA-1")).await.unwrap();
        let mut events = hub.subscribe();
        let mut stream = hub.connect(id, 9).await.unwrap();
        let mut banner = vec![0u8; 17];
        stream.read_exact(&mut banner).await.unwrap();

        device.unplug();
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("no detach event")
            .unwrap();
        assert_eq!(event, HubEvent::Detached(id));
        assert!(hub.devices().is_empty());
        let mut buf = [0u8; 1];
        let err = stream.read(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
        assert!(stream.write_all(b"x").await.is_err());
    }

    #[tokio::test]
    async fn silent_device_fails_the_handshake() {
        let hub = Hub::with_handshake_timeout(
            Box::new(MemoryPairStore::new()),
            Duration::from_millis(200),
        );
        let device = FakeDevice::new();
        device.stay_silent();
        let result = hub.add_device(Arc::new(device), info("AAA-1")).await;
        assert!(matches!(result, Err(HubError::HandshakeTimeout)));
        assert!(hub.devices().is_empty());
    }

    #[tokio::test]
    async fn unsupported_protocol_version_is_reported() {
        let hub = hub();
        let device = FakeDevice::new();
        device.set_major(7);
        let result = hub.add_device(Arc::new(device), info("AAA-1")).await;
        assert!(matches!(
            result,
            Err(HubError::Handshake(MuxError::UnsupportedVersion {
                major: 7,
                ..
            }))
        ));
    }

    #[tokio::test]
    async fn replugging_replaces_the_old_device() {
        let hub = hub();
        let first = hub
            .add_device(Arc::new(FakeDevice::with_services()), info("AAA-1"))
            .await
            .unwrap();
        let mut events = hub.subscribe();
        let second = hub
            .add_device(Arc::new(FakeDevice::with_services()), info("AAA-1"))
            .await
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(hub.devices().len(), 1);
        assert_eq!(hub.devices()[0].id, second);
        assert_eq!(events.recv().await.unwrap(), HubEvent::Detached(first));
        assert!(matches!(
            events.recv().await.unwrap(),
            HubEvent::Attached(device) if device.id == second
        ));
    }

    #[tokio::test]
    async fn dropping_a_stream_resets_the_connection() {
        let hub = hub();
        let device = Arc::new(FakeDevice::with_services());
        let id = hub.add_device(device.clone(), info("AAA-1")).await.unwrap();
        let stream = hub.connect(id, 7).await.unwrap();
        assert_eq!(device.open_connections(), 1);
        drop(stream);
        tokio::time::timeout(Duration::from_secs(5), async {
            while device.open_connections() != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("connection was not reset");
    }

    #[tokio::test]
    async fn services_can_be_added_after_the_fact() {
        let hub = hub();
        let device = Arc::new(FakeDevice::new());
        let id = hub.add_device(device.clone(), info("AAA-1")).await.unwrap();
        assert!(hub.connect(id, 62078).await.is_err());
        device.add_service(62078, Service::Echo);
        assert!(hub.connect(id, 62078).await.is_ok());
    }
}
