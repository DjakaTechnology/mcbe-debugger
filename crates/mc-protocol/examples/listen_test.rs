use mc_protocol::{
    DebuggeeConnection, DebuggeeEvent, DebuggerEvent, ProtocolHandshake, DEFAULT_PORT,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== mc-protocol listen test ===");
    println!("Listening on 127.0.0.1:{}", DEFAULT_PORT);
    println!("Run in MC Bedrock chat: /script debugger connect");
    println!();

    let (mut conn, handshake) = DebuggeeConnection::listen(DEFAULT_PORT).await?;

    let peer = conn
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "<unknown>".into());
    println!("Accepted connection from {}", peer);
    println!();
    print_handshake(&handshake);

    println!();
    println!("Sending Resume...");
    conn.send_event(&DebuggerEvent::Resume).await?;
    println!("Resume sent. Listening for events (Ctrl+C to exit).");
    println!();

    loop {
        match conn.recv_event().await {
            Ok(event) => print_event(&event),
            Err(e) => {
                println!();
                println!("=== Connection closed: {} ===", e);
                break;
            }
        }
    }

    Ok(())
}

fn print_handshake(handshake: &ProtocolHandshake) {
    println!("=== Handshake complete ===");
    println!("Protocol version: v{}", handshake.version.as_u8());
    println!("Plugins ({}):", handshake.plugins.len());
    for plugin in &handshake.plugins {
        println!("  - {} ({})", plugin.name, plugin.module_uuid);
    }
    println!("Require passcode: {}", handshake.require_passcode);
}

fn print_event(event: &DebuggeeEvent) {
    let line = match event {
        DebuggeeEvent::Protocol {
            version,
            plugins,
            require_passcode,
        } => format!(
            "Protocol v{} ({} plugins, passcode={})",
            version,
            plugins.len(),
            require_passcode
        ),
        DebuggeeEvent::Stopped { reason, thread } => {
            format!("Stopped: reason={}, thread={}", reason, thread)
        }
        DebuggeeEvent::Thread { reason, thread } => {
            format!("Thread {}: {}", thread, reason)
        }
        DebuggeeEvent::Print {
            message,
            log_level,
        } => format!("[{:?}] {}", log_level, message),
        DebuggeeEvent::Notification {
            message,
            log_level,
        } => format!("[Notification {:?}] {}", log_level, message),
        DebuggeeEvent::Stat2 { tick, stats } => {
            format!("Stat @ tick {} ({} roots)", tick, stats.len())
        }
        DebuggeeEvent::ProfilerCapture { capture_base_path, .. } => {
            format!("ProfilerCapture -> {}", capture_base_path)
        }
        DebuggeeEvent::DebuggeeResponse {
            request_seq,
            success,
            response_message,
            ..
        } => format!(
            "DebuggeeResponse seq={} success={:?} msg={:?}",
            request_seq, success, response_message
        ),
        DebuggeeEvent::Schema { descriptors } => {
            format!("Schema ({} tabs)", descriptors.len())
        }
    };
    println!("<<< {}", line);
}
