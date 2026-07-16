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
async fn cancel_pending_connect(state: tauri::State<'_, mc_tauri::AppState>) -> Result<(), String> {
    mc_tauri::cancel_pending_connect(&state).await
}

#[tauri::command]
async fn send_minecraft_command(
    state: tauri::State<'_, mc_tauri::AppState>,
    command: String,
) -> Result<(), String> {
    mc_tauri::send_minecraft_command(&state, command).await
}

#[tauri::command]
async fn get_handshake_info(
    state: tauri::State<'_, mc_tauri::AppState>,
) -> Result<Option<mc_tauri::HandshakeInfo>, String> {
    mc_tauri::get_handshake_info(&state).await
}

#[tauri::command]
async fn pause_thread(
    state: tauri::State<'_, mc_tauri::AppState>,
    thread_id: u32,
) -> Result<(), String> {
    mc_tauri::pause_thread(&state, thread_id).await
}

#[tauri::command]
async fn continue_thread(
    state: tauri::State<'_, mc_tauri::AppState>,
    thread_id: u32,
) -> Result<(), String> {
    mc_tauri::continue_thread(&state, thread_id).await
}

#[tauri::command]
async fn step_next(
    state: tauri::State<'_, mc_tauri::AppState>,
    thread_id: u32,
) -> Result<(), String> {
    mc_tauri::step_next(&state, thread_id).await
}

#[tauri::command]
async fn step_in(
    state: tauri::State<'_, mc_tauri::AppState>,
    thread_id: u32,
) -> Result<(), String> {
    mc_tauri::step_in(&state, thread_id).await
}

#[tauri::command]
async fn step_out(
    state: tauri::State<'_, mc_tauri::AppState>,
    thread_id: u32,
) -> Result<(), String> {
    mc_tauri::step_out(&state, thread_id).await
}

#[tauri::command]
async fn evaluate(
    state: tauri::State<'_, mc_tauri::AppState>,
    expression: String,
) -> Result<mc_tauri::ResponsePayload, String> {
    mc_tauri::evaluate(&state, expression).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(mc_tauri::AppState::new())
        .invoke_handler(tauri::generate_handler![
            adapter_info,
            listen_to_minecraft,
            connect_to_minecraft,
            disconnect,
            cancel_pending_connect,
            send_minecraft_command,
            get_handshake_info,
            pause_thread,
            continue_thread,
            step_next,
            step_in,
            step_out,
            evaluate,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
