#[tauri::command]
fn adapter_info() -> mc_tauri::AdapterInfo {
    mc_tauri::adapter_info()
}

#[tauri::command]
async fn listen_to_minecraft(
    state: tauri::State<'_, mc_tauri::AppState>,
    app: tauri::AppHandle,
    port: u16,
    target_module_uuid: Option<String>,
    passcode: Option<String>,
) -> Result<mc_tauri::HandshakeInfo, String> {
    mc_tauri::listen_to_minecraft(&state, app, port, target_module_uuid, passcode).await
}

#[tauri::command]
async fn connect_to_minecraft(
    state: tauri::State<'_, mc_tauri::AppState>,
    app: tauri::AppHandle,
    host: String,
    port: u16,
    target_module_uuid: Option<String>,
    passcode: Option<String>,
) -> Result<mc_tauri::HandshakeInfo, String> {
    mc_tauri::connect_to_minecraft(&state, app, host, port, target_module_uuid, passcode).await
}

#[tauri::command]
async fn disconnect(state: tauri::State<'_, mc_tauri::AppState>) -> Result<(), String> {
    mc_tauri::disconnect(&state).await
}

#[tauri::command]
async fn send_resume(state: tauri::State<'_, mc_tauri::AppState>) -> Result<(), String> {
    mc_tauri::send_resume(&state).await
}

#[tauri::command]
async fn get_handshake_info(
    state: tauri::State<'_, mc_tauri::AppState>,
) -> Result<Option<mc_tauri::HandshakeInfo>, String> {
    mc_tauri::get_handshake_info(&state).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(mc_tauri::AppState::new())
        .invoke_handler(tauri::generate_handler![
            adapter_info,
            listen_to_minecraft,
            connect_to_minecraft,
            disconnect,
            send_resume,
            get_handshake_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
