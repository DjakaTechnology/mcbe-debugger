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

#[derive(Debug, Default, Clone)]
pub struct ConnectOptions {
    pub target_module_uuid: Option<String>,
    pub passcode: Option<String>,
}

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
    #[error("passcode required by Minecraft but not provided")]
    PasscodeRequired,
    #[error("multiple plugins available, specify one with target_module_uuid. Available: {available:?}")]
    AmbiguousTarget {
        available: Vec<(String, String)>,
    },
}

impl DebuggeeConnection {
    pub async fn connect(
        host: &str,
        port: u16,
    ) -> Result<(Self, ProtocolHandshake), ConnectionError> {
        Self::connect_with_options(host, port, ConnectOptions::default()).await
    }

    pub async fn connect_with_options(
        host: &str,
        port: u16,
        opts: ConnectOptions,
    ) -> Result<(Self, ProtocolHandshake), ConnectionError> {
        let stream = TcpStream::connect((host, port)).await?;
        Self::handshake(stream, opts).await
    }

    pub async fn listen(port: u16) -> Result<(Self, ProtocolHandshake), ConnectionError> {
        Self::listen_with_options(port, ConnectOptions::default()).await
    }

    pub async fn listen_with_options(
        port: u16,
        opts: ConnectOptions,
    ) -> Result<(Self, ProtocolHandshake), ConnectionError> {
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;
        let (stream, _) = listener.accept().await?;
        drop(listener);
        Self::handshake(stream, opts).await
    }

    async fn handshake(
        stream: TcpStream,
        opts: ConnectOptions,
    ) -> Result<(Self, ProtocolHandshake), ConnectionError> {
        let mut conn = Self {
            stream,
            codec: MessageCodec::new(),
            read_buf: BytesMut::new(),
        };

        let first = conn.recv_event().await?;
        let (version_num, plugins, require_passcode) = match first {
            DebuggeeEvent::Protocol {
                version,
                plugins,
                require_passcode,
            } => (version, plugins, require_passcode),
            other => return Err(ConnectionError::ExpectedProtocolEvent(other)),
        };

        if version_num < ProtocolVersion::MIN_SUPPORTED.as_u8()
            || version_num > ProtocolVersion::CURRENT.as_u8()
        {
            return Err(ConnectionError::UnsupportedVersion {
                server: version_num,
                client: ProtocolVersion::CURRENT.as_u8(),
            });
        }
        let negotiated = ProtocolVersion::try_from(version_num).map_err(|_| {
            ConnectionError::UnsupportedVersion {
                server: version_num,
                client: ProtocolVersion::CURRENT.as_u8(),
            }
        })?;

        if require_passcode && opts.passcode.is_none() {
            return Err(ConnectionError::PasscodeRequired);
        }

        let target_uuid = opts.target_module_uuid.or_else(|| {
            if plugins.len() == 1 {
                Some(plugins[0].module_uuid.clone())
            } else {
                None
            }
        });
        if plugins.len() > 1 && target_uuid.is_none() {
            return Err(ConnectionError::AmbiguousTarget {
                available: plugins
                    .iter()
                    .map(|p| (p.name.clone(), p.module_uuid.clone()))
                    .collect(),
            });
        }

        conn.send_event(&DebuggerEvent::Protocol {
            version: negotiated.as_u8(),
            target_module_uuid: target_uuid,
            passcode: opts.passcode,
        })
        .await?;

        Ok((
            conn,
            ProtocolHandshake {
                version: negotiated,
                plugins,
                require_passcode,
            },
        ))
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

    async fn recv_frame(
        sock: &mut TcpStream,
        codec: &mut MessageCodec,
        buf: &mut BytesMut,
    ) -> serde_json::Value {
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
        assert_eq!(server_response["target_module_uuid"], "abc-123");

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
                    "version": 6,
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
            Err(ConnectionError::UnsupportedVersion { server: 6, client: 9 }) => {}
            other => panic!("expected UnsupportedVersion(6 vs 9), got {other:?}"),
        }

        let _ = tokio::time::timeout(TEST_TIMEOUT, server).await;
    }

    #[tokio::test]
    async fn rejects_when_passcode_required_but_not_provided() {
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
                    "require_passcode": true
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

        assert!(matches!(
            result,
            Err(ConnectionError::PasscodeRequired)
        ));

        let _ = tokio::time::timeout(TEST_TIMEOUT, server).await;
    }

    #[tokio::test]
    async fn rejects_ambiguous_target_with_multiple_plugins() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "ProtocolEvent",
                    "version": 9,
                    "plugins": [
                        {"name": "bp.one", "module_uuid": "uuid-1"},
                        {"name": "bp.two", "module_uuid": "uuid-2"}
                    ],
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
            Err(ConnectionError::AmbiguousTarget { available }) => {
                assert_eq!(available.len(), 2);
                assert_eq!(available[0].0, "bp.one");
                assert_eq!(available[1].0, "bp.two");
            }
            other => panic!("expected AmbiguousTarget, got {other:?}"),
        }

        let _ = tokio::time::timeout(TEST_TIMEOUT, server).await;
    }

    #[tokio::test]
    async fn explicit_target_uuid_overrides_auto_pick() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "ProtocolEvent",
                    "version": 9,
                    "plugins": [
                        {"name": "bp.one", "module_uuid": "uuid-1"},
                        {"name": "bp.two", "module_uuid": "uuid-2"}
                    ],
                    "require_passcode": false
                }),
            )
            .await;

            let mut codec = MessageCodec::new();
            let mut buf = BytesMut::new();
            recv_frame(&mut sock, &mut codec, &mut buf).await
        });

        let opts = ConnectOptions {
            target_module_uuid: Some("uuid-2".into()),
            passcode: None,
        };

        let (_conn, _handshake) = tokio::time::timeout(
            TEST_TIMEOUT,
            DebuggeeConnection::connect_with_options("127.0.0.1", port, opts),
        )
        .await
        .expect("connect timed out")
        .expect("connect failed");

        let server_response = tokio::time::timeout(TEST_TIMEOUT, server)
            .await
            .expect("server timed out")
            .expect("server task panicked");
        assert_eq!(server_response["target_module_uuid"], "uuid-2");
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
