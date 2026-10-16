use std::collections::{BTreeMap, VecDeque};

use super::MuxError;
use super::packet::{
    self, MAX_INBOUND_PACKET, MUX_HEADER_V2_LEN, MuxHeader, PROTO_CONTROL, PROTO_SETUP, PROTO_TCP,
    PROTO_VERSION, TCP_HEADER_LEN, TcpHeader, USB_MRU, USB_MTU, mux_header_len,
    tcp_flags::{ACK, PSH, RST, SYN},
};

/// Window we advertise to the device (it is sent right-shifted by 8 bits).
const TX_WINDOW: u32 = 131072;
/// Unread data we are willing to buffer per connection. The device never has
/// more than `TX_WINDOW` unacknowledged bytes in flight and we only
/// acknowledge what the application has read, so this cannot overflow unless
/// the device misbehaves.
const INBUF_CAPACITY: usize = TX_WINDOW as usize;
/// Largest TCP payload that fits in one USB transfer.
const MAX_PAYLOAD: usize = USB_MTU - MUX_HEADER_V2_LEN - TCP_HEADER_LEN;
/// Pending acknowledgements are flushed after this long.
const ACK_TIMEOUT_MS: u64 = 30;
/// Acknowledge immediately once this much consumed data is unacknowledged.
const ACK_THRESHOLD: u32 = 32 * 1024;
/// `rx_seq` value of the first packet after the setup handshake.
const SETUP_RX_SEQ: u16 = 0xFFFF;

/// Identifies a connection. It is the local port used on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// The device sent a RST.
    PeerReset,
    /// The device sent a FIN or an otherwise unexpected segment; we replied
    /// with a RST.
    PeerClosed,
    /// The device sent more data than we are willing to buffer.
    InputOverflow,
    /// The USB device went away.
    DeviceGone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceLevel {
    Error,
    Warning,
    Info,
    Other(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Version negotiation finished; connections can be opened.
    Ready { version: u8, minor: u32 },
    /// A connection requested with [`Session::connect`] is established.
    Connected(ConnId),
    /// A connection request was refused by the device.
    ConnectFailed(ConnId),
    /// New data can be read. Read until [`Session::read`] returns 0; no new
    /// event is sent before the buffer has been drained.
    Readable(ConnId),
    /// A [`Session::write`] that was cut short by flow control can continue.
    Writable(ConnId),
    /// The connection ended. Data received before that can still be read.
    /// The id stays allocated until [`Session::close`] is called.
    Closed(ConnId, CloseReason),
    /// A diagnostic message sent by the device.
    DeviceMessage { level: DeviceLevel, text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Version request sent, no answer yet.
    Init,
    Active,
    Dead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnState {
    /// SYN sent.
    Connecting,
    Connected,
    /// Ended by the peer (or locally by an error); waiting for `close`.
    Closed(CloseReason),
}

#[derive(Debug)]
struct Conn {
    state: ConnState,
    dport: u16,
    /// Next sequence number we send.
    tx_seq: u32,
    /// Next sequence number we expect, i.e. what we acknowledge. Only
    /// advanced as the application reads, which is what gives the device
    /// backpressure.
    tx_ack: u32,
    /// Value of `tx_ack` the device has last been told about.
    tx_acked: u32,
    /// Last acknowledgement number and window received from the device.
    rx_ack: u32,
    rx_win: u32,
    inbuf: VecDeque<u8>,
    readable_notified: bool,
    write_blocked: bool,
    last_ack_ms: u64,
}

impl Conn {
    fn new(dport: u16) -> Self {
        Conn {
            state: ConnState::Connecting,
            dport,
            tx_seq: 0,
            tx_ack: 0,
            tx_acked: 0,
            rx_ack: 0,
            rx_win: 0,
            inbuf: VecDeque::new(),
            readable_notified: false,
            write_blocked: false,
            last_ack_ms: 0,
        }
    }

    /// How many payload bytes the device's window currently lets us send in
    /// one segment.
    fn sendable(&self) -> usize {
        let in_flight = self.tx_seq.wrapping_sub(self.rx_ack);
        (self.rx_win.saturating_sub(in_flight) as usize).min(MAX_PAYLOAD)
    }

    fn ack_pending(&self) -> bool {
        self.state == ConnState::Connected && self.tx_acked != self.tx_ack
    }
}

enum Assembled {
    Buffering,
    Whole(Vec<u8>),
    /// The chunk is a complete packet by itself.
    AsIs,
}

/// The host side of the mux protocol for one USB device.
///
/// The session never blocks and does no I/O. The driver is responsible for:
/// - passing every bulk-in transfer to [`handle_input`](Self::handle_input),
/// - writing every packet from [`poll_transmit`](Self::poll_transmit) as one
///   bulk-out transfer, in order,
/// - draining [`poll_event`](Self::poll_event),
/// - calling [`tick`](Self::tick) when [`next_timeout`](Self::next_timeout)
///   expires.
///
/// Calls that do not take a timestamp use the most recent one passed to
/// `handle_input` or `tick`.
#[derive(Debug)]
pub struct Session {
    state: State,
    /// 0 until negotiated. Version 0 and 1 use the short header.
    version: u8,
    tx_seq: u16,
    rx_seq: u16,
    next_sport: u16,
    conns: BTreeMap<u16, Conn>,
    outbox: VecDeque<Vec<u8>>,
    events: VecDeque<Event>,
    partial: Vec<u8>,
    now_ms: u64,
}

impl Session {
    /// Creates a session and queues the version request.
    pub fn new(now_ms: u64) -> Self {
        let mut session = Session {
            state: State::Init,
            version: 0,
            tx_seq: 0,
            rx_seq: 0,
            next_sport: 1,
            conns: BTreeMap::new(),
            outbox: VecDeque::new(),
            events: VecDeque::new(),
            partial: Vec::new(),
            now_ms,
        };
        let body = packet::encode_version_body(2, 0);
        session
            .queue_packet(PROTO_VERSION, &body)
            .expect("version packet fits in a transfer");
        session
    }

    pub fn is_active(&self) -> bool {
        self.state == State::Active
    }

    /// The negotiated protocol version, 0 before the handshake finished.
    pub fn version(&self) -> u8 {
        self.version
    }

    /// Next packet to write to the bulk-out endpoint.
    pub fn poll_transmit(&mut self) -> Option<Vec<u8>> {
        self.outbox.pop_front()
    }

    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// Feeds one bulk-in transfer into the session.
    ///
    /// An error means the offending packet was dropped; the session stays
    /// usable unless the error is `UnsupportedVersion` or `DeviceGone`.
    pub fn handle_input(&mut self, chunk: &[u8], now_ms: u64) -> Result<(), MuxError> {
        self.now_ms = now_ms;
        if self.state == State::Dead {
            return Err(MuxError::DeviceGone);
        }
        if chunk.is_empty() && self.partial.is_empty() {
            return Ok(()); // zero length packet
        }
        match self.reassemble(chunk)? {
            Assembled::Buffering => Ok(()),
            Assembled::Whole(packet) => self.dispatch(&packet),
            Assembled::AsIs => self.dispatch(chunk),
        }
    }

    /// Flushes acknowledgements that have been pending for too long.
    pub fn tick(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
        if self.state != State::Active {
            return;
        }
        let due: Vec<u16> = self
            .conns
            .iter()
            .filter(|(_, conn)| {
                conn.ack_pending() && now_ms.saturating_sub(conn.last_ack_ms) >= ACK_TIMEOUT_MS
            })
            .map(|(&id, _)| id)
            .collect();
        for id in due {
            // A pure ACK always fits in a transfer.
            let _ = self.send_tcp(id, ACK, &[]);
        }
    }

    /// Milliseconds until [`tick`](Self::tick) needs to be called, if any
    /// timer is running. `Some(0)` means now.
    pub fn next_timeout(&self, now_ms: u64) -> Option<u64> {
        if self.state != State::Active {
            return None;
        }
        self.conns
            .values()
            .filter(|conn| conn.ack_pending())
            .map(|conn| ACK_TIMEOUT_MS.saturating_sub(now_ms.saturating_sub(conn.last_ack_ms)))
            .min()
    }

    /// Opens a connection to a TCP port of the device (e.g. 62078 for
    /// lockdownd). Completion is reported with `Connected` or
    /// `ConnectFailed`.
    pub fn connect(&mut self, port: u16) -> Result<ConnId, MuxError> {
        self.require_active()?;
        let sport = self.alloc_sport()?;
        let mut conn = Conn::new(port);
        conn.last_ack_ms = self.now_ms;
        self.conns.insert(sport, conn);
        if let Err(err) = self.send_tcp(sport, SYN, &[]) {
            self.conns.remove(&sport);
            return Err(err);
        }
        Ok(ConnId(sport))
    }

    /// Sends as much of `data` as the device's window allows and returns how
    /// much was taken. When that is less than `data.len()`, a `Writable`
    /// event follows once the window opens up again.
    pub fn write(&mut self, id: ConnId, data: &[u8]) -> Result<usize, MuxError> {
        self.require_active()?;
        let mut written = 0;
        while written < data.len() {
            let conn = self.conn(id)?;
            Self::require_connected(conn)?;
            let chunk = conn.sendable().min(data.len() - written);
            if chunk == 0 {
                break;
            }
            self.send_tcp(id.0, ACK, &data[written..written + chunk])?;
            let conn = self.conn_mut(id)?;
            conn.tx_seq = conn.tx_seq.wrapping_add(chunk as u32);
            written += chunk;
        }
        let conn = self.conn_mut(id)?;
        Self::require_connected(conn)?;
        conn.write_blocked = written < data.len();
        Ok(written)
    }

    /// Copies buffered data into `buf` and returns the amount. 0 means
    /// nothing is available right now; once the connection has ended and the
    /// buffer is drained this returns the close reason as an error.
    pub fn read(&mut self, id: ConnId, buf: &mut [u8]) -> Result<usize, MuxError> {
        let conn = self.conn_mut(id)?;
        if conn.state == ConnState::Connecting {
            return Err(MuxError::NotConnected);
        }
        let n = buf.len().min(conn.inbuf.len());
        if n == 0 {
            return match conn.state {
                ConnState::Closed(reason) if !buf.is_empty() => {
                    Err(MuxError::ConnectionClosed(reason))
                }
                _ => Ok(0),
            };
        }
        for (dst, src) in buf.iter_mut().zip(conn.inbuf.drain(..n)) {
            *dst = src;
        }
        conn.tx_ack = conn.tx_ack.wrapping_add(n as u32);
        if conn.inbuf.is_empty() {
            conn.readable_notified = false;
        }
        let unacked = conn.tx_ack.wrapping_sub(conn.tx_acked);
        if conn.state == ConnState::Connected && unacked >= ACK_THRESHOLD {
            self.send_tcp(id.0, ACK, &[])?;
        }
        Ok(n)
    }

    /// Number of bytes [`read`](Self::read) can return right now.
    pub fn available(&self, id: ConnId) -> Result<usize, MuxError> {
        Ok(self.conn(id)?.inbuf.len())
    }

    /// Closes a connection and frees its id, sending a RST if the device
    /// still considers it open.
    pub fn close(&mut self, id: ConnId) -> Result<(), MuxError> {
        let state = self.conn(id)?.state;
        if self.state == State::Active
            && matches!(state, ConnState::Connecting | ConnState::Connected)
        {
            let _ = self.send_tcp(id.0, RST, &[]);
        }
        self.conns.remove(&id.0);
        Ok(())
    }

    /// Tells the session the USB device was unplugged. Open connections
    /// report `Closed(DeviceGone)`; nothing further is transmitted.
    pub fn device_removed(&mut self) {
        self.state = State::Dead;
        self.outbox.clear();
        self.partial.clear();
        for (&id, conn) in self.conns.iter_mut() {
            if !matches!(conn.state, ConnState::Closed(_)) {
                conn.state = ConnState::Closed(CloseReason::DeviceGone);
                self.events
                    .push_back(Event::Closed(ConnId(id), CloseReason::DeviceGone));
            }
        }
    }

    fn require_active(&self) -> Result<(), MuxError> {
        match self.state {
            State::Active => Ok(()),
            State::Init => Err(MuxError::NotActive),
            State::Dead => Err(MuxError::DeviceGone),
        }
    }

    fn require_connected(conn: &Conn) -> Result<(), MuxError> {
        match conn.state {
            ConnState::Connected => Ok(()),
            ConnState::Connecting => Err(MuxError::NotConnected),
            ConnState::Closed(reason) => Err(MuxError::ConnectionClosed(reason)),
        }
    }

    fn conn(&self, id: ConnId) -> Result<&Conn, MuxError> {
        self.conns
            .get(&id.0)
            .ok_or(MuxError::UnknownConnection(id.0))
    }

    fn conn_mut(&mut self, id: ConnId) -> Result<&mut Conn, MuxError> {
        self.conns
            .get_mut(&id.0)
            .ok_or(MuxError::UnknownConnection(id.0))
    }

    fn alloc_sport(&mut self) -> Result<u16, MuxError> {
        for _ in 0..u16::MAX {
            let candidate = self.next_sport;
            // Port 0 is never used.
            self.next_sport = if candidate == u16::MAX {
                1
            } else {
                candidate + 1
            };
            if !self.conns.contains_key(&candidate) {
                return Ok(candidate);
            }
        }
        Err(MuxError::NoFreePort)
    }

    /// Wraps `body` in a mux header and queues it for transmission.
    fn queue_packet(&mut self, protocol: u32, body: &[u8]) -> Result<(), MuxError> {
        let total = mux_header_len(self.version) + body.len();
        if total > USB_MTU {
            return Err(MuxError::OutboundTooLarge(total));
        }
        let mut out = Vec::with_capacity(total);
        MuxHeader {
            protocol,
            length: total as u32,
            tx_seq: self.tx_seq,
            rx_seq: self.rx_seq,
        }
        .write(self.version, &mut out);
        out.extend_from_slice(body);
        if self.version >= 2 {
            self.tx_seq = self.tx_seq.wrapping_add(1);
        }
        self.outbox.push_back(out);
        Ok(())
    }

    /// Queues a segment for an existing connection using its current
    /// sequence and acknowledgement numbers. Also counts as an
    /// acknowledgement of everything received so far.
    fn send_tcp(&mut self, sport: u16, flags: u8, data: &[u8]) -> Result<(), MuxError> {
        let conn = self
            .conns
            .get_mut(&sport)
            .ok_or(MuxError::UnknownConnection(sport))?;
        let header = TcpHeader {
            sport,
            dport: conn.dport,
            seq: conn.tx_seq,
            ack: conn.tx_ack,
            flags,
            window: (TX_WINDOW >> 8) as u16,
        };
        conn.tx_acked = conn.tx_ack;
        conn.last_ack_ms = self.now_ms;
        self.queue_tcp(header, data)
    }

    fn queue_tcp(&mut self, header: TcpHeader, data: &[u8]) -> Result<(), MuxError> {
        let mut body = Vec::with_capacity(TCP_HEADER_LEN + data.len());
        header.write(&mut body);
        body.extend_from_slice(data);
        self.queue_packet(PROTO_TCP, &body)
    }

    /// Answers a segment for a connection we do not know with a RST.
    fn send_anon_rst(&mut self, sport: u16, dport: u16, ack: u32) -> Result<(), MuxError> {
        self.queue_tcp(
            TcpHeader {
                sport,
                dport,
                seq: 0,
                ack,
                flags: RST,
                window: 0,
            },
            &[],
        )
    }

    /// Puts together packets the device split over several USB transfers.
    /// Transfers are cut at `USB_MRU`; a shorter one ends the packet.
    fn reassemble(&mut self, chunk: &[u8]) -> Result<Assembled, MuxError> {
        if !self.partial.is_empty() {
            let total = self.partial.len() + chunk.len();
            if total > MAX_INBOUND_PACKET {
                self.partial.clear();
                return Err(MuxError::PacketTooLarge(total));
            }
            self.partial.extend_from_slice(chunk);
            let complete = chunk.len() < USB_MRU
                || packet::peek_length(&self.partial) == Some(self.partial.len());
            return Ok(if complete {
                Assembled::Whole(std::mem::take(&mut self.partial))
            } else {
                Assembled::Buffering
            });
        }
        if chunk.len() == USB_MRU
            && packet::peek_length(chunk).is_some_and(|expected| chunk.len() < expected)
        {
            self.partial.extend_from_slice(chunk);
            return Ok(Assembled::Buffering);
        }
        Ok(Assembled::AsIs)
    }

    fn dispatch(&mut self, packet: &[u8]) -> Result<(), MuxError> {
        let (header, header_len) = MuxHeader::parse(packet, self.version)?;
        if header.length as usize != packet.len() {
            return Err(MuxError::LengthMismatch {
                expected: header.length as usize,
                got: packet.len(),
            });
        }
        if self.version >= 2 {
            // The reference implementation echoes this field back in the
            // rx_seq of every packet it sends, so do the same.
            self.rx_seq = header.rx_seq;
        }
        let body = &packet[header_len..];
        match header.protocol {
            PROTO_VERSION => self.version_input(body),
            PROTO_CONTROL => self.control_input(body),
            PROTO_TCP => self.tcp_input(body),
            other => Err(MuxError::UnknownProtocol(other)),
        }
    }

    fn version_input(&mut self, body: &[u8]) -> Result<(), MuxError> {
        if self.state != State::Init {
            return Err(MuxError::UnexpectedVersion);
        }
        let (major, minor) = packet::decode_version_body(body)?;
        if major != 1 && major != 2 {
            self.state = State::Dead;
            return Err(MuxError::UnsupportedVersion { major, minor });
        }
        self.version = major as u8;
        if self.version >= 2 {
            // The setup packet restarts the sequence numbers.
            self.tx_seq = 0;
            self.rx_seq = SETUP_RX_SEQ;
            self.queue_packet(PROTO_SETUP, &[0x07])?;
        }
        self.state = State::Active;
        self.events.push_back(Event::Ready {
            version: self.version,
            minor,
        });
        Ok(())
    }

    fn control_input(&mut self, body: &[u8]) -> Result<(), MuxError> {
        let (&kind, text) = body.split_first().ok_or(MuxError::BadControl)?;
        let level = match kind {
            3 => DeviceLevel::Error,
            5 => DeviceLevel::Warning,
            7 => DeviceLevel::Info,
            other => DeviceLevel::Other(other),
        };
        self.events.push_back(Event::DeviceMessage {
            level,
            text: String::from_utf8_lossy(text).trim_end().to_string(),
        });
        Ok(())
    }

    fn tcp_input(&mut self, body: &[u8]) -> Result<(), MuxError> {
        let (header, header_len) = TcpHeader::parse(body)?;
        let payload = &body[header_len..];
        if self.state != State::Active {
            return Err(MuxError::NotActive);
        }
        // Our port is the destination of an inbound segment.
        let sport = header.dport;
        let known = self
            .conns
            .get(&sport)
            .is_some_and(|conn| conn.dport == header.sport);
        if !known {
            if header.flags & RST == 0 {
                self.send_anon_rst(sport, header.sport, header.seq)?;
            }
            return Ok(());
        }
        let id = ConnId(sport);
        let conn = self.conn_mut(id)?;
        conn.rx_ack = header.ack;
        conn.rx_win = header.window_bytes();
        match conn.state {
            ConnState::Connecting => self.connecting_input(id, header.flags),
            ConnState::Connected => self.connected_input(id, &header, payload),
            // Already over; stray segments are ignored until `close`.
            ConnState::Closed(_) => Ok(()),
        }
    }

    fn connecting_input(&mut self, id: ConnId, flags: u8) -> Result<(), MuxError> {
        if flags == SYN | ACK {
            let conn = self.conn_mut(id)?;
            conn.tx_seq = conn.tx_seq.wrapping_add(1);
            conn.tx_ack = conn.tx_ack.wrapping_add(1);
            self.send_tcp(id.0, ACK, &[])?;
            self.conn_mut(id)?.state = ConnState::Connected;
            self.events.push_back(Event::Connected(id));
        } else {
            if flags & RST == 0 {
                let _ = self.send_tcp(id.0, RST, &[]);
            }
            self.conns.remove(&id.0);
            self.events.push_back(Event::ConnectFailed(id));
        }
        Ok(())
    }

    fn connected_input(
        &mut self,
        id: ConnId,
        header: &TcpHeader,
        payload: &[u8],
    ) -> Result<(), MuxError> {
        // Anything but a plain ACK ends the connection. PSH is harmless.
        if header.flags & !PSH != ACK {
            let reason = if header.flags & RST != 0 {
                CloseReason::PeerReset
            } else {
                let _ = self.send_tcp(id.0, RST, &[]);
                CloseReason::PeerClosed
            };
            return self.end_connection(id, reason);
        }

        let conn = self.conn_mut(id)?;
        if conn.inbuf.len() + payload.len() > INBUF_CAPACITY {
            let _ = self.send_tcp(id.0, RST, &[]);
            return self.end_connection(id, CloseReason::InputOverflow);
        }
        conn.inbuf.extend(payload);
        if !payload.is_empty() && !conn.readable_notified {
            conn.readable_notified = true;
            self.events.push_back(Event::Readable(id));
        }
        // The device likes to be acknowledged promptly. What this announces
        // is only what the application has consumed so far.
        self.send_tcp(id.0, ACK, &[])?;

        let conn = self.conn_mut(id)?;
        if conn.write_blocked && conn.sendable() > 0 {
            conn.write_blocked = false;
            self.events.push_back(Event::Writable(id));
        }
        Ok(())
    }

    fn end_connection(&mut self, id: ConnId, reason: CloseReason) -> Result<(), MuxError> {
        self.conn_mut(id)?.state = ConnState::Closed(reason);
        self.events.push_back(Event::Closed(id, reason));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usbmux::packet::{decode_version_body, encode_version_body, tcp_flags::FIN};

    const PORT: u16 = 62078;
    /// Raw window field meaning 131072 bytes.
    const WIN: u16 = 512;

    // ---- helpers acting as the device ----

    fn v1_frame(protocol: u32, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        MuxHeader {
            protocol,
            length: (8 + body.len()) as u32,
            tx_seq: 0,
            rx_seq: 0,
        }
        .write(1, &mut out);
        out.extend_from_slice(body);
        out
    }

    fn v2_frame(protocol: u32, rx_seq: u16, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        MuxHeader {
            protocol,
            length: (16 + body.len()) as u32,
            tx_seq: 0,
            rx_seq,
        }
        .write(2, &mut out);
        out.extend_from_slice(body);
        out
    }

    /// A TCP segment from the device to the host port `host_port`.
    fn dev_tcp(
        host_port: u16,
        dev_port: u16,
        seq: u32,
        ack: u32,
        flags: u8,
        window: u16,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut body = Vec::new();
        TcpHeader {
            sport: dev_port,
            dport: host_port,
            seq,
            ack,
            flags,
            window,
        }
        .write(&mut body);
        body.extend_from_slice(payload);
        v2_frame(PROTO_TCP, 0, &body)
    }

    // ---- helpers inspecting what the host sent ----

    fn drain(session: &mut Session) -> Vec<Vec<u8>> {
        std::iter::from_fn(|| session.poll_transmit()).collect()
    }

    fn events(session: &mut Session) -> Vec<Event> {
        std::iter::from_fn(|| session.poll_event()).collect()
    }

    fn parse_tx(frame: &[u8], version: u8) -> (MuxHeader, Vec<u8>) {
        let (header, len) = MuxHeader::parse(frame, version).unwrap();
        assert_eq!(header.length as usize, frame.len());
        (header, frame[len..].to_vec())
    }

    fn tx_tcp(frame: &[u8]) -> (TcpHeader, Vec<u8>) {
        let (_, body) = parse_tx(frame, 2);
        let (header, len) = TcpHeader::parse(&body).unwrap();
        (header, body[len..].to_vec())
    }

    fn active() -> Session {
        let mut session = Session::new(0);
        drain(&mut session);
        let version = encode_version_body(2, 0);
        session
            .handle_input(&v1_frame(PROTO_VERSION, &version), 0)
            .unwrap();
        drain(&mut session);
        assert_eq!(
            events(&mut session),
            vec![Event::Ready {
                version: 2,
                minor: 0
            }]
        );
        session
    }

    fn connected(session: &mut Session, window: u16) -> ConnId {
        let id = session.connect(PORT).unwrap();
        drain(session);
        session
            .handle_input(&dev_tcp(id.0, PORT, 0, 1, SYN | ACK, window, &[]), 0)
            .unwrap();
        drain(session);
        assert_eq!(events(session), vec![Event::Connected(id)]);
        id
    }

    // ---- handshake ----

    #[test]
    fn version_request_bytes() {
        let mut session = Session::new(0);
        assert!(!session.is_active());
        assert_eq!(
            drain(&mut session),
            vec![vec![
                0, 0, 0, 0, 0, 0, 0, 20, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0
            ]]
        );
        assert_eq!(drain(&mut session), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn v2_handshake_sends_setup_packet() {
        let mut session = Session::new(0);
        drain(&mut session);
        let version = encode_version_body(2, 0);
        session
            .handle_input(&v1_frame(PROTO_VERSION, &version), 0)
            .unwrap();
        assert!(session.is_active());
        assert_eq!(session.version(), 2);
        assert_eq!(
            events(&mut session),
            vec![Event::Ready {
                version: 2,
                minor: 0
            }]
        );
        assert_eq!(
            drain(&mut session),
            vec![vec![
                0, 0, 0, 2, 0, 0, 0, 17, 0xfe, 0xed, 0xfa, 0xce, 0, 0, 0xff, 0xff, 7
            ]]
        );
    }

    #[test]
    fn v1_device_keeps_short_headers_and_skips_setup() {
        let mut session = Session::new(0);
        drain(&mut session);
        let version = encode_version_body(1, 0);
        session
            .handle_input(&v1_frame(PROTO_VERSION, &version), 0)
            .unwrap();
        assert_eq!(session.version(), 1);
        assert_eq!(drain(&mut session), Vec::<Vec<u8>>::new());
        assert_eq!(
            events(&mut session),
            vec![Event::Ready {
                version: 1,
                minor: 0
            }]
        );

        let id = session.connect(PORT).unwrap();
        let frames = drain(&mut session);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0][..8], [0, 0, 0, 6, 0, 0, 0, 28]);
        let (header, _) = TcpHeader::parse(&frames[0][8..]).unwrap();
        assert_eq!(
            (header.sport, header.dport, header.flags),
            (id.0, PORT, SYN)
        );

        // Segments from a v1 device are framed with the short header too.
        let mut body = Vec::new();
        TcpHeader {
            sport: PORT,
            dport: id.0,
            seq: 0,
            ack: 1,
            flags: SYN | ACK,
            window: WIN,
        }
        .write(&mut body);
        session
            .handle_input(&v1_frame(PROTO_TCP, &body), 0)
            .unwrap();
        assert_eq!(events(&mut session), vec![Event::Connected(id)]);
    }

    #[test]
    fn unsupported_version_kills_the_session() {
        let mut session = Session::new(0);
        drain(&mut session);
        let version = encode_version_body(3, 1);
        assert_eq!(
            session.handle_input(&v1_frame(PROTO_VERSION, &version), 0),
            Err(MuxError::UnsupportedVersion { major: 3, minor: 1 })
        );
        assert_eq!(session.connect(PORT), Err(MuxError::DeviceGone));
    }

    #[test]
    fn second_version_packet_is_rejected() {
        let mut session = active();
        let version = encode_version_body(2, 0);
        assert_eq!(
            session.handle_input(&v2_frame(PROTO_VERSION, 0, &version), 0),
            Err(MuxError::UnexpectedVersion)
        );
        assert!(session.is_active());
    }

    #[test]
    fn short_version_body_is_rejected() {
        let mut session = Session::new(0);
        assert_eq!(
            session.handle_input(&v1_frame(PROTO_VERSION, &[0; 8]), 0),
            Err(MuxError::Truncated("version body"))
        );
        assert!(!session.is_active());
        assert_eq!(decode_version_body(&[0; 12]), Ok((0, 0)));
    }

    #[test]
    fn connect_requires_an_active_session() {
        let mut session = Session::new(0);
        assert_eq!(session.connect(PORT), Err(MuxError::NotActive));
    }

    // ---- sequence numbers ----

    #[test]
    fn syn_packet_bytes() {
        let mut session = active();
        let id = session.connect(PORT).unwrap();
        assert_eq!(id, ConnId(1));
        assert_eq!(
            drain(&mut session),
            vec![vec![
                0, 0, 0, 6, 0, 0, 0, 36, 0xfe, 0xed, 0xfa, 0xce, 0, 1, 0xff, 0xff, // mux
                0, 1, 0xf2, 0x7e, 0, 0, 0, 0, 0, 0, 0, 0, 0x50, 0x02, 0x02, 0x00, 0, 0, 0,
                0, // tcp
            ]]
        );
    }

    #[test]
    fn mux_sequence_numbers_count_up_and_wrap() {
        let mut session = active();
        // The setup packet was number 0, so the next one is 1.
        session.connect(PORT).unwrap();
        session.connect(PORT).unwrap();
        let seqs: Vec<u16> = drain(&mut session)
            .iter()
            .map(|frame| parse_tx(frame, 2).0.tx_seq)
            .collect();
        assert_eq!(seqs, [1, 2]);

        session.tx_seq = 0xffff;
        session.connect(PORT).unwrap();
        session.connect(PORT).unwrap();
        let seqs: Vec<u16> = drain(&mut session)
            .iter()
            .map(|frame| parse_tx(frame, 2).0.tx_seq)
            .collect();
        assert_eq!(seqs, [0xffff, 0]);
    }

    #[test]
    fn rx_seq_of_the_device_is_echoed() {
        let mut session = active();
        let message = [7, b'h', b'i'];
        session
            .handle_input(&v2_frame(PROTO_CONTROL, 0x1234, &message), 0)
            .unwrap();
        session.connect(PORT).unwrap();
        let frames = drain(&mut session);
        assert_eq!(parse_tx(&frames[0], 2).0.rx_seq, 0x1234);
    }

    // ---- connection lifecycle ----

    #[test]
    fn connect_handshake_and_data_exchange() {
        let mut session = active();
        let id = session.connect(PORT).unwrap();
        drain(&mut session);
        session
            .handle_input(&dev_tcp(id.0, PORT, 0, 1, SYN | ACK, WIN, &[]), 0)
            .unwrap();
        let frames = drain(&mut session);
        assert_eq!(frames.len(), 1);
        let (ack, payload) = tx_tcp(&frames[0]);
        assert_eq!((ack.seq, ack.ack, ack.flags, ack.window), (1, 1, ACK, WIN));
        assert!(payload.is_empty());
        assert_eq!(events(&mut session), vec![Event::Connected(id)]);

        assert_eq!(session.write(id, b"hello"), Ok(5));
        assert_eq!(session.write(id, b"!!"), Ok(2));
        let frames = drain(&mut session);
        let (first, data) = tx_tcp(&frames[0]);
        assert_eq!((first.seq, first.ack, first.flags), (1, 1, ACK));
        assert_eq!(data, b"hello");
        let (second, data) = tx_tcp(&frames[1]);
        assert_eq!(second.seq, 6);
        assert_eq!(data, b"!!");

        session
            .handle_input(&dev_tcp(id.0, PORT, 1, 8, ACK, WIN, b"pong"), 100)
            .unwrap();
        assert_eq!(events(&mut session), vec![Event::Readable(id)]);
        assert_eq!(session.available(id), Ok(4));
        // The prompt acknowledgement does not yet cover unread data.
        let frames = drain(&mut session);
        let (reply, _) = tx_tcp(&frames[0]);
        assert_eq!((reply.seq, reply.ack, reply.flags), (8, 1, ACK));

        let mut buf = [0u8; 16];
        assert_eq!(session.read(id, &mut buf), Ok(4));
        assert_eq!(&buf[..4], b"pong");
        assert_eq!(session.read(id, &mut buf), Ok(0));
    }

    #[test]
    fn consumed_data_is_acknowledged_after_the_timeout() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        session
            .handle_input(&dev_tcp(id.0, PORT, 1, 1, ACK, WIN, b"pong"), 100)
            .unwrap();
        drain(&mut session);
        assert_eq!(session.next_timeout(100), None);

        let mut buf = [0u8; 16];
        session.read(id, &mut buf).unwrap();
        assert!(drain(&mut session).is_empty());
        assert_eq!(session.next_timeout(100), Some(30));
        assert_eq!(session.next_timeout(120), Some(10));
        assert_eq!(session.next_timeout(500), Some(0));

        session.tick(129);
        assert!(drain(&mut session).is_empty());
        session.tick(130);
        let frames = drain(&mut session);
        assert_eq!(frames.len(), 1);
        let (ack, payload) = tx_tcp(&frames[0]);
        assert_eq!((ack.seq, ack.ack, ack.flags), (1, 5, ACK));
        assert!(payload.is_empty());
        assert_eq!(session.next_timeout(130), None);
    }

    #[test]
    fn large_reads_are_acknowledged_immediately() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        let chunk = vec![0xab; 10_000];
        for i in 0..4u32 {
            session
                .handle_input(&dev_tcp(id.0, PORT, 1 + i * 10_000, 1, ACK, WIN, &chunk), 0)
                .unwrap();
        }
        drain(&mut session);
        let mut buf = vec![0u8; 40_000];
        assert_eq!(session.read(id, &mut buf), Ok(40_000));
        let frames = drain(&mut session);
        assert_eq!(frames.len(), 1);
        assert_eq!(tx_tcp(&frames[0]).0.ack, 40_001);
    }

    #[test]
    fn readable_is_reported_once_until_drained() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        session
            .handle_input(&dev_tcp(id.0, PORT, 1, 1, ACK, WIN, b"ab"), 0)
            .unwrap();
        session
            .handle_input(&dev_tcp(id.0, PORT, 3, 1, ACK, WIN, b"cd"), 0)
            .unwrap();
        assert_eq!(events(&mut session), vec![Event::Readable(id)]);

        let mut buf = [0u8; 3];
        assert_eq!(session.read(id, &mut buf), Ok(3));
        assert_eq!(session.read(id, &mut buf), Ok(1));
        session
            .handle_input(&dev_tcp(id.0, PORT, 5, 1, ACK, WIN, b"e"), 0)
            .unwrap();
        assert_eq!(events(&mut session), vec![Event::Readable(id)]);
    }

    #[test]
    fn pure_acks_do_not_raise_readable() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        session
            .handle_input(&dev_tcp(id.0, PORT, 1, 1, ACK, WIN, &[]), 0)
            .unwrap();
        assert_eq!(events(&mut session), vec![]);
    }

    #[test]
    fn psh_flag_is_tolerated() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        session
            .handle_input(&dev_tcp(id.0, PORT, 1, 1, ACK | PSH, WIN, b"x"), 0)
            .unwrap();
        assert_eq!(events(&mut session), vec![Event::Readable(id)]);
    }

    #[test]
    fn refused_connection_is_reported_and_freed() {
        let mut session = active();
        let id = session.connect(PORT).unwrap();
        drain(&mut session);
        session
            .handle_input(&dev_tcp(id.0, PORT, 0, 1, RST | ACK, 0, b"refused\n"), 0)
            .unwrap();
        assert_eq!(events(&mut session), vec![Event::ConnectFailed(id)]);
        // A RST is not answered with another RST.
        assert!(drain(&mut session).is_empty());
        let mut buf = [0u8; 1];
        assert_eq!(
            session.read(id, &mut buf),
            Err(MuxError::UnknownConnection(id.0))
        );
    }

    #[test]
    fn unexpected_reply_to_syn_is_answered_with_rst() {
        let mut session = active();
        let id = session.connect(PORT).unwrap();
        drain(&mut session);
        session
            .handle_input(&dev_tcp(id.0, PORT, 0, 1, ACK, WIN, &[]), 0)
            .unwrap();
        assert_eq!(events(&mut session), vec![Event::ConnectFailed(id)]);
        let frames = drain(&mut session);
        assert_eq!(tx_tcp(&frames[0]).0.flags, RST);
    }

    #[test]
    fn io_before_the_connection_is_established_is_rejected() {
        let mut session = active();
        let id = session.connect(PORT).unwrap();
        let mut buf = [0u8; 1];
        assert_eq!(session.write(id, b"x"), Err(MuxError::NotConnected));
        assert_eq!(session.read(id, &mut buf), Err(MuxError::NotConnected));
        assert_eq!(
            session.write(ConnId(999), b"x"),
            Err(MuxError::UnknownConnection(999))
        );
    }

    #[test]
    fn reset_keeps_received_data_readable() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        session
            .handle_input(&dev_tcp(id.0, PORT, 1, 1, ACK, WIN, b"bye"), 0)
            .unwrap();
        session
            .handle_input(&dev_tcp(id.0, PORT, 4, 1, RST | ACK, WIN, &[]), 0)
            .unwrap();
        assert_eq!(
            events(&mut session),
            vec![
                Event::Readable(id),
                Event::Closed(id, CloseReason::PeerReset)
            ]
        );
        drain(&mut session);

        let mut buf = [0u8; 8];
        assert_eq!(session.read(id, &mut buf), Ok(3));
        assert_eq!(
            session.read(id, &mut buf),
            Err(MuxError::ConnectionClosed(CloseReason::PeerReset))
        );
        assert_eq!(
            session.write(id, b"x"),
            Err(MuxError::ConnectionClosed(CloseReason::PeerReset))
        );
        // The device already reset the connection: no RST from our side.
        session.close(id).unwrap();
        assert!(drain(&mut session).is_empty());
        assert_eq!(session.close(id), Err(MuxError::UnknownConnection(id.0)));
    }

    #[test]
    fn fin_from_the_device_is_answered_with_rst() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        session
            .handle_input(&dev_tcp(id.0, PORT, 1, 1, FIN | ACK, WIN, &[]), 0)
            .unwrap();
        assert_eq!(
            events(&mut session),
            vec![Event::Closed(id, CloseReason::PeerClosed)]
        );
        let frames = drain(&mut session);
        assert_eq!(frames.len(), 1);
        assert_eq!(tx_tcp(&frames[0]).0.flags, RST);
        // Later segments for the dead connection are ignored.
        session
            .handle_input(&dev_tcp(id.0, PORT, 1, 1, ACK, WIN, b"late"), 0)
            .unwrap();
        assert_eq!(session.available(id), Ok(0));
        assert!(events(&mut session).is_empty());
    }

    #[test]
    fn close_sends_rst_and_frees_the_id() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        session.write(id, b"abc").unwrap();
        drain(&mut session);
        session.close(id).unwrap();
        let frames = drain(&mut session);
        let (rst, _) = tx_tcp(&frames[0]);
        assert_eq!((rst.seq, rst.ack, rst.flags), (4, 1, RST));
        assert_eq!(
            session.available(id),
            Err(MuxError::UnknownConnection(id.0))
        );
    }

    #[test]
    fn close_while_connecting_sends_rst() {
        let mut session = active();
        let id = session.connect(PORT).unwrap();
        drain(&mut session);
        session.close(id).unwrap();
        let frames = drain(&mut session);
        assert_eq!(tx_tcp(&frames[0]).0.flags, RST);
    }

    #[test]
    fn segments_for_unknown_connections_get_a_rst() {
        let mut session = active();
        session
            .handle_input(&dev_tcp(7, 1234, 99, 0, ACK, WIN, b"?"), 0)
            .unwrap();
        let frames = drain(&mut session);
        assert_eq!(frames.len(), 1);
        let (rst, _) = tx_tcp(&frames[0]);
        assert_eq!(
            rst,
            TcpHeader {
                sport: 7,
                dport: 1234,
                seq: 0,
                ack: 99,
                flags: RST,
                window: 0
            }
        );
        // ... but never to a RST.
        session
            .handle_input(&dev_tcp(7, 1234, 99, 0, RST, 0, &[]), 0)
            .unwrap();
        assert!(drain(&mut session).is_empty());
    }

    #[test]
    fn segment_from_the_wrong_device_port_is_not_ours() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        session
            .handle_input(&dev_tcp(id.0, PORT + 1, 1, 1, ACK, WIN, b"x"), 0)
            .unwrap();
        assert_eq!(session.available(id), Ok(0));
        let frames = drain(&mut session);
        assert_eq!(tx_tcp(&frames[0]).0.flags, RST);
    }

    #[test]
    fn input_overflow_resets_the_connection() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        let chunk = vec![0u8; 16_384];
        for i in 0..8u32 {
            session
                .handle_input(&dev_tcp(id.0, PORT, 1 + i * 16_384, 1, ACK, WIN, &chunk), 0)
                .unwrap();
        }
        assert_eq!(events(&mut session), vec![Event::Readable(id)]);
        drain(&mut session);
        session
            .handle_input(&dev_tcp(id.0, PORT, 131_073, 1, ACK, WIN, &chunk), 0)
            .unwrap();
        assert_eq!(
            events(&mut session),
            vec![Event::Closed(id, CloseReason::InputOverflow)]
        );
        let frames = drain(&mut session);
        assert_eq!(tx_tcp(&frames[0]).0.flags, RST);
        assert_eq!(session.available(id), Ok(131_072));
    }

    // ---- flow control ----

    #[test]
    fn write_is_limited_by_the_device_window() {
        let mut session = active();
        let id = connected(&mut session, 1); // 256 byte window
        let data = vec![7u8; 1000];
        assert_eq!(session.write(id, &data), Ok(256));
        let frames = drain(&mut session);
        assert_eq!(frames.len(), 1);
        assert_eq!(tx_tcp(&frames[0]).1.len(), 256);

        assert_eq!(session.write(id, &data[256..]), Ok(0));
        assert!(drain(&mut session).is_empty());
        assert!(events(&mut session).is_empty());

        // The device acknowledges everything and opens its window.
        session
            .handle_input(&dev_tcp(id.0, PORT, 1, 257, ACK, 8, &[]), 0)
            .unwrap();
        assert_eq!(events(&mut session), vec![Event::Writable(id)]);
        drain(&mut session);
        assert_eq!(session.write(id, &data[256..]), Ok(744));
        let sent: usize = drain(&mut session)
            .iter()
            .map(|frame| tx_tcp(frame).1.len())
            .sum();
        assert_eq!(sent, 744);

        // A fully accepted write does not leave a Writable pending.
        session
            .handle_input(&dev_tcp(id.0, PORT, 1, 1001, ACK, 8, &[]), 0)
            .unwrap();
        assert!(events(&mut session).is_empty());
    }

    #[test]
    fn partially_acknowledged_window_counts_bytes_in_flight() {
        let mut session = active();
        let id = connected(&mut session, 1);
        assert_eq!(session.write(id, &[0; 400]), Ok(256));
        drain(&mut session);
        // Device acks 100 bytes: 156 still in flight, 100 more fit.
        session
            .handle_input(&dev_tcp(id.0, PORT, 1, 101, ACK, 1, &[]), 0)
            .unwrap();
        assert_eq!(events(&mut session), vec![Event::Writable(id)]);
        assert_eq!(session.write(id, &[0; 400]), Ok(100));
    }

    #[test]
    fn large_writes_are_split_to_fit_one_usb_transfer() {
        let mut session = active();
        let id = connected(&mut session, 0xffff);
        let data: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        assert_eq!(session.write(id, &data), Ok(100_000));
        let frames = drain(&mut session);
        let sizes: Vec<usize> = frames.iter().map(|f| tx_tcp(f).1.len()).collect();
        assert_eq!(sizes, [49_116, 49_116, 1_768]);
        assert!(frames.iter().all(|frame| frame.len() <= USB_MTU));
        let seqs: Vec<u32> = frames.iter().map(|f| tx_tcp(f).0.seq).collect();
        assert_eq!(seqs, [1, 49_117, 98_233]);
        let rejoined: Vec<u8> = frames.iter().flat_map(|f| tx_tcp(f).1).collect();
        assert_eq!(rejoined, data);
    }

    #[test]
    fn empty_write_does_nothing() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        assert_eq!(session.write(id, &[]), Ok(0));
        assert!(drain(&mut session).is_empty());
    }

    // ---- framing ----

    #[test]
    fn packets_split_over_usb_transfers_are_reassembled() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        let payload: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        let frame = dev_tcp(id.0, PORT, 1, 1, ACK, WIN, &payload);
        assert_eq!(frame.len(), 20_036);

        session.handle_input(&frame[..USB_MRU], 0).unwrap();
        assert!(events(&mut session).is_empty());
        session.handle_input(&frame[USB_MRU..], 0).unwrap();
        assert_eq!(events(&mut session), vec![Event::Readable(id)]);
        let mut buf = vec![0u8; 20_000];
        assert_eq!(session.read(id, &mut buf), Ok(20_000));
        assert_eq!(buf, payload);
    }

    #[test]
    fn packet_ending_exactly_on_a_transfer_boundary() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        // 2 * 16384 bytes in total: the second transfer is full sized, so
        // only the length field says the packet is complete.
        let payload = vec![1u8; 2 * USB_MRU - 36];
        let frame = dev_tcp(id.0, PORT, 1, 1, ACK, WIN, &payload);
        assert_eq!(frame.len(), 2 * USB_MRU);
        session.handle_input(&frame[..USB_MRU], 0).unwrap();
        assert!(events(&mut session).is_empty());
        session.handle_input(&frame[USB_MRU..], 0).unwrap();
        assert_eq!(events(&mut session), vec![Event::Readable(id)]);
        assert_eq!(session.available(id), Ok(payload.len()));
    }

    #[test]
    fn full_sized_transfer_that_is_a_whole_packet() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        let payload = vec![2u8; USB_MRU - 36];
        let frame = dev_tcp(id.0, PORT, 1, 1, ACK, WIN, &payload);
        assert_eq!(frame.len(), USB_MRU);
        session.handle_input(&frame, 0).unwrap();
        assert_eq!(events(&mut session), vec![Event::Readable(id)]);
    }

    #[test]
    fn zero_length_transfers_are_ignored() {
        let mut session = active();
        session.handle_input(&[], 0).unwrap();
        assert!(events(&mut session).is_empty());
        assert!(drain(&mut session).is_empty());
    }

    #[test]
    fn oversized_reassembly_is_dropped_and_recovers() {
        let mut session = active();
        let mut first = vec![0u8; USB_MRU];
        first[4..8].copy_from_slice(&100_000u32.to_be_bytes());
        session.handle_input(&first, 0).unwrap();
        let rest = vec![0u8; USB_MRU];
        for _ in 0..3 {
            session.handle_input(&rest, 0).unwrap();
        }
        assert_eq!(
            session.handle_input(&rest, 0),
            Err(MuxError::PacketTooLarge(81_920))
        );
        // The buffer was discarded, so the next packet is parsed afresh.
        session
            .handle_input(&v2_frame(PROTO_CONTROL, 0, &[7, b'o', b'k']), 0)
            .unwrap();
        assert_eq!(
            events(&mut session),
            vec![Event::DeviceMessage {
                level: DeviceLevel::Info,
                text: "ok".into()
            }]
        );
    }

    #[test]
    fn length_mismatch_drops_the_packet_but_not_the_session() {
        let mut session = active();
        let mut frame = v2_frame(PROTO_CONTROL, 0, &[7, b'x']);
        frame[4..8].copy_from_slice(&100u32.to_be_bytes());
        assert_eq!(
            session.handle_input(&frame, 0),
            Err(MuxError::LengthMismatch {
                expected: 100,
                got: 18
            })
        );
        assert!(session.is_active());
        session
            .handle_input(&v2_frame(PROTO_CONTROL, 0, &[7, b'x']), 0)
            .unwrap();
        assert_eq!(events(&mut session).len(), 1);
    }

    #[test]
    fn truncated_and_unknown_packets_are_errors() {
        let mut session = active();
        assert_eq!(
            session.handle_input(&[0; 5], 0),
            Err(MuxError::Truncated("mux header"))
        );
        assert_eq!(
            session.handle_input(&v2_frame(PROTO_TCP, 0, &[0; 10]), 0),
            Err(MuxError::Truncated("tcp header"))
        );
        assert_eq!(
            session.handle_input(&v2_frame(9, 0, &[]), 0),
            Err(MuxError::UnknownProtocol(9))
        );
    }

    #[test]
    fn tcp_segment_before_the_handshake_is_rejected() {
        let mut session = Session::new(0);
        let mut body = Vec::new();
        TcpHeader {
            sport: 1,
            dport: 2,
            seq: 0,
            ack: 0,
            flags: SYN | ACK,
            window: 0,
        }
        .write(&mut body);
        assert_eq!(
            session.handle_input(&v1_frame(PROTO_TCP, &body), 0),
            Err(MuxError::NotActive)
        );
    }

    #[test]
    fn control_messages_become_events() {
        let mut session = active();
        for message in [
            &[3, b'b', b'a', b'd'][..],
            &[5, b'h', b'm', b'm', b'\n'][..],
            &[7, b'o', b'k'][..],
            &[9, b'?'][..],
            &[3][..],
        ] {
            session
                .handle_input(&v2_frame(PROTO_CONTROL, 0, message), 0)
                .unwrap();
        }
        let message = |level, text: &str| Event::DeviceMessage {
            level,
            text: text.into(),
        };
        assert_eq!(
            events(&mut session),
            vec![
                message(DeviceLevel::Error, "bad"),
                message(DeviceLevel::Warning, "hmm"),
                message(DeviceLevel::Info, "ok"),
                message(DeviceLevel::Other(9), "?"),
                message(DeviceLevel::Error, ""),
            ]
        );
        assert_eq!(
            session.handle_input(&v2_frame(PROTO_CONTROL, 0, &[]), 0),
            Err(MuxError::BadControl)
        );
    }

    // ---- ports ----

    #[test]
    fn local_ports_skip_zero_and_ports_in_use() {
        let mut session = active();
        assert_eq!(session.connect(PORT), Ok(ConnId(1)));
        session.next_sport = u16::MAX;
        assert_eq!(session.connect(PORT), Ok(ConnId(u16::MAX)));
        // Wraps around to 1 (taken), so 2 is next.
        assert_eq!(session.connect(PORT), Ok(ConnId(2)));
    }

    #[test]
    fn running_out_of_ports_is_an_error() {
        let mut session = active();
        for port in 1..=u16::MAX {
            session.conns.insert(port, Conn::new(PORT));
        }
        assert_eq!(session.connect(PORT), Err(MuxError::NoFreePort));
    }

    #[test]
    fn independent_connections_do_not_mix() {
        let mut session = active();
        let a = connected(&mut session, WIN);
        let b = connected(&mut session, WIN);
        assert_ne!(a, b);
        session
            .handle_input(&dev_tcp(b.0, PORT, 1, 1, ACK, WIN, b"for b"), 0)
            .unwrap();
        assert_eq!(session.available(a), Ok(0));
        assert_eq!(session.available(b), Ok(5));
    }

    // ---- device removal ----

    #[test]
    fn unplugging_closes_everything_without_traffic() {
        let mut session = active();
        let id = connected(&mut session, WIN);
        session
            .handle_input(&dev_tcp(id.0, PORT, 1, 1, ACK, WIN, b"last"), 0)
            .unwrap();
        drain(&mut session);
        events(&mut session);

        session.device_removed();
        assert_eq!(
            events(&mut session),
            vec![Event::Closed(id, CloseReason::DeviceGone)]
        );
        assert!(drain(&mut session).is_empty());

        let mut buf = [0u8; 8];
        assert_eq!(session.read(id, &mut buf), Ok(4));
        assert_eq!(
            session.read(id, &mut buf),
            Err(MuxError::ConnectionClosed(CloseReason::DeviceGone))
        );
        assert_eq!(session.write(id, b"x"), Err(MuxError::DeviceGone));
        assert_eq!(session.connect(PORT), Err(MuxError::DeviceGone));
        assert_eq!(session.handle_input(&[0; 8], 0), Err(MuxError::DeviceGone));
        session.close(id).unwrap();
        assert!(drain(&mut session).is_empty());
        assert_eq!(session.next_timeout(0), None);
    }
}
