//! A scripted iOS device for tests: it speaks the device side of the usbmux
//! protocol over an in-memory [`UsbPipe`].

use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::packet::{
    MuxHeader, PROTO_SETUP, PROTO_TCP, PROTO_VERSION, TcpHeader, USB_MRU, encode_version_body,
    mux_header_len,
    tcp_flags::{ACK, RST, SYN},
};
use super::pipe::UsbPipe;

/// What a port of the fake device does.
#[derive(Clone)]
pub enum Service {
    /// Sends back whatever it receives.
    Echo,
    /// Sends a fixed message right after the connection is established.
    Banner(Vec<u8>),
    /// Echoes, then closes the connection (a RST, like a real device).
    EchoThenClose,
    /// A minimal lockdownd: `GetValue` and `Pair`.
    Lockdown(Arc<Lockdownd>),
}

/// Script and log of a fake lockdownd.
#[derive(Default)]
pub struct Lockdownd {
    /// How many `Pair` requests are answered "pending" before one succeeds.
    pub pending: AtomicU32,
    /// Answer `Pair` with `UserDeniedPairing`.
    pub deny: AtomicBool,
    /// How many `Pair` requests are answered `PasswordProtected` (the device
    /// is locked) before the trust dialog shows.
    pub locked: AtomicU32,
    /// Number of `Pair` requests seen.
    pub pair_requests: AtomicU32,
    /// The `PairRecord` of the last `Pair` request.
    pub last_record: Mutex<Option<plist::Dictionary>>,
}

impl Lockdownd {
    fn respond(&self, request: &plist::Dictionary) -> plist::Dictionary {
        use plist::Value;
        let mut reply = plist::Dictionary::new();
        let error = |reply: &mut plist::Dictionary, name: &str| {
            reply.insert("Error".into(), Value::String(name.into()));
        };
        match request.get("Request").and_then(Value::as_string) {
            Some("GetValue") => match request.get("Key").and_then(Value::as_string) {
                Some("DevicePublicKey") => {
                    let key = include_bytes!("testdata_pub.pem").to_vec();
                    reply.insert("Value".into(), Value::Data(key));
                }
                Some("WiFiAddress") => {
                    reply.insert("Value".into(), Value::String("aa:bb:cc:dd:ee:ff".into()));
                }
                _ => error(&mut reply, "MissingValue"),
            },
            Some("Pair") => {
                self.pair_requests.fetch_add(1, Ordering::SeqCst);
                if let Some(Value::Dictionary(record)) = request.get("PairRecord") {
                    *self.last_record.lock().unwrap() = Some(record.clone());
                }
                let pending = self.pending.load(Ordering::SeqCst);
                let locked = self.locked.load(Ordering::SeqCst);
                if locked > 0 {
                    self.locked.store(locked - 1, Ordering::SeqCst);
                    error(&mut reply, "PasswordProtected");
                } else if self.deny.load(Ordering::SeqCst) {
                    error(&mut reply, "UserDeniedPairing");
                } else if pending > 0 {
                    self.pending.store(pending - 1, Ordering::SeqCst);
                    error(&mut reply, "PairingDialogResponsePending");
                } else {
                    reply.insert("EscrowBag".into(), Value::Data(b"escrow".to_vec()));
                }
            }
            _ => error(&mut reply, "InvalidRequest"),
        }
        reply
    }
}

struct DevConn {
    service: Service,
    /// Next sequence number the device sends.
    seq: u32,
    /// What the device acknowledges: the host's next sequence number.
    ack: u32,
    /// Bytes of a request that is not complete yet (lockdown framing).
    buf: Vec<u8>,
}

struct Inner {
    /// Version the device announces during the handshake.
    major: u32,
    /// Framing the device expects from the host and uses itself.
    framing: u8,
    answer_version: bool,
    services: HashMap<u16, Service>,
    conns: HashMap<(u16, u16), DevConn>,
    to_host: VecDeque<Vec<u8>>,
    from_host: Vec<Vec<u8>>,
    unplugged: bool,
    tx_seq: u16,
    window: u16,
    /// Largest payload in one segment sent to the host.
    segment: usize,
}

pub struct FakeDevice {
    inner: Mutex<Inner>,
    wake: Condvar,
}

impl FakeDevice {
    pub fn new() -> Self {
        FakeDevice {
            inner: Mutex::new(Inner {
                major: 2,
                framing: 1,
                answer_version: true,
                services: HashMap::new(),
                conns: HashMap::new(),
                to_host: VecDeque::new(),
                from_host: Vec::new(),
                unplugged: false,
                tx_seq: 0,
                window: 512,
                segment: 15_000,
            }),
            wake: Condvar::new(),
        }
    }

    /// Standard setup: echo on 7, a banner on 9, echo-then-close on 8.
    pub fn with_services() -> Self {
        let device = Self::new();
        device.add_service(7, Service::Echo);
        device.add_service(8, Service::EchoThenClose);
        device.add_service(9, Service::Banner(b"hello from device".to_vec()));
        device
    }

    pub fn add_service(&self, port: u16, service: Service) {
        self.inner.lock().unwrap().services.insert(port, service);
    }

    pub fn set_major(&self, major: u32) {
        self.inner.lock().unwrap().major = major;
    }

    pub fn set_window(&self, window: u16) {
        self.inner.lock().unwrap().window = window;
    }

    /// Largest payload per segment sent to the host. Anything over one USB
    /// read (16 KiB) arrives split over several transfers.
    pub fn set_segment(&self, bytes: usize) {
        self.inner.lock().unwrap().segment = bytes;
    }

    /// Never answers the version request.
    pub fn stay_silent(&self) {
        self.inner.lock().unwrap().answer_version = false;
    }

    pub fn unplug(&self) {
        self.inner.lock().unwrap().unplugged = true;
        self.wake.notify_all();
    }

    pub fn open_connections(&self) -> usize {
        self.inner.lock().unwrap().conns.len()
    }

    /// Every transfer the host has written so far.
    pub fn received(&self) -> Vec<Vec<u8>> {
        self.inner.lock().unwrap().from_host.clone()
    }
}

impl Inner {
    fn queue(&mut self, protocol: u32, body: &[u8]) {
        let version = self.framing;
        let total = mux_header_len(version) + body.len();
        let mut out = Vec::with_capacity(total);
        MuxHeader {
            protocol,
            length: total as u32,
            tx_seq: self.tx_seq,
            rx_seq: 0,
        }
        .write(version, &mut out);
        out.extend_from_slice(body);
        if version >= 2 {
            self.tx_seq = self.tx_seq.wrapping_add(1);
        }
        self.to_host.push_back(out);
    }

    fn queue_tcp(
        &mut self,
        host_port: u16,
        dev_port: u16,
        seq: u32,
        ack: u32,
        flags: u8,
        payload: &[u8],
    ) {
        let mut body = Vec::new();
        TcpHeader {
            sport: dev_port,
            dport: host_port,
            seq,
            ack,
            flags,
            window: self.window,
        }
        .write(&mut body);
        body.extend_from_slice(payload);
        self.queue(PROTO_TCP, &body);
    }

    fn handle_frame(&mut self, frame: &[u8]) {
        let Ok((header, header_len)) = MuxHeader::parse(frame, self.framing) else {
            return;
        };
        let body = &frame[header_len..];
        match header.protocol {
            PROTO_VERSION => {
                if self.answer_version {
                    // The reply still uses the short framing.
                    let version = encode_version_body(self.major, 0);
                    self.queue(PROTO_VERSION, &version);
                    self.framing = self.major.min(2) as u8;
                }
            }
            PROTO_SETUP => {}
            PROTO_TCP => self.handle_tcp(body),
            _ => {}
        }
    }

    fn handle_tcp(&mut self, body: &[u8]) {
        let Ok((th, len)) = TcpHeader::parse(body) else {
            return;
        };
        let payload = &body[len..];
        let (host_port, dev_port) = (th.sport, th.dport);
        let key = (host_port, dev_port);

        if th.flags == SYN {
            match self.services.get(&dev_port).cloned() {
                Some(service) => {
                    let banner = match &service {
                        Service::Banner(text) => Some(text.clone()),
                        _ => None,
                    };
                    self.conns.insert(
                        key,
                        DevConn {
                            service,
                            seq: 1,
                            ack: th.seq.wrapping_add(1),
                            buf: Vec::new(),
                        },
                    );
                    self.queue_tcp(
                        host_port,
                        dev_port,
                        0,
                        th.seq.wrapping_add(1),
                        SYN | ACK,
                        &[],
                    );
                    if let Some(text) = banner {
                        self.send_data(key, &text);
                    }
                }
                None => self.queue_tcp(
                    host_port,
                    dev_port,
                    0,
                    th.seq.wrapping_add(1),
                    RST | ACK,
                    &[],
                ),
            }
            return;
        }
        if th.flags & RST != 0 {
            self.conns.remove(&key);
            return;
        }
        let Some(conn) = self.conns.get_mut(&key) else {
            return;
        };
        if payload.is_empty() {
            return;
        }
        conn.ack = conn.ack.wrapping_add(payload.len() as u32);
        let service = conn.service.clone();
        match service {
            Service::Echo => self.send_data(key, payload),
            Service::EchoThenClose => {
                self.send_data(key, payload);
                let conn = self.conns.remove(&key).unwrap();
                self.queue_tcp(host_port, dev_port, conn.seq, conn.ack, RST | ACK, &[]);
            }
            Service::Lockdown(script) => {
                let conn = self.conns.get_mut(&key).unwrap();
                conn.buf.extend_from_slice(payload);
                // Requests are a 4 byte big endian length and a plist.
                let mut replies = Vec::new();
                while conn.buf.len() >= 4 {
                    let len = u32::from_be_bytes(conn.buf[..4].try_into().unwrap()) as usize;
                    if conn.buf.len() < 4 + len {
                        break;
                    }
                    let request: plist::Dictionary =
                        plist::from_bytes(&conn.buf[4..4 + len]).expect("bad lockdown request");
                    conn.buf.drain(..4 + len);
                    let mut body = Vec::new();
                    plist::to_writer_xml(&mut body, &script.respond(&request)).unwrap();
                    let mut reply = (body.len() as u32).to_be_bytes().to_vec();
                    reply.extend_from_slice(&body);
                    replies.push(reply);
                }
                if replies.is_empty() {
                    let (seq, ack) = (conn.seq, conn.ack);
                    self.queue_tcp(host_port, dev_port, seq, ack, ACK, &[]);
                }
                for reply in replies {
                    self.send_data(key, &reply);
                }
            }
            Service::Banner(_) => {
                let (seq, ack) = (conn.seq, conn.ack);
                self.queue_tcp(host_port, dev_port, seq, ack, ACK, &[]);
            }
        }
    }

    /// Sends data to the host in segments, acknowledging what was received.
    fn send_data(&mut self, key: (u16, u16), data: &[u8]) {
        let (host_port, dev_port) = key;
        let segment = self.segment;
        for chunk in data.chunks(segment) {
            let conn = self.conns.get_mut(&key).unwrap();
            let (seq, ack) = (conn.seq, conn.ack);
            conn.seq = conn.seq.wrapping_add(chunk.len() as u32);
            self.queue_tcp(host_port, dev_port, seq, ack, ACK, chunk);
        }
    }
}

impl UsbPipe for FakeDevice {
    fn read(&self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let mut inner = self.inner.lock().unwrap();
        if inner.to_host.is_empty() && !inner.unplugged {
            inner = self.wake.wait_timeout(inner, timeout).unwrap().0;
        }
        if inner.unplugged {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let Some(mut frame) = inner.to_host.pop_front() else {
            return Err(io::ErrorKind::TimedOut.into());
        };
        // Like usbfs, hand out at most one read buffer at a time.
        let n = frame.len().min(buf.len()).min(USB_MRU);
        buf[..n].copy_from_slice(&frame[..n]);
        if n < frame.len() {
            let rest = frame.split_off(n);
            inner.to_host.push_front(rest);
        }
        Ok(n)
    }

    fn write(&self, data: &[u8]) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.unplugged {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        inner.from_host.push(data.to_vec());
        inner.handle_frame(data);
        drop(inner);
        self.wake.notify_all();
        Ok(())
    }
}
