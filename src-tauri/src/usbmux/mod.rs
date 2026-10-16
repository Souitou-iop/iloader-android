//! A sans-IO implementation of the host side of the usbmux USB protocol.
//!
//! On a computer, `usbmuxd` multiplexes TCP-like streams over the USB
//! interface of an iOS device. Android has no `usbmuxd`, so the same job has
//! to be done inside the app. This module only contains the protocol: it
//! never touches USB itself. A transport feeds it every bulk-in transfer and
//! writes out every packet it queues:
//!
//! ```text
//! bulk-in  -> Session::handle_input(chunk, now_ms)
//! Session::poll_transmit() -> bulk-out   (one packet per transfer)
//! Session::poll_event()    -> Ready / Connected / Readable / ...
//! Session::tick(now_ms)    -> call on a timer, see Session::next_timeout
//! ```
//!
//! `link` drives a session with a real USB pipe and `hub` keeps track of the
//! attached devices. `server` makes them look like `usbmuxd` to the rest of
//! the app, and `android` connects it all to the Kotlin USB plugin.
//!
//! The wire format and state machine follow the behaviour of the reference
//! `usbmuxd` daemon, which is the only implementation real devices are known
//! to work with.

// Only the Android build wires this in; elsewhere it is exercised by tests.
#![allow(dead_code)]

#[cfg(target_os = "android")]
pub mod android;
pub mod hub;
mod link;
pub mod packet;
pub mod pair_store;
pub mod pairing;
pub mod pipe;
pub mod server;
mod session;
#[cfg(test)]
mod testutil;
pub mod usbfs;

use std::sync::OnceLock;

use idevice::usbmuxd::UsbmuxdAddr;

static ADDR_OVERRIDE: OnceLock<UsbmuxdAddr> = OnceLock::new();

/// Where the rest of the app finds a usbmuxd when it is not the system's:
/// on Android, the muxer running inside this process.
pub fn addr_override() -> Option<UsbmuxdAddr> {
    ADDR_OVERRIDE.get().cloned()
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn set_addr_override(addr: UsbmuxdAddr) {
    let _ = ADDR_OVERRIDE.set(addr);
}

#[allow(unused_imports)]
pub use session::{CloseReason, ConnId, DeviceLevel, Event, Session};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MuxError {
    #[error("truncated {0}")]
    Truncated(&'static str),
    #[error("invalid tcp header")]
    BadTcpHeader,
    #[error("packet length mismatch (header says {expected}, got {got})")]
    LengthMismatch { expected: usize, got: usize },
    #[error("inbound packet too large ({0} bytes)")]
    PacketTooLarge(usize),
    #[error("outbound packet too large ({0} bytes)")]
    OutboundTooLarge(usize),
    #[error("unknown protocol {0}")]
    UnknownProtocol(u32),
    #[error("unsupported protocol version {major}.{minor}")]
    UnsupportedVersion { major: u32, minor: u32 },
    #[error("version packet received after negotiation")]
    UnexpectedVersion,
    #[error("malformed control packet")]
    BadControl,
    #[error("session is not active")]
    NotActive,
    #[error("the device is gone")]
    DeviceGone,
    #[error("no free local port")]
    NoFreePort,
    #[error("unknown connection {0}")]
    UnknownConnection(u16),
    #[error("connection is not established")]
    NotConnected,
    #[error("connection closed: {0:?}")]
    ConnectionClosed(CloseReason),
}
