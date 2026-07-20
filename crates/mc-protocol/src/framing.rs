use bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};

#[derive(Debug, Default)]
pub struct MessageCodec {
    state: CodecState,
}

#[derive(Debug, Default)]
enum CodecState {
    #[default]
    WaitingForHeader,
    WaitingForPayload {
        length: usize,
    },
}

impl MessageCodec {
    pub fn new() -> Self {
        Self::default()
    }
}

const HEADER_LEN: usize = 9;

impl Decoder for MessageCodec {
    type Item = serde_json::Value;
    type Error = std::io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            match self.state {
                CodecState::WaitingForHeader => {
                    if buf.len() < HEADER_LEN {
                        return Ok(None);
                    }
                    let header = &buf[..HEADER_LEN];
                    if header[HEADER_LEN - 1] != b'\n' {
                        return Err(invalid_data("expected newline after header"));
                    }
                    let hex_str = std::str::from_utf8(&header[..8])
                        .map_err(|e| invalid_data(format!("non-utf8 header: {e}")))?;
                    let length = usize::from_str_radix(hex_str, 16)
                        .map_err(|e| invalid_data(format!("invalid hex length: {e}")))?;
                    if length == 0 {
                        return Err(invalid_data("zero-length frame"));
                    }
                    let _ = buf.split_to(HEADER_LEN);
                    self.state = CodecState::WaitingForPayload { length };
                }
                CodecState::WaitingForPayload { length } => {
                    if buf.len() < length {
                        return Ok(None);
                    }
                    let payload = &buf[..length];
                    if payload[length - 1] != b'\n' {
                        return Err(invalid_data("expected newline after payload"));
                    }
                    #[cfg(debug_assertions)]
                    tracing::debug!(
                        target: "mc_protocol::wire",
                        direction = "inbound",
                        bytes = length - 1,
                        payload = %String::from_utf8_lossy(&payload[..length - 1]),
                        "raw Minecraft debugger payload"
                    );
                    let value: serde_json::Value =
                        serde_json::from_slice(&payload[..length - 1])
                            .map_err(|e| invalid_data(format!("invalid JSON: {e}")))?;
                    let _ = buf.split_to(length);
                    self.state = CodecState::WaitingForHeader;
                    return Ok(Some(value));
                }
            }
        }
    }

    fn decode_eof(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if buf.is_empty() && matches!(self.state, CodecState::WaitingForHeader) {
            return Ok(None);
        }
        match self.decode(buf)? {
            Some(frame) => Ok(Some(frame)),
            None => Err(invalid_data("bytes remaining on stream")),
        }
    }
}

impl Encoder<serde_json::Value> for MessageCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: serde_json::Value, buf: &mut BytesMut) -> Result<(), Self::Error> {
        let json = serde_json::to_vec(&item)
            .map_err(|e| invalid_data(format!("JSON serialization failed: {e}")))?;
        #[cfg(debug_assertions)]
        tracing::debug!(
            target: "mc_protocol::wire",
            direction = "outbound",
            bytes = json.len(),
            payload = %String::from_utf8_lossy(&json),
            "raw Minecraft debugger payload"
        );
        let length = json.len() + 1;
        buf.extend_from_slice(format!("{:08x}\n", length).as_bytes());
        buf.extend_from_slice(&json);
        buf.extend_from_slice(b"\n");
        Ok(())
    }
}

fn invalid_data(msg: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frame(json: &str) -> Vec<u8> {
        let length = json.len() + 1;
        let mut frame = Vec::with_capacity(HEADER_LEN + json.len() + 1);
        frame.extend_from_slice(format!("{:08x}\n", length).as_bytes());
        frame.extend_from_slice(json.as_bytes());
        frame.push(b'\n');
        frame
    }

    #[test]
    fn encode_minimal_value() {
        let value = serde_json::json!({"a":1});
        let mut buf = BytesMut::new();
        let mut codec = MessageCodec::new();
        codec.encode(value, &mut buf).unwrap();
        let expected = b"00000008\n{\"a\":1}\n";
        assert_eq!(buf.as_ref(), expected.as_slice());
    }

    #[test]
    fn encode_resume_message() {
        let value = serde_json::json!({"type":"resume"});
        let mut buf = BytesMut::new();
        let mut codec = MessageCodec::new();
        codec.encode(value, &mut buf).unwrap();
        let expected = b"00000012\n{\"type\":\"resume\"}\n";
        assert_eq!(buf.as_ref(), expected.as_slice());
    }

    #[test]
    fn encode_large_payload_uses_correct_hex_width() {
        let big = "x".repeat(300);
        let value = serde_json::json!({"data": big});
        let mut buf = BytesMut::new();
        let mut codec = MessageCodec::new();
        codec.encode(value.clone(), &mut buf).unwrap();
        let header = &buf[..HEADER_LEN];
        assert_eq!(header[8], b'\n');
        let hex_str = std::str::from_utf8(&header[..8]).unwrap();
        let length = usize::from_str_radix(hex_str, 16).unwrap();
        assert_eq!(buf.len(), HEADER_LEN + length);
        assert_eq!(buf[buf.len() - 1], b'\n');
    }

    #[test]
    fn decode_full_frame() {
        let frame = make_frame(r#"{"type":"resume"}"#);
        let mut buf = BytesMut::from(&frame[..]);
        let mut codec = MessageCodec::new();
        let value = codec.decode(&mut buf).unwrap();
        assert_eq!(value, Some(serde_json::json!({"type":"resume"})));
        assert!(buf.is_empty());
    }

    #[test]
    fn decode_returns_none_for_partial_header() {
        let mut buf = BytesMut::from(&b"00000"[..]);
        let mut codec = MessageCodec::new();
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
    }

    #[test]
    fn decode_returns_none_for_partial_payload() {
        let mut buf = BytesMut::from(&b"00000010\n{\"type\":\"resu"[..]);
        let mut codec = MessageCodec::new();
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
    }

    #[test]
    fn decode_multi_frame_stream() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_frame(r#"{"a":1}"#));
        data.extend_from_slice(&make_frame(r#"{"b":2}"#));
        let mut buf = BytesMut::from(&data[..]);
        let mut codec = MessageCodec::new();
        let v1 = codec.decode(&mut buf).unwrap();
        let v2 = codec.decode(&mut buf).unwrap();
        let v3 = codec.decode(&mut buf).unwrap();
        assert_eq!(v1, Some(serde_json::json!({"a":1})));
        assert_eq!(v2, Some(serde_json::json!({"b":2})));
        assert_eq!(v3, None);
    }

    #[test]
    fn decode_incremental_one_byte_at_a_time() {
        let frame = make_frame(r#"{"hello":"world"}"#);
        let mut codec = MessageCodec::new();
        let mut buf = BytesMut::new();
        let mut result = None;
        for byte in frame {
            buf.extend_from_slice(&[byte]);
            if let Some(v) = codec.decode(&mut buf).unwrap() {
                result = Some(v);
                break;
            }
        }
        assert_eq!(result, Some(serde_json::json!({"hello":"world"})));
    }

    #[test]
    fn round_trip_preserves_value() {
        let original = serde_json::json!({
            "type": "StoppedEvent",
            "reason": "breakpoint",
            "thread": 0
        });
        let mut buf = BytesMut::new();
        let mut encoder = MessageCodec::new();
        encoder.encode(original.clone(), &mut buf).unwrap();
        let mut decoder = MessageCodec::new();
        let decoded = decoder.decode(&mut buf).unwrap();
        assert_eq!(decoded, Some(original));
    }

    #[test]
    fn decode_rejects_missing_newline_after_header() {
        let mut buf = BytesMut::from(&b"00000008X{\"a\":1}\n"[..]);
        let mut codec = MessageCodec::new();
        let err = codec.decode(&mut buf).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn decode_rejects_invalid_hex() {
        let mut buf = BytesMut::from(&b"0000000z\n{\"a\":1}\n"[..]);
        let mut codec = MessageCodec::new();
        let err = codec.decode(&mut buf).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn decode_rejects_zero_length_frame() {
        let mut buf = BytesMut::from(&b"00000000\n\n"[..]);
        let mut codec = MessageCodec::new();
        let err = codec.decode(&mut buf).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn decode_rejects_missing_newline_after_payload() {
        let mut buf = BytesMut::from(&b"00000002{}X"[..]);
        let mut codec = MessageCodec::new();
        let err = codec.decode(&mut buf).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn decode_eof_clean_at_header_boundary() {
        let mut codec = MessageCodec::new();
        let mut buf = BytesMut::new();
        assert_eq!(codec.decode_eof(&mut buf).unwrap(), None);
    }

    #[test]
    fn decode_eof_errors_on_partial_frame() {
        let mut codec = MessageCodec::new();
        let mut buf = BytesMut::from(&b"00000010\n{"[..]);
        let err = codec.decode_eof(&mut buf).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
