use bytes::BytesMut;
use std::collections::VecDeque;
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

#[derive(Debug, Clone)]
pub struct DebuggeeResponse {
    pub request_seq: u32,
    pub args: Option<serde_json::Value>,
    pub success: bool,
    pub response_message: Option<String>,
}

#[derive(Debug)]
pub struct DebuggeeConnection {
    stream: TcpStream,
    codec: MessageCodec,
    read_buf: BytesMut,
    event_buffer: VecDeque<DebuggeeEvent>,
    next_request_seq: u32,
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
            event_buffer: VecDeque::new(),
            next_request_seq: 1,
        };

        let first = conn.recv_event_inner().await?;
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
        if let Some(event) = self.event_buffer.pop_front() {
            return Ok(event);
        }
        self.recv_event_inner().await
    }

    async fn recv_event_inner(&mut self) -> Result<DebuggeeEvent, ConnectionError> {
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

    pub async fn request(
        &mut self,
        command: impl Into<String>,
        args: serde_json::Value,
    ) -> Result<DebuggeeResponse, ConnectionError> {
        let seq = self.next_request_seq;
        self.next_request_seq = self.next_request_seq.checked_add(1).unwrap_or(1);

        self.send_event(&DebuggerEvent::Request {
            request_seq: seq,
            command: command.into(),
            args,
        })
        .await?;

        loop {
            match self.recv_event_inner().await? {
                DebuggeeEvent::DebuggeeResponse {
                    request_seq,
                    args,
                    success,
                    response_message,
                } if request_seq == seq => {
                    return Ok(DebuggeeResponse {
                        request_seq,
                        args,
                        success: success.unwrap_or(true),
                        response_message,
                    });
                }
                other => {
                    self.event_buffer.push_back(other);
                }
            }
        }
    }

    pub async fn step_next(
        &mut self,
        thread_id: u32,
    ) -> Result<DebuggeeResponse, ConnectionError> {
        self.request("next", serde_json::json!({"threadId": thread_id}))
            .await
    }

    pub async fn step_in(
        &mut self,
        thread_id: u32,
    ) -> Result<DebuggeeResponse, ConnectionError> {
        self.request("stepIn", serde_json::json!({"threadId": thread_id}))
            .await
    }

    pub async fn step_out(
        &mut self,
        thread_id: u32,
    ) -> Result<DebuggeeResponse, ConnectionError> {
        self.request("stepOut", serde_json::json!({"threadId": thread_id}))
            .await
    }

    pub async fn continue_thread(
        &mut self,
        thread_id: u32,
    ) -> Result<DebuggeeResponse, ConnectionError> {
        self.request("continue", serde_json::json!({"threadId": thread_id}))
            .await
    }

    pub async fn pause(
        &mut self,
        thread_id: u32,
    ) -> Result<DebuggeeResponse, ConnectionError> {
        self.request("pause", serde_json::json!({"threadId": thread_id}))
            .await
    }

    pub async fn evaluate(
        &mut self,
        expression: &str,
    ) -> Result<DebuggeeResponse, ConnectionError> {
        self.request("evaluate", serde_json::json!({"expression": expression}))
            .await
    }

    pub async fn stack_trace(
        &mut self,
        thread_id: u32,
    ) -> Result<DebuggeeResponse, ConnectionError> {
        self.request("stackTrace", serde_json::json!({"threadId": thread_id}))
            .await
    }

    pub async fn scopes(
        &mut self,
        frame_id: u32,
    ) -> Result<DebuggeeResponse, ConnectionError> {
        self.request("scopes", serde_json::json!({"frameId": frame_id}))
            .await
    }

    pub async fn variables(
        &mut self,
        variables_reference: u32,
    ) -> Result<DebuggeeResponse, ConnectionError> {
        self.request(
            "variables",
            serde_json::json!({"variablesReference": variables_reference}),
        )
        .await
    }

    pub async fn threads(&mut self) -> Result<DebuggeeResponse, ConnectionError> {
        self.request("threads", serde_json::json!({})).await
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

    async fn server_handshake(
        sock: &mut TcpStream,
        codec: &mut MessageCodec,
        buf: &mut BytesMut,
        protocol_event: serde_json::Value,
    ) -> serde_json::Value {
        send_frame(sock, protocol_event).await;
        recv_frame(sock, codec, buf).await
    }

    fn default_protocol_event() -> serde_json::Value {
        serde_json::json!({
            "type": "ProtocolEvent",
            "version": 9,
            "plugins": [],
            "require_passcode": false
        })
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

        assert!(matches!(result, Err(ConnectionError::PasscodeRequired)));

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
            let mut codec = MessageCodec::new();
            let mut buf = BytesMut::new();
            let _protocol_response =
                server_handshake(&mut sock, &mut codec, &mut buf, default_protocol_event()).await;

            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "StoppedEvent",
                    "reason": "breakpoint",
                    "thread": 0
                }),
            )
            .await;

            recv_frame(&mut sock, &mut codec, &mut buf).await
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

    #[tokio::test]
    async fn request_returns_matching_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut codec = MessageCodec::new();
            let mut buf = BytesMut::new();
            let _protocol_response =
                server_handshake(&mut sock, &mut codec, &mut buf, default_protocol_event()).await;

            let request = recv_frame(&mut sock, &mut codec, &mut buf).await;
            let seq = request["request_seq"].as_u64().unwrap();

            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "debuggee-response",
                    "request_seq": seq,
                    "args": {"result": 42},
                    "success": true
                }),
            )
            .await;

            request
        });

        let (mut conn, _) = tokio::time::timeout(
            TEST_TIMEOUT,
            DebuggeeConnection::connect("127.0.0.1", port),
        )
        .await
        .expect("connect timed out")
        .expect("connect failed");

        let response = tokio::time::timeout(TEST_TIMEOUT, conn.evaluate("1+1"))
            .await
            .expect("request timed out")
            .expect("request failed");

        assert!(response.success);
        assert_eq!(response.args.unwrap()["result"], 42);

        let server_seen_request = tokio::time::timeout(TEST_TIMEOUT, server)
            .await
            .expect("server timed out")
            .expect("server task panicked");
        assert_eq!(server_seen_request["type"], "request");
        assert_eq!(server_seen_request["command"], "evaluate");
        assert_eq!(server_seen_request["args"]["expression"], "1+1");
    }

    #[tokio::test]
    async fn request_buffers_interleaved_events() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut codec = MessageCodec::new();
            let mut buf = BytesMut::new();
            let _protocol_response =
                server_handshake(&mut sock, &mut codec, &mut buf, default_protocol_event()).await;

            let request = recv_frame(&mut sock, &mut codec, &mut buf).await;
            let seq = request["request_seq"].as_u64().unwrap();

            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "event",
                    "event": {
                        "type": "PrintEvent",
                        "message": "interleaved!",
                        "logLevel": 0
                    }
                }),
            )
            .await;
            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "debuggee-response",
                    "request_seq": seq,
                    "success": true
                }),
            )
            .await;
        });

        let (mut conn, _) = tokio::time::timeout(
            TEST_TIMEOUT,
            DebuggeeConnection::connect("127.0.0.1", port),
        )
        .await
        .expect("connect timed out")
        .expect("connect failed");

        let response = tokio::time::timeout(TEST_TIMEOUT, conn.threads())
            .await
            .expect("request timed out")
            .expect("request failed");
        assert!(response.success);

        let buffered = tokio::time::timeout(TEST_TIMEOUT, conn.recv_event())
            .await
            .expect("recv timed out")
            .expect("recv failed");
        match buffered {
            DebuggeeEvent::Print { message, .. } => assert_eq!(message, "interleaved!"),
            other => panic!("expected buffered Print, got {other:?}"),
        }

        let _ = tokio::time::timeout(TEST_TIMEOUT, server).await;
    }

    #[tokio::test]
    async fn request_ignores_responses_with_other_seq() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut codec = MessageCodec::new();
            let mut buf = BytesMut::new();
            let _protocol_response =
                server_handshake(&mut sock, &mut codec, &mut buf, default_protocol_event()).await;

            let request = recv_frame(&mut sock, &mut codec, &mut buf).await;
            let seq = request["request_seq"].as_u64().unwrap();

            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "debuggee-response",
                    "request_seq": 99999,
                    "success": true
                }),
            )
            .await;
            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "debuggee-response",
                    "request_seq": seq,
                    "args": {"ok": true},
                    "success": true
                }),
            )
            .await;
        });

        let (mut conn, _) = tokio::time::timeout(
            TEST_TIMEOUT,
            DebuggeeConnection::connect("127.0.0.1", port),
        )
        .await
        .expect("connect timed out")
        .expect("connect failed");

        let response = tokio::time::timeout(TEST_TIMEOUT, conn.stack_trace(0))
            .await
            .expect("request timed out")
            .expect("request failed");
        assert!(response.success);

        let stray = tokio::time::timeout(TEST_TIMEOUT, conn.recv_event())
            .await
            .expect("recv timed out")
            .expect("recv failed");
        match stray {
            DebuggeeEvent::DebuggeeResponse { request_seq: 99999, .. } => {}
            other => panic!("expected stray DebuggeeResponse(99999), got {other:?}"),
        }

        let _ = tokio::time::timeout(TEST_TIMEOUT, server).await;
    }
}
