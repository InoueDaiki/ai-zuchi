#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(debug_assertions)]
mod dev;
mod overlay;
mod payload;
mod reminders;
mod runner;
mod tray;

use std::sync::mpsc;

use overlay::Overlay;
use reminders::ReminderInbox;
use tauri::Manager;

fn main() {
    let (events, received) = mpsc::channel();
    let (reminder_commands, reminder_inbox) = mpsc::channel();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // Rust からだけ使う。画面には通知の権限を渡さない
        .plugin(tauri_plugin_notification::init())
        .manage(Overlay::new(events))
        .manage(ReminderInbox::new(reminder_commands))
        .invoke_handler(tauri::generate_handler![
            overlay::report_reaction,
            overlay::accept_action,
            overlay::open_external,
            overlay::set_overlay_height,
        ])
        .setup(move |app| {
            // Dock に出さず、起動しても手前のアプリからフォーカスを奪わない
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let window = app
                .get_webview_window(overlay::WINDOW_LABEL)
                .ok_or("オーバーレイのウィンドウがない")?;
            overlay::prepare_window(&window)?;
            tray::install(app.handle())?;
            runner::spawn(app.handle().clone(), received, reminder_inbox);
            #[cfg(debug_assertions)]
            dev::start_from_env(app.handle());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("アプリを起動できなかった");
}
