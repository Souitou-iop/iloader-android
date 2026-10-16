//! Creates the pair record for a device that has none yet.
//!
//! A computer's `usbmuxd` usually already holds the record from an earlier
//! pairing. Android starts empty, so the muxer pairs with the device itself
//! (lockdown `Pair`) as soon as it is attached. The device asks its user to
//! trust the computer; until they answer it keeps replying "pending", which
//! is polled here.

use std::time::Duration;

use idevice::services::lockdown::LockdownClient;
use idevice::{Idevice, IdeviceError};
use serde::Serialize;
use tracing::{debug, info, warn};

use super::hub::{AttachedDevice, Hub};

/// Pause between attempts while the trust dialog is open.
const POLL_INTERVAL: Duration = if cfg!(test) {
    Duration::from_millis(30)
} else {
    Duration::from_secs(2)
};
/// Give up when nobody answers the dialog for this long.
const TRUST_TIMEOUT: Duration = Duration::from_secs(300);
/// Failures that may be transient (lockdownd still starting, a hiccup).
const MAX_ERRORS: u32 = 5;
const HOST_NAME: &str = "iloader";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "message", rename_all = "camelCase")]
pub enum PairState {
    /// Talking to lockdownd.
    Pairing,
    /// The device shows "Trust This Computer?" (or must be unlocked).
    WaitingForTrust,
    Paired,
    /// The user tapped "Don't Trust".
    Denied,
    Failed(String),
}

impl PairState {
    pub fn in_progress(&self) -> bool {
        matches!(self, PairState::Pairing | PairState::WaitingForTrust)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairEvent {
    pub udid: String,
    #[serde(flatten)]
    pub state: PairState,
}

/// Starts pairing `device` in the background unless a record exists already.
/// The state is set before returning, so a `ReadPairRecord` that comes right
/// after the attach already waits instead of failing.
pub fn spawn(hub: &Hub, device: AttachedDevice) {
    let udid = &device.info.udid;
    if hub.pairs().read(udid).is_some() {
        hub.set_pair_state(udid, PairState::Paired);
        return;
    }
    hub.set_pair_state(udid, PairState::Pairing);
    let hub = hub.clone();
    tokio::spawn(async move {
        if let Some(state) = run(&hub, &device).await {
            hub.set_pair_state(&device.info.udid, state);
        }
    });
}

enum Attempt {
    Paired(Vec<u8>),
    Pending,
    Denied,
    Retry(String),
    Fatal(String),
}

/// `None` when the device went away meanwhile; nobody is interested then.
async fn run(hub: &Hub, device: &AttachedDevice) -> Option<PairState> {
    let udid = &device.info.udid;
    let host_id = uuid::Uuid::new_v4().to_string().to_uppercase();
    let started = tokio::time::Instant::now();
    let mut errors = 0;
    loop {
        if !hub.devices().iter().any(|d| d.id == device.id) {
            return None;
        }
        match attempt(hub, device, &host_id).await {
            Attempt::Paired(record) => {
                return Some(match hub.pairs().save(udid, &record) {
                    Ok(()) => {
                        info!("paired with {udid}");
                        PairState::Paired
                    }
                    Err(err) => PairState::Failed(format!("could not save the pair record: {err}")),
                });
            }
            Attempt::Pending => {
                if hub.pair_state(udid) != Some(PairState::WaitingForTrust) {
                    hub.set_pair_state(udid, PairState::WaitingForTrust);
                }
                if started.elapsed() > TRUST_TIMEOUT {
                    return Some(PairState::Failed("nobody trusted this computer".into()));
                }
            }
            Attempt::Denied => return Some(PairState::Denied),
            Attempt::Fatal(message) => return Some(PairState::Failed(message)),
            Attempt::Retry(message) => {
                errors += 1;
                debug!("pairing with {udid}: {message} ({errors}/{MAX_ERRORS})");
                if errors >= MAX_ERRORS {
                    return Some(PairState::Failed(message));
                }
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One `Pair` request on a fresh lockdown connection.
async fn attempt(hub: &Hub, device: &AttachedDevice, host_id: &str) -> Attempt {
    let stream = match hub.connect(device.id, LockdownClient::LOCKDOWND_PORT).await {
        Ok(stream) => stream,
        Err(err) => return Attempt::Retry(err.to_string()),
    };
    let mut lockdown = LockdownClient::new(Idevice::new(Box::new(stream), HOST_NAME));
    let buid = hub.pairs().buid();
    match lockdown.pair_once(host_id, buid, Some(HOST_NAME)).await {
        Ok(file) => match file.serialize() {
            Ok(record) => Attempt::Paired(record),
            Err(err) => Attempt::Fatal(err.to_string()),
        },
        Err(IdeviceError::PairingDialogResponsePending) => Attempt::Pending,
        Err(IdeviceError::UserDeniedPairing) => Attempt::Denied,
        Err(IdeviceError::PasswordProtected) => {
            Attempt::Fatal("the device is locked with a passcode: unlock it and replug".into())
        }
        Err(err) => {
            warn!("lockdown pair failed: {err}");
            Attempt::Retry(err.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use idevice::pairing_file::PairingFile;

    use super::*;
    use crate::usbmux::hub::DeviceInfo;
    use crate::usbmux::pair_store::MemoryPairStore;
    use crate::usbmux::testutil::{FakeDevice, Lockdownd, Service};

    const UDID: &str = "00008030-001C1D2E0A68802E";

    async fn attach(hub: &Hub, lockdownd: Option<Arc<Lockdownd>>) -> AttachedDevice {
        let device = FakeDevice::new();
        if let Some(lockdownd) = lockdownd {
            device.add_service(62078, Service::Lockdown(lockdownd));
        }
        let info = DeviceInfo {
            udid: UDID.into(),
            product_id: 0x12a8,
            location_id: 1,
            speed: 480_000_000,
        };
        let id = hub
            .add_device(Arc::new(device), info.clone())
            .await
            .unwrap();
        AttachedDevice { id, info }
    }

    fn hub() -> Hub {
        Hub::new(Box::new(MemoryPairStore::new()))
    }

    /// Waits until the pairing of the test device is over.
    async fn finished(hub: &Hub) -> PairState {
        let mut events = hub.subscribe_pairing();
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                if let Some(state) = hub.pair_state(UDID).filter(|s| !s.in_progress()) {
                    return state;
                }
                let _ = events.recv().await;
            }
        })
        .await
        .expect("pairing did not finish")
    }

    #[tokio::test]
    async fn pairs_once_the_user_trusts_the_computer() {
        let hub = hub();
        let lockdownd = Arc::new(Lockdownd::default());
        lockdownd.pending.store(2, Ordering::SeqCst);
        let device = attach(&hub, Some(lockdownd.clone())).await;
        let mut events = hub.subscribe_pairing();

        spawn(&hub, device);
        assert_eq!(hub.pair_state(UDID), Some(PairState::Pairing));
        assert_eq!(finished(&hub).await, PairState::Paired);

        let mut seen = Vec::new();
        while let Ok(event) = events.try_recv() {
            seen.push(event.state);
        }
        assert_eq!(
            seen,
            vec![
                PairState::Pairing,
                PairState::WaitingForTrust,
                PairState::Paired
            ]
        );
        assert_eq!(lockdownd.pair_requests.load(Ordering::SeqCst), 3);

        let record = PairingFile::from_bytes(&hub.pairs().read(UDID).unwrap()).unwrap();
        assert_eq!(record.system_buid, hub.pairs().buid());
        assert_eq!(record.escrow_bag.as_deref(), Some(&b"escrow"[..]));
        // The device got the same ids the record carries.
        let sent = lockdownd.last_record.lock().unwrap().clone().unwrap();
        assert_eq!(
            sent.get("HostID").and_then(|v| v.as_string()),
            Some(record.host_id.as_str())
        );
        assert_eq!(
            sent.get("SystemBUID").and_then(|v| v.as_string()),
            Some(record.system_buid.as_str())
        );
    }

    #[tokio::test]
    async fn an_existing_record_is_left_alone() {
        let hub = hub();
        hub.pairs().save(UDID, b"old").unwrap();
        let lockdownd = Arc::new(Lockdownd::default());
        let device = attach(&hub, Some(lockdownd.clone())).await;
        spawn(&hub, device);
        assert_eq!(hub.pair_state(UDID), Some(PairState::Paired));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(lockdownd.pair_requests.load(Ordering::SeqCst), 0);
        assert_eq!(hub.pairs().read(UDID).as_deref(), Some(&b"old"[..]));
    }

    #[tokio::test]
    async fn declining_the_dialog_is_reported() {
        let hub = hub();
        let lockdownd = Arc::new(Lockdownd::default());
        lockdownd.deny.store(true, Ordering::SeqCst);
        let device = attach(&hub, Some(lockdownd.clone())).await;
        spawn(&hub, device);
        assert_eq!(finished(&hub).await, PairState::Denied);
        assert_eq!(lockdownd.pair_requests.load(Ordering::SeqCst), 1);
        assert!(hub.pairs().read(UDID).is_none());
    }

    #[tokio::test]
    async fn a_passcode_protected_device_fails_without_retrying() {
        let hub = hub();
        let lockdownd = Arc::new(Lockdownd::default());
        lockdownd.passcode.store(true, Ordering::SeqCst);
        let device = attach(&hub, Some(lockdownd.clone())).await;
        spawn(&hub, device);
        assert!(matches!(finished(&hub).await, PairState::Failed(_)));
        assert_eq!(lockdownd.pair_requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_device_without_lockdownd_fails_after_a_few_tries() {
        let hub = hub();
        let device = attach(&hub, None).await;
        spawn(&hub, device);
        assert!(matches!(finished(&hub).await, PairState::Failed(_)));
        assert!(hub.pairs().read(UDID).is_none());
    }

    #[tokio::test]
    async fn pairing_stops_when_the_device_is_unplugged() {
        let hub = hub();
        let lockdownd = Arc::new(Lockdownd::default());
        lockdownd.pending.store(u32::MAX, Ordering::SeqCst);
        let device = attach(&hub, Some(lockdownd.clone())).await;
        let id = device.id;
        spawn(&hub, device);
        // Every attempt generates a key pair, which is slow in a debug build.
        while hub.pair_state(UDID) != Some(PairState::WaitingForTrust) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        hub.remove_device(id);
        // Let an attempt that was under way finish.
        tokio::time::sleep(Duration::from_secs(3)).await;
        let requests = lockdownd.pair_requests.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(lockdownd.pair_requests.load(Ordering::SeqCst), requests);
    }

    #[tokio::test]
    async fn reading_the_record_waits_for_the_trust_dialog() {
        let hub = hub();
        let lockdownd = Arc::new(Lockdownd::default());
        lockdownd.pending.store(1, Ordering::SeqCst);
        let device = attach(&hub, Some(lockdownd)).await;
        spawn(&hub, device);
        let record = hub
            .wait_for_pair_record(UDID, Duration::from_secs(60))
            .await;
        assert!(record.is_some());
    }

    #[tokio::test]
    async fn reading_the_record_does_not_wait_when_nothing_is_pairing() {
        let hub = hub();
        let started = tokio::time::Instant::now();
        assert!(
            hub.wait_for_pair_record(UDID, Duration::from_secs(10))
                .await
                .is_none()
        );
        hub.set_pair_state(UDID, PairState::Denied);
        assert!(
            hub.wait_for_pair_record(UDID, Duration::from_secs(10))
                .await
                .is_none()
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn reading_the_record_gives_up_after_the_timeout() {
        let hub = hub();
        hub.set_pair_state(UDID, PairState::WaitingForTrust);
        assert!(
            hub.wait_for_pair_record(UDID, Duration::from_millis(100))
                .await
                .is_none()
        );
    }

    #[test]
    fn events_serialize_for_the_frontend() {
        let event = PairEvent {
            udid: "u".into(),
            state: PairState::Failed("boom".into()),
        };
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"udid":"u","state":"failed","message":"boom"}"#
        );
        let event = PairEvent {
            udid: "u".into(),
            state: PairState::WaitingForTrust,
        };
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"udid":"u","state":"waitingForTrust"}"#
        );
    }
}
