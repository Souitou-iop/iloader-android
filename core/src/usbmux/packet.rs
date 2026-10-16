//! Wire format of the usbmux protocol spoken over the iOS USB interface
//! (class 0xFF, subclass 0xFE, protocol 2).
//!
//! Every USB bulk transfer carries one mux packet:
//!
//! ```text
//! v1 (handshake only)   v2 (after version negotiation)
//! protocol  u32         protocol  u32
//! length    u32         length    u32   (whole packet, header included)
//!                       magic     u32   (0xfeedface)
//!                       tx_seq    u16
//!                       rx_seq    u16
//! ```
//!
//! All integers are big endian. The body that follows is a version header
//! (12 bytes), a control message, a setup byte, or a 20 byte TCP header plus
//! payload, depending on `protocol`.

use super::MuxError;

/// Size of one inbound USB read; a transfer of exactly this size may be the
/// first fragment of a larger packet.
pub const USB_MRU: usize = 16384;
/// Largest packet we will ever put in one USB transfer.
pub const USB_MTU: usize = 3 * 16384;
/// Upper bound for reassembling a fragmented inbound packet.
pub const MAX_INBOUND_PACKET: usize = 65536;

pub const MUX_MAGIC: u32 = 0xfeed_face;
pub const MUX_HEADER_V1_LEN: usize = 8;
pub const MUX_HEADER_V2_LEN: usize = 16;
pub const TCP_HEADER_LEN: usize = 20;
pub const VERSION_BODY_LEN: usize = 12;

pub const PROTO_VERSION: u32 = 0;
pub const PROTO_CONTROL: u32 = 1;
pub const PROTO_SETUP: u32 = 2;
pub const PROTO_TCP: u32 = 6;

pub mod tcp_flags {
    pub const FIN: u8 = 0x01;
    pub const SYN: u8 = 0x02;
    pub const RST: u8 = 0x04;
    pub const PSH: u8 = 0x08;
    pub const ACK: u8 = 0x10;
}

/// Length of the mux header for a negotiated protocol version. Version 0
/// (not negotiated yet) uses the short v1 framing.
pub fn mux_header_len(version: u8) -> usize {
    if version >= 2 {
        MUX_HEADER_V2_LEN
    } else {
        MUX_HEADER_V1_LEN
    }
}

fn be16(buf: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([buf[at], buf[at + 1]])
}

fn be32(buf: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

/// Reads the `length` field without validating anything else.
pub fn peek_length(buf: &[u8]) -> Option<usize> {
    (buf.len() >= MUX_HEADER_V1_LEN).then(|| be32(buf, 4) as usize)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MuxHeader {
    pub protocol: u32,
    /// Length of the whole packet including this header.
    pub length: u32,
    /// Always 0 when parsed with the v1 framing.
    pub tx_seq: u16,
    pub rx_seq: u16,
}

impl MuxHeader {
    /// Parses a header and returns it with its encoded length. The v2 magic
    /// is deliberately not checked.
    pub fn parse(buf: &[u8], version: u8) -> Result<(Self, usize), MuxError> {
        let len = mux_header_len(version);
        if buf.len() < len {
            return Err(MuxError::Truncated("mux header"));
        }
        let mut header = MuxHeader {
            protocol: be32(buf, 0),
            length: be32(buf, 4),
            tx_seq: 0,
            rx_seq: 0,
        };
        if version >= 2 {
            header.tx_seq = be16(buf, 12);
            header.rx_seq = be16(buf, 14);
        }
        Ok((header, len))
    }

    pub fn write(&self, version: u8, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.protocol.to_be_bytes());
        out.extend_from_slice(&self.length.to_be_bytes());
        if version >= 2 {
            out.extend_from_slice(&MUX_MAGIC.to_be_bytes());
            out.extend_from_slice(&self.tx_seq.to_be_bytes());
            out.extend_from_slice(&self.rx_seq.to_be_bytes());
        }
    }
}

/// The TCP-like header used for stream connections. `window` is the raw
/// 16 bit wire value; the real window is `window << 8` bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpHeader {
    pub sport: u16,
    pub dport: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub window: u16,
}

impl TcpHeader {
    /// Parses a header and returns it with its encoded length (the data
    /// offset field is honoured, so TCP options are skipped).
    pub fn parse(buf: &[u8]) -> Result<(Self, usize), MuxError> {
        if buf.len() < TCP_HEADER_LEN {
            return Err(MuxError::Truncated("tcp header"));
        }
        let header_len = usize::from(buf[12] >> 4) * 4;
        if header_len < TCP_HEADER_LEN || header_len > buf.len() {
            return Err(MuxError::BadTcpHeader);
        }
        Ok((
            TcpHeader {
                sport: be16(buf, 0),
                dport: be16(buf, 2),
                seq: be32(buf, 4),
                ack: be32(buf, 8),
                flags: buf[13],
                window: be16(buf, 14),
            },
            header_len,
        ))
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.sport.to_be_bytes());
        out.extend_from_slice(&self.dport.to_be_bytes());
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.ack.to_be_bytes());
        out.push(((TCP_HEADER_LEN / 4) as u8) << 4);
        out.push(self.flags);
        out.extend_from_slice(&self.window.to_be_bytes());
        out.extend_from_slice(&[0; 4]); // checksum and urgent pointer, unused
    }

    /// Real window size in bytes.
    pub fn window_bytes(&self) -> u32 {
        u32::from(self.window) << 8
    }
}

pub fn encode_version_body(major: u32, minor: u32) -> [u8; VERSION_BODY_LEN] {
    let mut body = [0u8; VERSION_BODY_LEN];
    body[0..4].copy_from_slice(&major.to_be_bytes());
    body[4..8].copy_from_slice(&minor.to_be_bytes());
    body
}

/// Returns `(major, minor)`.
pub fn decode_version_body(body: &[u8]) -> Result<(u32, u32), MuxError> {
    if body.len() < VERSION_BODY_LEN {
        return Err(MuxError::Truncated("version body"));
    }
    Ok((be32(body, 0), be32(body, 4)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_header_roundtrip() {
        let header = MuxHeader {
            protocol: PROTO_VERSION,
            length: 20,
            tx_seq: 0,
            rx_seq: 0,
        };
        let mut out = Vec::new();
        header.write(1, &mut out);
        assert_eq!(out, [0, 0, 0, 0, 0, 0, 0, 20]);
        assert_eq!(MuxHeader::parse(&out, 1).unwrap(), (header, 8));
        // Version 0 (not negotiated yet) also uses the short framing.
        assert_eq!(MuxHeader::parse(&out, 0).unwrap(), (header, 8));
    }

    #[test]
    fn v2_header_roundtrip_and_layout() {
        let header = MuxHeader {
            protocol: PROTO_TCP,
            length: 0x0102_0304,
            tx_seq: 0xaabb,
            rx_seq: 0xccdd,
        };
        let mut out = Vec::new();
        header.write(2, &mut out);
        assert_eq!(
            out,
            [
                0, 0, 0, 6, 1, 2, 3, 4, 0xfe, 0xed, 0xfa, 0xce, 0xaa, 0xbb, 0xcc, 0xdd
            ]
        );
        assert_eq!(MuxHeader::parse(&out, 2).unwrap(), (header, 16));
    }

    #[test]
    fn mux_header_parse_rejects_short_buffers() {
        assert_eq!(
            MuxHeader::parse(&[0; 7], 1),
            Err(MuxError::Truncated("mux header"))
        );
        assert_eq!(
            MuxHeader::parse(&[0; 15], 2),
            Err(MuxError::Truncated("mux header"))
        );
    }

    #[test]
    fn v2_header_ignores_bad_magic() {
        let mut out = vec![0, 0, 0, 6, 0, 0, 0, 16, 1, 2, 3, 4, 0, 1, 0, 2];
        let (header, len) = MuxHeader::parse(&out, 2).unwrap();
        assert_eq!((len, header.tx_seq, header.rx_seq), (16, 1, 2));
        out.truncate(8);
        assert!(MuxHeader::parse(&out, 2).is_err());
    }

    #[test]
    fn tcp_header_roundtrip_and_layout() {
        let header = TcpHeader {
            sport: 1,
            dport: 62078,
            seq: 0x0a0b0c0d,
            ack: 1,
            flags: tcp_flags::SYN | tcp_flags::ACK,
            window: 512,
        };
        let mut out = Vec::new();
        header.write(&mut out);
        assert_eq!(
            out,
            [
                0, 1, 0xf2, 0x7e, 0x0a, 0x0b, 0x0c, 0x0d, 0, 0, 0, 1, 0x50, 0x12, 0x02, 0x00, 0, 0,
                0, 0
            ]
        );
        assert_eq!(TcpHeader::parse(&out).unwrap(), (header, 20));
        assert_eq!(header.window_bytes(), 131072);
    }

    #[test]
    fn tcp_header_honours_data_offset() {
        let header = TcpHeader {
            sport: 5,
            dport: 6,
            seq: 0,
            ack: 0,
            flags: tcp_flags::ACK,
            window: 1,
        };
        let mut out = Vec::new();
        header.write(&mut out);
        out[12] = 6 << 4; // 24 byte header: four bytes of options
        out.extend_from_slice(&[9, 9, 9, 9, 42]);
        let (parsed, len) = TcpHeader::parse(&out).unwrap();
        assert_eq!(parsed, header);
        assert_eq!(len, 24);
        assert_eq!(out[len..], [42]);
    }

    #[test]
    fn tcp_header_rejects_bad_offsets_and_truncation() {
        let mut out = Vec::new();
        TcpHeader {
            sport: 1,
            dport: 2,
            seq: 3,
            ack: 4,
            flags: 0,
            window: 0,
        }
        .write(&mut out);
        assert_eq!(
            TcpHeader::parse(&out[..19]),
            Err(MuxError::Truncated("tcp header"))
        );
        let mut short_offset = out.clone();
        short_offset[12] = 4 << 4;
        assert_eq!(TcpHeader::parse(&short_offset), Err(MuxError::BadTcpHeader));
        let mut past_end = out;
        past_end[12] = 15 << 4;
        assert_eq!(TcpHeader::parse(&past_end), Err(MuxError::BadTcpHeader));
    }

    #[test]
    fn version_body_roundtrip() {
        let body = encode_version_body(2, 0);
        assert_eq!(body, [0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(decode_version_body(&body).unwrap(), (2, 0));
        assert_eq!(
            decode_version_body(&body[..11]),
            Err(MuxError::Truncated("version body"))
        );
    }

    #[test]
    fn peek_length_needs_eight_bytes() {
        assert_eq!(peek_length(&[0; 7]), None);
        assert_eq!(peek_length(&[0, 0, 0, 6, 0, 0, 1, 0]), Some(256));
    }
}
