//! Integration test proving that the TUI crate can exercise the full
//! mc-session controller API without modification:
//!
//! 1. A mock TCP server advertises multiple plugins
//! 2. SessionController::connect emits TargetSelectionRequired
//! 3. The test (acting as the TUI) calls SessionController::select_target
//! 4. The handshake completes with the selected UUID
//! 5. The server sends a debuggee event which arrives at the controller

use std::time::Duration;

use bytes::BytesMut;
use mc_protocol::events::DebuggeeEvent;
use mc_protocol::framing::MessageCodec;
use mc_session::{SessionController, SessionEvent};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::codec::{Decoder, Encoder};

use mc_tui::app::{App, ConnectionState};

/// Helper: encode a JSON value as a framed message and write it to a TCP stream.
async fn send_test_frame(sock: &mut tokio::net::TcpStream, value: serde_json::Value) {
    let mut codec = MessageCodec::new();
    let mut buf = BytesMut::new();
    codec.encode(value, &mut buf).unwrap();
    sock.write_all(&buf).await.unwrap();
}

#[tokio::test]
async fn framed_stat_event2_reaches_app_and_accumulates_when_stat_log_is_filtered() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let (mut codec, mut buf) = handshake_and_resume(&mut sock).await;
        send_test_frame(&mut sock, serde_json::json!({"type":"event", "event": {
            "type":"StatEvent2", "tick":42, "stats":[{"name":"server_tick_timings","values":[],
            "children":[{"name":"tick","values":[10,20],"children":[],"should_aggregate":false},
            {"name":"entities","values":[3],"children":[],"should_aggregate":false}],"should_aggregate":false}]
        }})).await;
        let _ = read_frame(&mut sock, &mut codec, &mut buf).await;
    });
    let (controller, event_rx) = SessionController::new();
    let hs = tokio::time::timeout(
        Duration::from_secs(5),
        controller.connect("127.0.0.1".into(), port, None, None),
    )
    .await
    .expect("timed out connecting")
    .expect("connect should succeed");
    let mut app = App::new(
        controller.clone(),
        event_rx,
        "127.0.0.1".into(),
        port,
        None,
        None,
    );
    app.handle_connection_result(Ok(hs));
    app.log_filter.kinds.stat = false;
    let event = tokio::time::timeout(Duration::from_secs(5), app.event_rx.recv())
        .await
        .expect("timed out waiting for StatEvent2")
        .expect("event channel closed unexpectedly");
    assert!(matches!(
        event,
        SessionEvent::Debuggee(DebuggeeEvent::Stat2 { .. })
    ));
    app.handle_session_event(event);
    let series = &app.stats.collection()["server_tick_timings.tick"];
    assert_eq!(series.ticks, vec![42]);
    assert_eq!(series.values, vec![20.0]);
    assert!(app
        .event_log
        .iter()
        .any(|entry| entry.kind == mc_tui::app::LogKind::Stat
            && entry.message.contains("Stats tick=42")));
    assert!(app
        .filtered_log_indices
        .iter()
        .all(|&i| app.event_log[i].kind != mc_tui::app::LogKind::Stat));
    let _ = controller.disconnect().await;
    let _ = tokio::time::timeout(Duration::from_secs(3), server).await;
}

#[tokio::test]
async fn protocol_v7_handshake_and_nested_request_roundtrip() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let (mut codec, mut buf) = handshake_and_resume_v7(&mut sock).await;
        let request = read_frame(&mut sock, &mut codec, &mut buf)
            .await
            .expect("expected v7 request");
        assert_eq!(request["type"], "request");
        assert_eq!(request["request"]["command"], "pause");
        assert_eq!(request["request"]["args"]["threadId"], 0);
    });
    let (controller, event_rx) = SessionController::new();
    let hs = tokio::time::timeout(
        Duration::from_secs(5),
        controller.connect("127.0.0.1".into(), port, None, None),
    )
    .await
    .expect("timed out waiting for v7 handshake")
    .expect("v7 handshake should succeed");
    assert_eq!(hs.version, 7);
    let mut app = App::new(
        controller.clone(),
        event_rx,
        "127.0.0.1".into(),
        port,
        None,
        None,
    );
    app.handle_connection_result(Ok(hs));
    app.request_pause(0).await;
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("v7 server timed out")
        .expect("v7 server panicked");
    let _ = controller.disconnect().await;
}

async fn handshake_and_resume_v7(sock: &mut tokio::net::TcpStream) -> (MessageCodec, BytesMut) {
    let mut codec = MessageCodec::new();
    let mut buf = BytesMut::new();
    send_test_frame(sock, serde_json::json!({"type":"ProtocolEvent","version":7,"plugins":[],"require_passcode":false})).await;
    let protocol = read_frame(sock, &mut codec, &mut buf)
        .await
        .expect("v7 protocol response");
    assert_eq!(protocol["type"], "protocol");
    assert_eq!(protocol["version"], 7);
    let resume = read_frame(sock, &mut codec, &mut buf)
        .await
        .expect("v7 resume");
    assert_eq!(resume["type"], "resume");
    (codec, buf)
}

/// Helper: read one framed JSON value from a TCP stream.
async fn read_frame(
    sock: &mut tokio::net::TcpStream,
    codec: &mut MessageCodec,
    buf: &mut BytesMut,
) -> Option<serde_json::Value> {
    loop {
        if let Some(value) = codec.decode(buf).unwrap() {
            return Some(value);
        }
        let mut tmp = [0u8; 4096];
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

/// Helper: complete the handshake on a mock server and consume the auto-resume.
async fn handshake_and_resume(sock: &mut tokio::net::TcpStream) -> (MessageCodec, BytesMut) {
    let mut codec = MessageCodec::new();
    let mut buf = BytesMut::new();

    send_test_frame(
        sock,
        serde_json::json!({
            "type": "ProtocolEvent",
            "version": 9,
            "plugins": [],
            "require_passcode": false,
        }),
    )
    .await;

    let protocol = read_frame(sock, &mut codec, &mut buf)
        .await
        .expect("server: expected protocol response");
    assert_eq!(protocol["type"], "protocol");

    let resume = read_frame(sock, &mut codec, &mut buf)
        .await
        .expect("server: expected auto-resume");
    assert_eq!(resume["type"], "resume");

    (codec, buf)
}

/// Helper: build a connected App backed by a real controller.
async fn connected_app(port: u16) -> App {
    let (controller, event_rx) = SessionController::new();
    let hs = controller
        .connect("127.0.0.1".into(), port, None, None)
        .await
        .expect("connect should succeed");
    let mut app = App::new(controller, event_rx, "127.0.0.1".into(), 19144, None, None);
    app.handle_connection_result(Ok(hs));
    app
}

#[tokio::test]
async fn multi_plugin_selection_handshake_and_event() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    // ── Mock server ───────────────────────────────────────────────────
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();

        // 1) Send ProtocolEvent with 2 plugins -> triggers TargetSelectionRequired
        send_test_frame(
            &mut sock,
            serde_json::json!({
                "type": "ProtocolEvent",
                "version": 9,
                "plugins": [
                    {"name": "AlphaPlugin", "module_uuid": "uuid-alpha"},
                    {"name": "BetaPlugin",  "module_uuid": "uuid-beta"},
                ],
                "require_passcode": false,
            }),
        )
        .await;

        // 2) Read the protocol response -> confirms handshake completed with selection
        let mut codec = MessageCodec::new();
        let mut read_buf = BytesMut::new();
        loop {
            if let Some(value) = codec.decode(&mut read_buf).unwrap() {
                assert_eq!(value["type"], "protocol", "expected protocol response");
                // The TUI should have selected uuid-beta (second plugin)
                assert_eq!(
                    value["target_module_uuid"].as_str(),
                    Some("uuid-beta"),
                    "target_module_uuid should be uuid-beta"
                );
                break;
            }
            let mut tmp = [0u8; 4096];
            let n = sock.read(&mut tmp).await.unwrap();
            if n == 0 {
                panic!("server: unexpected close before protocol response");
            }
            read_buf.extend_from_slice(&tmp[..n]);
        }

        // 3) Read the auto Resume sent by connection_task
        loop {
            if let Some(value) = codec.decode(&mut read_buf).unwrap() {
                assert_eq!(value["type"], "resume", "expected resume");
                break;
            }
            let mut tmp = [0u8; 4096];
            let n = sock.read(&mut tmp).await.unwrap();
            if n == 0 {
                panic!("server: unexpected close before resume");
            }
            read_buf.extend_from_slice(&tmp[..n]);
        }

        // 4) Send a Print event to prove incoming events reach the controller
        send_test_frame(
            &mut sock,
            serde_json::json!({
                "type": "PrintEvent",
                "message": "hello from mock server",
                "logLevel": 0,
            }),
        )
        .await;

        // 5) Wait for the client to close (graceful shutdown)
        let mut tmp = [0u8; 4096];
        loop {
            match sock.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });

    // ── Client ─────────────────────────────────────────────────────────
    let (controller, mut rx) = SessionController::new();

    // Spawn connect so the main test can consume TargetSelectionRequired
    let ctrl = controller.clone();
    let connect_task =
        tokio::spawn(async move { ctrl.connect("127.0.0.1".into(), port, None, None).await });

    // Wait for TargetSelectionRequired
    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for event")
        .expect("event channel closed unexpectedly");

    match &event {
        SessionEvent::TargetSelectionRequired { plugins } => {
            assert_eq!(plugins.len(), 2, "expected 2 plugins");
            assert_eq!(plugins[0].name, "AlphaPlugin");
            assert_eq!(plugins[1].name, "BetaPlugin");
        }
        other => panic!("expected TargetSelectionRequired, got {other:?}"),
    }

    // Select the second plugin -> same handshake continues with uuid-beta
    controller
        .select_target("uuid-beta".into())
        .await
        .expect("select_target should succeed");

    // Wait for connect to complete
    let hs = tokio::time::timeout(Duration::from_secs(5), connect_task)
        .await
        .expect("timed out waiting for connect task")
        .expect("connect task panicked")
        .expect("connect should succeed");

    assert_eq!(hs.version, 9, "protocol version should be 9");
    assert_eq!(hs.plugins.len(), 2, "should report 2 plugins");

    // Wait for the Print event from the server
    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for Print event")
        .expect("event channel closed unexpectedly");

    match event {
        SessionEvent::Debuggee(mc_protocol::events::DebuggeeEvent::Print { message, .. }) => {
            assert_eq!(message, "hello from mock server");
        }
        other => panic!("expected Debuggee(Print), got {other:?}"),
    }

    // Cleanup
    let _ = controller.disconnect().await;
    let _ = tokio::time::timeout(Duration::from_secs(3), server).await;
}

// ── Phase 3: debug controls, commands, and evaluate ───────────────────

#[tokio::test]
async fn pause_command_reaches_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let (mut codec, mut buf) = handshake_and_resume(&mut sock).await;

        let req = read_frame(&mut sock, &mut codec, &mut buf)
            .await
            .expect("server: expected pause request");
        assert_eq!(req["type"], "request");
        assert_eq!(req["command"], "pause");
        assert_eq!(req["args"]["threadId"], 0);
    });

    let mut app = tokio::time::timeout(Duration::from_secs(5), connected_app(port))
        .await
        .expect("timed out connecting");

    assert!(matches!(app.state, ConnectionState::Connected { .. }));
    app.request_pause(0).await;

    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("server timed out")
        .expect("server task panicked");
    let _ = app.controller.disconnect().await;
}

#[tokio::test]
async fn continue_command_reaches_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let (mut codec, mut buf) = handshake_and_resume(&mut sock).await;

        // Put the client into the stopped state before reading the continue.
        send_test_frame(
            &mut sock,
            serde_json::json!({
                "type": "StoppedEvent",
                "reason": "breakpoint",
                "thread": 0,
            }),
        )
        .await;

        let req = read_frame(&mut sock, &mut codec, &mut buf)
            .await
            .expect("server: expected continue request");
        assert_eq!(req["type"], "request");
        assert_eq!(req["command"], "continue");
        assert_eq!(req["args"]["threadId"], 0);
    });

    let mut app = tokio::time::timeout(Duration::from_secs(5), connected_app(port))
        .await
        .expect("timed out connecting");

    app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stopped {
        reason: "breakpoint".into(),
        thread: 0,
    }));
    assert!(app.stopped);

    app.request_continue().await;
    assert!(!app.stopped);

    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("server timed out")
        .expect("server task panicked");
    let _ = app.controller.disconnect().await;
}

#[tokio::test]
async fn step_commands_reach_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let (mut codec, mut buf) = handshake_and_resume(&mut sock).await;

        send_test_frame(
            &mut sock,
            serde_json::json!({
                "type": "StoppedEvent",
                "reason": "breakpoint",
                "thread": 0,
            }),
        )
        .await;

        for expected in ["next", "stepIn", "stepOut"] {
            let req = read_frame(&mut sock, &mut codec, &mut buf)
                .await
                .unwrap_or_else(|| panic!("server: expected {expected} request"));
            assert_eq!(req["type"], "request");
            assert_eq!(req["command"], expected);
            assert_eq!(req["args"]["threadId"], 0);

            // Re-stop so the next step is enabled.
            send_test_frame(
                &mut sock,
                serde_json::json!({
                    "type": "StoppedEvent",
                    "reason": "step",
                    "thread": 0,
                }),
            )
            .await;
        }
    });

    let mut app = tokio::time::timeout(Duration::from_secs(5), connected_app(port))
        .await
        .expect("timed out connecting");

    app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stopped {
        reason: "breakpoint".into(),
        thread: 0,
    }));

    app.request_step_next().await;
    // The server re-stops us between each step.
    app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stopped {
        reason: "step".into(),
        thread: 0,
    }));
    app.request_step_in().await;
    app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stopped {
        reason: "step".into(),
        thread: 0,
    }));
    app.request_step_out().await;

    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("server timed out")
        .expect("server task panicked");
    let _ = app.controller.disconnect().await;
}

#[tokio::test]
async fn minecraft_command_reaches_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let (mut codec, mut buf) = handshake_and_resume(&mut sock).await;

        let req = read_frame(&mut sock, &mut codec, &mut buf)
            .await
            .expect("server: expected minecraftCommand");
        assert_eq!(req["type"], "minecraftCommand");
        assert_eq!(req["command"], "say hello");
    });

    let mut app = tokio::time::timeout(Duration::from_secs(5), connected_app(port))
        .await
        .expect("timed out connecting");

    app.submit_minecraft_command("say hello".into()).await;

    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("server timed out")
        .expect("server task panicked");
    let _ = app.controller.disconnect().await;
}

#[tokio::test]
async fn evaluate_request_reaches_wire_and_result_enters_history() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let (mut codec, mut buf) = handshake_and_resume(&mut sock).await;

        send_test_frame(
            &mut sock,
            serde_json::json!({
                "type": "StoppedEvent",
                "reason": "breakpoint",
                "thread": 0,
            }),
        )
        .await;

        let req = read_frame(&mut sock, &mut codec, &mut buf)
            .await
            .expect("server: expected evaluate request");
        assert_eq!(req["type"], "request");
        assert_eq!(req["command"], "evaluate");
        assert_eq!(req["args"]["expression"], "1+1");
        let seq = req["request_seq"].as_u64().unwrap() as u32;

        send_test_frame(
            &mut sock,
            serde_json::json!({
                "type": "debuggee-response",
                "request_seq": seq,
                "args": { "result": 42 },
                "success": true,
            }),
        )
        .await;
    });

    let mut app = tokio::time::timeout(Duration::from_secs(5), connected_app(port))
        .await
        .expect("timed out connecting");

    app.handle_session_event(SessionEvent::Debuggee(DebuggeeEvent::Stopped {
        reason: "breakpoint".into(),
        thread: 0,
    }));

    app.evaluate_input.field.value = "1+1".into();
    app.start_evaluate().await;
    assert!(app.evaluate_input.busy);

    // Wait until the result has been delivered, polling in place of the main loop.
    let received = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            app.try_recv_evaluate();
            if !app.evaluate_input.busy {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(received.is_ok(), "evaluate result should arrive");

    assert_eq!(app.evaluate_input.history.len(), 1);
    let entry = &app.evaluate_input.history[0];
    assert_eq!(entry.expression, "1+1");
    assert!(entry.success);
    assert!(entry.detail.contains("42"));

    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("server timed out")
        .expect("server task panicked");
    let _ = app.controller.disconnect().await;
}
