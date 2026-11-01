//! 取得と判定をアプリにつなぐ。モデルの読み込みに時間がかかるので別のスレッドで始める。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use zuchi_runtime::secret::load_or_create_key;
use zuchi_runtime::{Runtime, RuntimeConfig};

use crate::overlay::{Overlay, OverlayEvent};
use crate::reminders::{self, ReminderCommand, ReminderInbox};
use crate::tray;

const DEFAULT_CHAT_MODEL: &str = "gemma4:e4b-it-q4_K_M";
const DEFAULT_EMBED_MODEL: &str = "embeddinggemma";
/// 一時停止の切り替えが取得へ届くまでの最大の時間
const SYNC_INTERVAL: Duration = Duration::from_millis(300);

pub fn spawn(
    app: AppHandle,
    events: Receiver<OverlayEvent>,
    reminder_commands: Receiver<ReminderCommand>,
) {
    thread::spawn(move || {
        if let Err(error) = run(&app, events, reminder_commands) {
            eprintln!("起動できなかった: {error}");
        }
    });
}

fn config(app: &AppHandle) -> tauri::Result<RuntimeConfig> {
    let data_dir = app.path().app_data_dir()?;
    let env = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    Ok(RuntimeConfig {
        models_dir: std::env::var("ZUCHI_MODELS_DIR")
            .map_or_else(|_| data_dir.join("models"), PathBuf::from),
        data_dir,
        chat_model: env("ZUCHI_CHAT_MODEL", DEFAULT_CHAT_MODEL),
        embed_model: env("ZUCHI_EMBED_MODEL", DEFAULT_EMBED_MODEL),
        blocked_terms: env("ZUCHI_BLOCKED_TERMS", "")
            .split(',')
            .filter(|term| !term.trim().is_empty())
            .map(str::to_owned)
            .collect(),
        listen_screen: std::env::var_os("ZUCHI_NO_SCREEN").is_none(),
    })
}

fn run(
    app: &AppHandle,
    events: Receiver<OverlayEvent>,
    reminder_commands: Receiver<ReminderCommand>,
) -> zuchi_core::ports::Result<()> {
    let (suggestions, received) = mpsc::channel();
    let runtime = Arc::new(Runtime::start(
        config(app)?,
        load_or_create_key()?,
        suggestions,
    )?);

    // 一時停止の同期を待たせないよう、データベースに触る処理は別のスレッドで行う
    reminders::spawn(app.clone(), runtime.reminders(), reminder_commands);

    let display = app.clone();
    thread::spawn(move || {
        for suggestion in received {
            let overlay = display.state::<Overlay>();
            if overlay.show_suggestion(&display, suggestion).is_err() {
                eprintln!("補足を表示できなかった");
            }
        }
    });

    // 起動したら聞き始める。一時停止の操作はトレイから届く
    let overlay = app.state::<Overlay>();
    overlay.mark_ready();
    overlay.set_paused(false);
    tray::refresh(app);
    loop {
        let paused = overlay.is_paused();
        if paused == runtime.is_listening() {
            if paused {
                runtime.pause();
            } else {
                for failure in runtime.listen() {
                    eprintln!("{}を取得できない: {}", failure.source, failure.message);
                }
                // 何も取れないなら、聞いていると表示しない
                if !runtime.is_listening() {
                    overlay.set_paused(true);
                    tray::refresh(app);
                }
            }
        }
        match events.recv_timeout(SYNC_INTERVAL) {
            Ok(OverlayEvent::Reaction {
                suggestion,
                reaction,
            }) => runtime.react(suggestion, reaction),
            // 操作が実行へ進むのはここだけ。カードのボタンを本人が押したときにだけ届く
            Ok(OverlayEvent::ActionAccepted { action }) => {
                app.state::<ReminderInbox>().perform(action);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}
