use clap::Parser;
use mc_protocol::{
    ConnectOptions, DebuggeeConnection, DebuggeeEvent, DebuggerEvent, ProtocolHandshake,
    DEFAULT_PORT,
};

#[derive(Parser)]
#[command(about = "Listen for Minecraft Bedrock debug connections on 127.0.0.1:19144")]
struct Args {
    /// UUID of the script module to attach to.
    /// Auto-detected when MC has exactly one plugin; required when multiple are present.
    #[arg(long)]
    target_module_uuid: Option<String>,

    /// Passcode (only needed if Minecraft requires one).
    #[arg(long)]
    passcode: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let opts = ConnectOptions {
        target_module_uuid: args.target_module_uuid,
        passcode: args.passcode,
    };

    println!("=== mc-protocol listen test ===");
    println!("Listening on 127.0.0.1:{}", DEFAULT_PORT);
    println!("Run in MC Bedrock chat: /script debugger connect");
    if opts.target_module_uuid.is_some() {
        println!("Target module UUID: {:?}", opts.target_module_uuid);
    }
    if opts.passcode.is_some() {
        println!("Passcode: <provided>");
    }
    println!();

    let (mut conn, handshake) =
        DebuggeeConnection::listen_with_options(DEFAULT_PORT, opts).await?;

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
            Ok(event) => {
                if matches!(event, DebuggeeEvent::Terminated { .. }) {
                    println!();
                    println!("=== Session terminated by Minecraft ===");
                    if let DebuggeeEvent::Terminated { reason: Some(r) } = &event {
                        println!("Reason: {}", r);
                    }
                    break;
                }
                print_event(&event);
            }
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
        DebuggeeEvent::Response { request_seq, success, .. } => {
            format!("Response seq={} success={:?}", request_seq, success)
        }
        DebuggeeEvent::Terminated { reason } => {
            format!("Terminated (reason={:?})", reason)
        }
        DebuggeeEvent::Unknown { type_name, data } => {
            format!("[UNKNOWN type={}] {}", type_name, data)
        }
    };
    println!("<<< {}", line);
}
