fn main() {
    println!(
        "mc-dap-server {} — DAP sidecar for Minecraft Bedrock",
        env!("CARGO_PKG_VERSION")
    );
    println!(
        "DAP protocol: {}",
        mc_dap_server::DAP_PROTOCOL_VERSION
    );
    println!(
        "MC protocol: v{}",
        mc_protocol::ProtocolVersion::CURRENT.as_u8()
    );
    println!("Default MC port: {}", mc_protocol::DEFAULT_PORT);
    println!();
    println!("This binary will eventually:");
    println!("  1. Listen on a TCP port for Zed's DAP client to connect");
    println!("  2. Open a raw TCP socket to Minecraft at 127.0.0.1:19144");
    println!("  3. Translate DAP requests <-> MC debug protocol events");
    println!();
    println!("Not yet implemented. Exiting.");
    std::process::exit(0);
}
