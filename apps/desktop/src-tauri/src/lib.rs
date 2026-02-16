mod core;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_core = core::AppCore::new().expect("failed to initialize app core");

    tauri::Builder::default()
        .manage(app_core)
        .plugin(
            tauri_plugin_log::Builder::default()
                .level(log::LevelFilter::Info)
                .build(),
        )
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<core::AppCore>();
                if let Err(err) = state.emit_initial_events(&handle).await {
                    log::warn!("failed to emit initial state events: {err}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core::bootstrap,
            core::connect,
            core::disconnect,
            core::set_mute,
            core::set_deafen,
            core::set_ptt,
            core::set_ptt_hotkey,
            core::set_input_device,
            core::set_output_device,
            core::refresh_devices
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
