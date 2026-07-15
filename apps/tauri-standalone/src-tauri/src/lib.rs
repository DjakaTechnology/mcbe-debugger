#[tauri::command]
fn adapter_info() -> mc_tauri::AdapterInfo {
    mc_tauri::adapter_info()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![adapter_info])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
