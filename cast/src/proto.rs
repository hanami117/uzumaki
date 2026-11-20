//! CASTV2 on the wire: one protobuf message (`CastMessage`), each preceded by its length as a
//! 4-byte big-endian number. The message is small enough to write by hand:
//!
//! ```text
//! 1 protocol_version  varint (0 = CASTV2_1_0)
//! 2 source_id         string
//! 3 destination_id    string
//! 4 namespace         string
//! 5 payload_type      varint (0 = STRING, 1 = BINARY)
//! 6 payload_utf8      string
//! 7 payload_binary    bytes
//! ```

pub const NS_CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
pub const NS_HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
pub const NS_RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
pub const NS_MEDIA: &str = "urn:x-cast:com.google.cast.media";

/// The Default Media Receiver: Google's own player, no registration needed.
pub const DEFAULT_MEDIA_RECEIVER: &str = "CC1AD845";
pub const RECEIVER: &str = "receiver-0";
pub const SENDER: &str = "sender-uzumaki";

/// A frame longer than this is not a Cast message (the device's are a few KB).
pub const MAX_FRAME: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub source: String,
    pub destination: String,
    pub namespace: String,
    /// The JSON text (binary payloads are only used by device authentication, which we skip).
    pub payload: String,
}

impl Message {
    pub fn new(source: &str, destination: &str, namespace: &str, payload: &serde_json::Value) -> Self {
        Message { source: source.into(), destination: destination.into(), namespace: namespace.into(), payload: payload.to_string() }
    }

    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.payload).ok()
    }

    /// The `type` of the JSON payload ("" if none).
    pub fn kind(&self) -> String {
        self.json().and_then(|v| v["type"].as_str().map(String::from)).unwrap_or_default()
    }

    /// The message with its length prefix, ready to write.
    pub fn frame(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(64 + self.payload.len());
        varint_field(&mut body, 1, 0);
        bytes_field(&mut body, 2, self.source.as_bytes());
        bytes_field(&mut body, 3, self.destination.as_bytes());
        bytes_field(&mut body, 4, self.namespace.as_bytes());
        varint_field(&mut body, 5, 0);
        bytes_field(&mut body, 6, self.payload.as_bytes());
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&body);
        out
    }

    /// Parses one message body (without the length prefix). Unknown fields are skipped.
    pub fn parse(mut b: &[u8]) -> Result<Self, String> {
        let mut m = Message { source: String::new(), destination: String::new(), namespace: String::new(), payload: String::new() };
        while !b.is_empty() {
            let key = varint(&mut b)?;
            let (field, wire) = (key >> 3, key & 7);
            match wire {
                0 => {
                    varint(&mut b)?;
                }
                2 => {
                    let len = varint(&mut b)? as usize;
                    if len > b.len() {
                        return Err("truncated field".into());
                    }
                    let (v, rest) = b.split_at(len);
                    b = rest;
                    let text = || String::from_utf8_lossy(v).into_owned();
                    match field {
                        2 => m.source = text(),
                        3 => m.destination = text(),
                        4 => m.namespace = text(),
                        6 => m.payload = text(),
                        _ => {}
                    }
                }
                1 => b = b.get(8..).ok_or("truncated field")?,
                5 => b = b.get(4..).ok_or("truncated field")?,
                _ => return Err(format!("wire type {wire}")),
            }
        }
        Ok(m)
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn varint_field(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

fn bytes_field(out: &mut Vec<u8>, field: u64, v: &[u8]) {
    put_varint(out, (field << 3) | 2);
    put_varint(out, v.len() as u64);
    out.extend_from_slice(v);
}

fn varint(b: &mut &[u8]) -> Result<u64, String> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = b.split_first().ok_or("truncated varint")?;
        *b = rest;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err("varint too long".into())
}

/// Cuts a byte stream into messages: what a read returns may hold part of a frame or several.
#[derive(Default)]
pub struct Deframer {
    buf: Vec<u8>,
}

impl Deframer {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole message, if one is there.
    pub fn take(&mut self) -> Option<Result<Message, String>> {
        if self.buf.len() < 4 {
            return None;
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len > MAX_FRAME {
            self.buf.clear();
            return Some(Err(format!("frame of {len} bytes")));
        }
        if self.buf.len() < 4 + len {
            return None;
        }
        let m = Message::parse(&self.buf[4..4 + len]);
        self.buf.drain(..4 + len);
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_message_goes_through_the_wire_and_back() {
        let m = Message::new(SENDER, RECEIVER, NS_RECEIVER, &json!({"type": "GET_STATUS", "requestId": 1}));
        let f = m.frame();
        assert_eq!(u32::from_be_bytes([f[0], f[1], f[2], f[3]]) as usize, f.len() - 4);
        // Field 1 (varint 0) first, as every sender writes it.
        assert_eq!(&f[4..6], &[0x08, 0x00]);
        let mut d = Deframer::default();
        // Byte by byte: a read may end anywhere.
        for b in &f[..f.len() - 1] {
            d.push(&[*b]);
            assert!(d.take().is_none());
        }
        d.push(&f[f.len() - 1..]);
        let back = d.take().unwrap().unwrap();
        assert_eq!(back, m);
        assert_eq!(back.kind(), "GET_STATUS");
        assert!(d.take().is_none());
    }

    #[test]
    fn two_frames_in_one_read_and_unknown_fields() {
        let a = Message::new("a", "b", NS_HEARTBEAT, &json!({"type": "PING"}));
        let b = Message::new("b", "a", NS_HEARTBEAT, &json!({"type": "PONG"}));
        let mut bytes = a.frame();
        bytes.extend(b.frame());
        let mut d = Deframer::default();
        d.push(&bytes);
        assert_eq!(d.take().unwrap().unwrap().kind(), "PING");
        assert_eq!(d.take().unwrap().unwrap().kind(), "PONG");
        // A binary payload (field 7) and a fixed64 field are skipped.
        let mut body = Vec::new();
        bytes_field(&mut body, 4, b"ns");
        bytes_field(&mut body, 7, &[1, 2, 3]);
        put_varint(&mut body, (9 << 3) | 1);
        body.extend_from_slice(&[0; 8]);
        let m = Message::parse(&body).unwrap();
        assert_eq!((m.namespace.as_str(), m.payload.as_str()), ("ns", ""));
        assert!(Message::parse(&[0x12, 0x05, b'a']).is_err(), "truncated");
    }

    #[test]
    fn a_huge_length_is_refused() {
        let mut d = Deframer::default();
        d.push(&[0x7f, 0xff, 0xff, 0xff, 0]);
        assert!(d.take().unwrap().is_err());
    }
}
