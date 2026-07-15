use bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};

#[derive(Debug, Default)]
pub struct MessageCodec;

impl Decoder for MessageCodec {
    type Item = serde_json::Value;
    type Error = std::io::Error;

    fn decode(
        &mut self,
        _buf: &mut BytesMut,
    ) -> Result<Option<Self::Item>, Self::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "MessageCodec::decode not yet implemented",
        ))
    }
}

impl Encoder<serde_json::Value> for MessageCodec {
    type Error = std::io::Error;

    fn encode(
        &mut self,
        _item: serde_json::Value,
        _buf: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "MessageCodec::encode not yet implemented",
        ))
    }
}
