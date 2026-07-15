use bytes::BytesMut;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::{Decoder, Encoder};

use crate::events::{
    encode_debugger_message, parse_debuggee_message, DebuggeeEvent, DebuggerEvent, PluginDetails,
};
use crate::framing::MessageCodec;
use crate::version::ProtocolVersion;

const READ_CHUNK: usize = 4096;

#[derive(Debug)]
pub struct ProtocolHandshake {
    pub version: ProtocolVersion,
    pub plugins: Vec<PluginDetails>,
    pub require_passcode: bool,
}

#[derive(Debug)]
pub struct DebuggeeConnection {
    stream: TcpStream,
    codec: MessageCodec,
    read_buf: BytesMut,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol parse error: {0}")]
    Parse(#[from] crate::events::ParseError),
    #[error("connection closed by peer")]
    Closed,
    #[error("expected ProtocolEvent as first message, got {0:?}")]
    ExpectedProtocolEvent(DebuggeeEvent),
    #[error("unsupported protocol version: server v{server}, supported v{client}")]
    UnsupportedVersion { server: u8, client: u8 },
}

impl DebuggeeConnection {
    pub async fn connect(
        host: &str,
        port: u16,
    ) -> Result<(Self, ProtocolHandshake), ConnectionError> {
        let stream = TcpStream::connect((host, port)).await?;
        Self::handshake(stream).await
    }

    pub async fn listen(port: u16) -> Result<(Self, ProtocolHandshake), ConnectionError> {
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;
        let (stream, _) = listener.accept().await?;
        drop(listener);
        Self::handshake(stream).await
    }

    async fn handshake(
        stream: TcpStream,
    ) -> Result<(Self, ProtocolHandshake), ConnectionError> {
        let mut conn = Self {
            stream,
            codec: MessageCodec::new(),
            read_buf: BytesMut::new(),
        };

        let first = conn.recv_event().await?;
        let handshake = match first {
            DebuggeeEvent::Protocol {
                version,
                plugins,
                require_passcode,
            } => {
                if version != ProtocolVersion::CURRENT.as_u8() {
                    return Err(ConnectionError::UnsupportedVersion {
                        server: version,
                        client: ProtocolVersion::CURRENT.as_u8(),
                    });
                }
                ProtocolHandshake {
                    version: ProtocolVersion::CURRENT,
                    plugins,
                    require_passcode,
                }
            }
            other => return Err(ConnectionError::ExpectedProtocolEvent(other)),
        };

        conn.send_event(&DebuggerEvent::Protocol {
            version: handshake.version.as_u8(),
            target_module_uuid: None,
            passcode: None,
        })
        .await?;

        Ok((conn, handshake))
    }

    pub async fn send_event(
        &mut self,
        event: &DebuggerEvent,
    ) -> Result<(), ConnectionError> {
        let value = encode_debugger_message(event)?;
        let mut buf = BytesMut::new();
        self.codec.encode(value, &mut buf)?;
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    pub async fn recv_event(&mut self) -> Result<DebuggeeEvent, ConnectionError> {
        loop {
            if let Some(value) = self.codec.decode(&mut self.read_buf)? {
                return parse_debuggee_message(value).map_err(ConnectionError::Parse);
            }
            let mut tmp = [0u8; READ_CHUNK];
            let n = self.stream.read(&mut tmp).await?;
            if n == 0 {
                return Err(ConnectionError::Closed);
            }
            self.read_buf.extend_from_slice(&tmp[..n]);
        }
    }

    pub fn peer_addr(&self) -> std::io::Result<SocketAddr> {
        self.stream.peer_addr()
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.stream.local_addr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const TEST_TIMEOUT: Duration = Duration::from_secs(3);

    async fn send_frame(sock: &mut TcpStream, value: serde_json::Value) {
        let mut codec = MessageCodec::new();
        let mut buf = BytesMut::new();
        codec.encode(value, &mut buf).unwrap();
        sock.write_all(&buf).await.unwrap();
    }

    async fn recv_frame(sock: &mut TcpStream, codec: &mut MessageCodec, buf: &mut BytesMut) -> serde_json::Value {
        loop {
            if let Some(value) = codec.decode(buf).unwrap() {
                return value;
            }
            let mut tmp = [0u8; READ_CHUNK];
            let n = sock.read(&mut tmp).await.unwrap();
            if n == 0 {
                panic!("server closed before sending a frame");
            }
            buf.extend_from_slice(&tmp[..n]);
        }
    }

    #[tokio::test]
    async fn connect_completes_handshake() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "ProtocolEvent",
                    "version": 9,
                    "plugins": [{"name": "bp.main", "module_uuid": "abc-123"}],
                    "require_passcode": false
                }),
            )
            .await;

            let mut codec = MessageCodec::new();
            let mut buf = BytesMut::new();
            recv_frame(&mut sock, &mut codec, &mut buf).await
        });

        let (conn, handshake) = tokio::time::timeout(
            TEST_TIMEOUT,
            DebuggeeConnection::connect("127.0.0.1", port),
        )
        .await
        .expect("connect timed out")
        .expect("connect failed");

        assert_eq!(handshake.version, ProtocolVersion::CURRENT);
        assert_eq!(handshake.plugins.len(), 1);
        assert_eq!(handshake.plugins[0].name, "bp.main");
        assert!(!handshake.require_passcode);

        let server_response = tokio::time::timeout(TEST_TIMEOUT, server)
            .await
            .expect("server timed out")
            .expect("server task panicked");
        assert_eq!(server_response["type"], "protocol");
        assert_eq!(server_response["version"], 9);

        drop(conn);
    }

    #[tokio::test]
    async fn listen_completes_handshake() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let server = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "ProtocolEvent",
                    "version": 9,
                    "plugins": [],
                    "require_passcode": false
                }),
            )
            .await;

            let mut codec = MessageCodec::new();
            let mut buf = BytesMut::new();
            recv_frame(&mut sock, &mut codec, &mut buf).await
        });

        let (_conn, handshake) = tokio::time::timeout(
            TEST_TIMEOUT,
            DebuggeeConnection::listen(port),
        )
        .await
        .expect("listen timed out")
        .expect("listen failed");

        assert_eq!(handshake.version, ProtocolVersion::CURRENT);
        assert!(handshake.plugins.is_empty());

        let _ = tokio::time::timeout(TEST_TIMEOUT, server).await;
    }

    #[tokio::test]
    async fn rejects_unsupported_version() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "ProtocolEvent",
                    "version": 7,
                    "plugins": [],
                    "require_passcode": false
                }),
            )
            .await;
            let _ = sock.read(&mut [0u8; 16]).await;
        });

        let result = tokio::time::timeout(
            TEST_TIMEOUT,
            DebuggeeConnection::connect("127.0.0.1", port),
        )
        .await
        .expect("connect timed out");

        match result {
            Err(ConnectionError::UnsupportedVersion { server: 7, client: 9 }) => {}
            other => panic!("expected UnsupportedVersion(7 vs 9), got {other:?}"),
        }

        let _ = tokio::time::timeout(TEST_TIMEOUT, server).await;
    }

    #[tokio::test]
    async fn send_recv_after_handshake() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "ProtocolEvent",
                    "version": 9,
                    "plugins": [],
                    "require_passcode": false
                }),
            )
            .await;

            let mut codec = MessageCodec::new();
            let mut buf = BytesMut::new();
            let _protocol_response = recv_frame(&mut sock, &mut codec, &mut buf).await;

            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "StoppedEvent",
                    "reason": "breakpoint",
                    "thread": 0
                }),
            )
            .await;

            let client_request = recv_frame(&mut sock, &mut codec, &mut buf).await;
            client_request
        });

        let (mut conn, _handshake) = tokio::time::timeout(
            TEST_TIMEOUT,
            DebuggeeConnection::connect("127.0.0.1", port),
        )
        .await
        .expect("connect timed out")
        .expect("connect failed");

        let event = tokio::time::timeout(TEST_TIMEOUT, conn.recv_event())
            .await
            .expect("recv timed out")
            .expect("recv failed");
        match event {
            DebuggeeEvent::Stopped { reason, thread } => {
                assert_eq!(reason, "breakpoint");
                assert_eq!(thread, 0);
            }
            other => panic!("expected Stopped, got {other:?}"),
        }

        conn.send_event(&DebuggerEvent::Resume).await.unwrap();

        let server_seen = tokio::time::timeout(TEST_TIMEOUT, server)
            .await
            .expect("server timed out")
            .expect("server task panicked");
        assert_eq!(server_seen["type"], "resume");
    }
}
