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
    version: ProtocolVersion,
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
    #[error("selected target_module_uuid \"{selected}\" not found in available plugins: {available:?}")]
    TargetNotFound {
        selected: String,
        available: Vec<(String, String)>,
    },
    #[error("request timed out after {0:?} (MC may not respond to this command type)")]
    RequestTimeout(std::time::Duration),
}

/// A partially handshaken connection that has received the initial `ProtocolEvent`
/// from Minecraft but has not yet validated passcode/target or sent the debugger's
/// protocol response. Call [`complete`](PendingConnection::complete) to finish the handshake.
#[derive(Debug)]
pub struct PendingConnection {
    conn: DebuggeeConnection,
    plugins: Vec<PluginDetails>,
    require_passcode: bool,
}

impl PendingConnection {
    /// Metadata extracted from the incoming `ProtocolEvent`.
    pub fn handshake_info(&self) -> ProtocolHandshake {
        ProtocolHandshake {
            version: self.conn.version,
            plugins: self.plugins.clone(),
            require_passcode: self.require_passcode,
        }
    }

    /// Plugins advertised by Minecraft.
    pub fn plugins(&self) -> &[PluginDetails] {
        &self.plugins
    }

    /// Whether the server requires a passcode.
    pub fn require_passcode(&self) -> bool {
        self.require_passcode
    }

    /// Complete the handshake by validating passcode/target and sending the
    /// debugger's protocol response. Consumes the pending connection and
    /// returns a fully operational [`DebuggeeConnection`].
    pub async fn complete(
        self,
        opts: ConnectOptions,
    ) -> Result<(DebuggeeConnection, ProtocolHandshake), ConnectionError> {
        let PendingConnection {
            mut conn,
            plugins,
            require_passcode,
        } = self;

        if require_passcode && opts.passcode.is_none() {
            return Err(ConnectionError::PasscodeRequired);
        }

        let target_uuid = match opts.target_module_uuid {
            Some(uuid) => {
                // Reject explicit targets not in the plugin list, unless the
                // list is empty (legacy / diagnostics use).
                if !plugins.is_empty()
                    && !plugins.iter().any(|p| p.module_uuid == uuid)
                {
                    return Err(ConnectionError::TargetNotFound {
                        selected: uuid,
                        available: plugins
                            .iter()
                            .map(|p| (p.name.clone(), p.module_uuid.clone()))
                            .collect(),
                    });
                }
                Some(uuid)
            }
            None => {
                if plugins.len() == 1 {
                    Some(plugins[0].module_uuid.clone())
                } else if plugins.len() > 1 {
                    return Err(ConnectionError::AmbiguousTarget {
                        available: plugins
                            .iter()
                            .map(|p| (p.name.clone(), p.module_uuid.clone()))
                            .collect(),
                    });
                } else {
                    None
                }
            }
        };

        let version = conn.version;
        conn.send_event(&DebuggerEvent::Protocol {
            version: version.as_u8(),
            target_module_uuid: target_uuid,
            passcode: opts.passcode,
        })
        .await?;

        Ok((
            conn,
            ProtocolHandshake {
                version,
                plugins,
                require_passcode,
            },
        ))
    }
}

impl DebuggeeConnection {
    /// Connect to Minecraft and complete the full handshake with default options.
    pub async fn connect(
        host: &str,
        port: u16,
    ) -> Result<(Self, ProtocolHandshake), ConnectionError> {
        Self::connect_with_options(host, port, ConnectOptions::default()).await
    }

    /// Connect to Minecraft with explicit options (equivalent to pending + complete).
    pub async fn connect_with_options(
        host: &str,
        port: u16,
        opts: ConnectOptions,
    ) -> Result<(Self, ProtocolHandshake), ConnectionError> {
        let pending = Self::connect_pending(host, port).await?;
        pending.complete(opts).await
    }

    /// Listen for an incoming Minecraft debugger connection and complete
    /// the full handshake with default options.
    pub async fn listen(port: u16) -> Result<(Self, ProtocolHandshake), ConnectionError> {
        Self::listen_with_options(port, ConnectOptions::default()).await
    }

    /// Listen with explicit options (equivalent to pending + complete).
    pub async fn listen_with_options(
        port: u16,
        opts: ConnectOptions,
    ) -> Result<(Self, ProtocolHandshake), ConnectionError> {
        let pending = Self::listen_pending(port).await?;
        pending.complete(opts).await
    }

    // ── Pending (two-phase) handshake API ───────────────────────────────

    /// Connect to Minecraft but stop after receiving the `ProtocolEvent`.
    /// Does **not** validate passcode/target or send the debugger's protocol response.
    /// Call [`PendingConnection::complete`] to finish the handshake.
    pub async fn connect_pending(
        host: &str,
        port: u16,
    ) -> Result<PendingConnection, ConnectionError> {
        let stream = TcpStream::connect((host, port)).await?;
        Self::handshake_pending(stream).await
    }

    /// Accept an incoming Minecraft debugger connection but stop after
    /// receiving the `ProtocolEvent`.  Does **not** validate passcode/target
    /// or send the debugger's protocol response.
    /// Call [`PendingConnection::complete`] to finish the handshake.
    pub async fn listen_pending(port: u16) -> Result<PendingConnection, ConnectionError> {
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;
        let (stream, _) = listener.accept().await?;
        drop(listener);
        Self::handshake_pending(stream).await
    }

    /// Internal: read the initial `ProtocolEvent` and set up the connection
    /// without validating passcode/target or sending the protocol response.
    async fn handshake_pending(
        stream: TcpStream,
    ) -> Result<PendingConnection, ConnectionError> {
        let mut conn = Self {
            stream,
            codec: MessageCodec::new(),
            read_buf: BytesMut::new(),
            event_buffer: VecDeque::new(),
            next_request_seq: 1,
            version: ProtocolVersion::CURRENT,
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
        conn.version = negotiated;

        Ok(PendingConnection {
            conn,
            plugins,
            require_passcode,
        })
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

    async fn send_raw(&mut self, value: serde_json::Value) -> Result<(), ConnectionError> {
        let mut buf = BytesMut::new();
        self.codec.encode(value, &mut buf)?;
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    fn build_request_payload(
        &self,
        seq: u32,
        command: impl Into<String>,
        args: serde_json::Value,
    ) -> serde_json::Value {
        let command = command.into();
        if self.version.as_u8() >= ProtocolVersion::V8.as_u8() {
            serde_json::json!({
                "type": "request",
                "request_seq": seq,
                "command": command,
                "args": args,
            })
        } else {
            serde_json::json!({
                "type": "request",
                "request": {
                    "request_seq": seq,
                    "command": command,
                    "args": args,
                },
            })
        }
    }

    pub async fn send_minecraft_command(
        &mut self,
        command: &str,
        dimension_type: &str,
    ) -> Result<(), ConnectionError> {
        let payload = if self.version.as_u8() >= ProtocolVersion::V8.as_u8() {
            serde_json::json!({
                "type": "minecraftCommand",
                "command": command,
                "dimension_type": dimension_type,
            })
        } else {
            serde_json::json!({
                "type": "minecraftCommand",
                "command": {
                    "command": command,
                    "dimension_type": dimension_type,
                },
            })
        };
        self.send_raw(payload).await
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
        self.request_with_timeout(command, args, std::time::Duration::from_secs(5))
            .await
    }

    pub async fn request_with_timeout(
        &mut self,
        command: impl Into<String>,
        args: serde_json::Value,
        timeout: std::time::Duration,
    ) -> Result<DebuggeeResponse, ConnectionError> {
        let seq = self.next_request_seq;
        self.next_request_seq = self.next_request_seq.checked_add(1).unwrap_or(1);

        let payload = self.build_request_payload(seq, command, args);
        self.send_raw(payload).await?;

        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                event_result = self.recv_event_inner() => match event_result? {
                    DebuggeeEvent::Response {
                        request_seq,
                        success,
                        body,
                        error,
                        ..
                    } if request_seq == seq => {
                        return Ok(DebuggeeResponse {
                            request_seq,
                            args: body,
                            success: success.unwrap_or(true),
                            response_message: error,
                        });
                    }
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
                },
                _ = &mut deadline => {
                    return Err(ConnectionError::RequestTimeout(timeout));
                }
            }
        }
    }

    pub async fn step_next(
        &mut self,
        thread_id: u32,
    ) -> Result<(), ConnectionError> {
        self.send_request_no_wait("next", serde_json::json!({"threadId": thread_id}))
            .await
    }

    pub async fn step_in(
        &mut self,
        thread_id: u32,
    ) -> Result<(), ConnectionError> {
        self.send_request_no_wait("stepIn", serde_json::json!({"threadId": thread_id}))
            .await
    }

    pub async fn step_out(
        &mut self,
        thread_id: u32,
    ) -> Result<(), ConnectionError> {
        self.send_request_no_wait("stepOut", serde_json::json!({"threadId": thread_id}))
            .await
    }

    pub async fn continue_thread(
        &mut self,
        thread_id: u32,
    ) -> Result<(), ConnectionError> {
        self.send_request_no_wait("continue", serde_json::json!({"threadId": thread_id}))
            .await
    }

    pub async fn pause(
        &mut self,
        thread_id: u32,
    ) -> Result<(), ConnectionError> {
        self.send_request_no_wait("pause", serde_json::json!({"threadId": thread_id}))
            .await
    }

    pub async fn send_request_no_wait(
        &mut self,
        command: impl Into<String>,
        args: serde_json::Value,
    ) -> Result<(), ConnectionError> {
        let seq = self.next_request_seq;
        self.next_request_seq = self.next_request_seq.checked_add(1).unwrap_or(1);
        let payload = self.build_request_payload(seq, command, args);
        self.send_raw(payload).await
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

    // ── PendingConnection tests ─────────────────────────────────────────

    #[tokio::test]
    async fn pending_single_plugin_auto_selects() {
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

        let pending = tokio::time::timeout(
            TEST_TIMEOUT,
            DebuggeeConnection::connect_pending("127.0.0.1", port),
        )
        .await
        .expect("pending connect timed out")
        .expect("pending connect failed");

        assert_eq!(pending.plugins().len(), 1);
        assert_eq!(pending.plugins()[0].module_uuid, "abc-123");
        assert!(!pending.require_passcode());

        let (_conn, hs) = tokio::time::timeout(
            TEST_TIMEOUT,
            pending.complete(ConnectOptions::default()),
        )
        .await
        .expect("complete timed out")
        .expect("complete failed");

        assert_eq!(hs.plugins.len(), 1);

        let server_response = tokio::time::timeout(TEST_TIMEOUT, server)
            .await
            .expect("server timed out")
            .expect("server task panicked");
        assert_eq!(server_response["type"], "protocol");
        assert_eq!(server_response["target_module_uuid"], "abc-123");
    }

    #[tokio::test]
    async fn pending_explicit_target_overrides_auto() {
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

        let pending = DebuggeeConnection::connect_pending("127.0.0.1", port)
            .await
            .expect("pending connect failed");

        let (_conn, _hs) = pending
            .complete(ConnectOptions {
                target_module_uuid: Some("uuid-2".into()),
                passcode: None,
            })
            .await
            .expect("complete failed");

        let server_response = tokio::time::timeout(TEST_TIMEOUT, server)
            .await
            .expect("server timed out")
            .expect("server task panicked");
        assert_eq!(server_response["target_module_uuid"], "uuid-2");
    }

    #[tokio::test]
    async fn pending_multiple_no_target_gives_ambiguous() {
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
                        {"name": "bp.a", "module_uuid": "u1"},
                        {"name": "bp.b", "module_uuid": "u2"}
                    ],
                    "require_passcode": false
                }),
            )
            .await;
            let _ = sock.read(&mut [0u8; 16]).await;
        });

        let pending = DebuggeeConnection::connect_pending("127.0.0.1", port)
            .await
            .expect("pending connect failed");

        let result = tokio::time::timeout(
            TEST_TIMEOUT,
            pending.complete(ConnectOptions::default()),
        )
        .await
        .expect("complete timed out");

        match result {
            Err(ConnectionError::AmbiguousTarget { available }) => {
                assert_eq!(available.len(), 2);
            }
            other => panic!("expected AmbiguousTarget, got {other:?}"),
        }

        let _ = tokio::time::timeout(TEST_TIMEOUT, server).await;
    }

    #[tokio::test]
    async fn pending_explicit_target_not_found_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            send_frame(
                &mut sock,
                serde_json::json!({
                    "type": "ProtocolEvent",
                    "version": 9,
                    "plugins": [{"name": "bp.one", "module_uuid": "uuid-1"}],
                    "require_passcode": false
                }),
            )
            .await;
            let _ = sock.read(&mut [0u8; 16]).await;
        });

        let pending = DebuggeeConnection::connect_pending("127.0.0.1", port)
            .await
            .expect("pending connect failed");

        let result = pending
            .complete(ConnectOptions {
                target_module_uuid: Some("nonexistent".into()),
                passcode: None,
            })
            .await;

        match result {
            Err(ConnectionError::TargetNotFound { selected, available }) => {
                assert_eq!(selected, "nonexistent");
                assert_eq!(available.len(), 1);
                assert_eq!(available[0].1, "uuid-1");
            }
            other => panic!("expected TargetNotFound, got {other:?}"),
        }

        let _ = tokio::time::timeout(TEST_TIMEOUT, server).await;
    }

    #[tokio::test]
    async fn pending_empty_plugins_allows_explicit_target() {
        // Legacy / diagnostics: explicit target is allowed even when
        // Minecraft reports zero plugins.
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
            recv_frame(&mut sock, &mut codec, &mut buf).await
        });

        let pending = DebuggeeConnection::connect_pending("127.0.0.1", port)
            .await
            .expect("pending connect failed");

        let (_conn, _hs) = pending
            .complete(ConnectOptions {
                target_module_uuid: Some("legacy-uuid".into()),
                passcode: None,
            })
            .await
            .expect("complete should allow explicit target on empty plugins");

        let server_response = tokio::time::timeout(TEST_TIMEOUT, server)
            .await
            .expect("server timed out")
            .expect("server task panicked");
        assert_eq!(server_response["target_module_uuid"], "legacy-uuid");
    }

    #[tokio::test]
    async fn pending_listen_works() {
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
                    "plugins": [{"name": "bp.x", "module_uuid": "xxx"}],
                    "require_passcode": false
                }),
            )
            .await;
            let mut codec = MessageCodec::new();
            let mut buf = BytesMut::new();
            recv_frame(&mut sock, &mut codec, &mut buf).await
        });

        let pending = DebuggeeConnection::listen_pending(port)
            .await
            .expect("listen pending failed");
        assert_eq!(pending.plugins().len(), 1);

        let (_conn, _hs) = pending.complete(ConnectOptions::default()).await.unwrap();

        let server_response = tokio::time::timeout(TEST_TIMEOUT, server)
            .await
            .expect("server timed out")
            .expect("server task panicked");
        assert_eq!(server_response["target_module_uuid"], "xxx");
    }

    #[tokio::test]
    async fn existing_wrapper_behavior_unchanged() {
        // Sanity check that the old convenience APIs still work identically.
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
            recv_frame(&mut sock, &mut codec, &mut buf).await
        });

        let (_conn, hs) = DebuggeeConnection::connect("127.0.0.1", port)
            .await
            .expect("connect failed");
        assert_eq!(hs.version, ProtocolVersion::CURRENT);

        let _ = tokio::time::timeout(TEST_TIMEOUT, server).await;
    }
}
