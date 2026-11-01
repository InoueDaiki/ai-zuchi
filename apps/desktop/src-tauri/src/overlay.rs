//! 補足カードの境界。パイプラインは [`Overlay::show_suggestion`] を呼び、
//! 本人の反応は [`OverlayEvent`] として channel で受け取る。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, PoisonError};

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Runtime, State, WebviewWindow,
};
use tauri_plugin_opener::OpenerExt;
use zuchi_core::types::{ProposedAction, Reaction, Suggestion};

use crate::payload::{Pending, ReactionPayload, safe_https_url, to_payload};

pub const WINDOW_LABEL: &str = "overlay";
/// 論理ピクセル。`style.css` の `.stack` の幅と合わせる。
const WINDOW_WIDTH: f64 = 360.0;
/// 画面の端との間隔(論理ピクセル)。
const SCREEN_MARGIN: f64 = 6.0;

/// 本人の操作の結果。記憶の `remember_reaction` に渡せるよう、補足そのものを添える。
#[derive(Debug)]
pub enum OverlayEvent {
    Reaction {
        suggestion: Suggestion,
        reaction: Reaction,
    },
    /// 本人が実行してよいと示した操作。実行はこのイベントを受けた側が行う
    ActionAccepted { action: ProposedAction },
}

/// Tauri の state として持つ。
pub struct Overlay {
    pending: Mutex<Pending>,
    events: Sender<OverlayEvent>,
    paused: AtomicBool,
    ready: AtomicBool,
}

impl Overlay {
    pub fn new(events: Sender<OverlayEvent>) -> Self {
        Self {
            pending: Mutex::new(Pending::default()),
            events,
            // 実際に聞き始めるまでは「聴取中」と表示しない
            paused: AtomicBool::new(true),
            ready: AtomicBool::new(false),
        }
    }

    /// 一時停止中か。音声と画面の取得はこれを見て止める。
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 取得を始められる状態か。
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    /// 補足を画面へ出す。一時停止中は出さず `None` を返す。
    pub fn show_suggestion<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        suggestion: Suggestion,
    ) -> tauri::Result<Option<u64>> {
        if self.is_paused() {
            return Ok(None);
        }
        let id = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(suggestion.clone());
        // イベントの名前は `src/bridge/tauri.ts` と合わせる
        app.emit_to(WINDOW_LABEL, "show-suggestion", to_payload(id, &suggestion))?;
        Ok(Some(id))
    }

    fn take(&self, id: u64) -> Option<Suggestion> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take(id)
    }

    fn send(&self, event: OverlayEvent) -> Result<(), String> {
        self.events
            .send(event)
            .map_err(|_| "イベントの受け手がいない".to_string())
    }
}

/// 起動時の設定。補足が来るまでクリックは背後へ抜ける。
pub fn prepare_window<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    // 設定ファイルの contentProtected に加えて、起動後にも指定しておく
    window.set_content_protected(true)?;
    window.set_ignore_cursor_events(true)?;
    place_top_right(window, 1.0)
}

/// 主画面の作業領域の右上に、カードの大きさで置く。メニューバーとタスクバーは避ける。
fn place_top_right<R: Runtime>(window: &WebviewWindow<R>, height: f64) -> tauri::Result<()> {
    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let margin = (SCREEN_MARGIN * scale) as i32;
    let width = (WINDOW_WIDTH * scale) as u32;
    let height = ((height * scale).ceil() as u32).min(area.size.height);
    window.set_size(PhysicalSize::new(width, height))?;
    window.set_position(PhysicalPosition::new(
        area.position.x + area.size.width as i32 - width as i32 - margin,
        area.position.y + margin,
    ))
}

/// 画面が描いている高さを受け取る。0 ならクリックを背後へ通す。
#[tauri::command]
pub fn set_overlay_height<R: Runtime>(app: AppHandle<R>, height: f64) -> Result<(), String> {
    let window = app
        .get_webview_window(WINDOW_LABEL)
        .ok_or("オーバーレイのウィンドウがない")?;
    let result = if height > 0.0 {
        place_top_right(&window, height).and_then(|()| window.set_ignore_cursor_events(false))
    } else {
        window.set_ignore_cursor_events(true)
    };
    result.map_err(|error| error.to_string())
}

#[tauri::command]
pub fn report_reaction(
    overlay: State<'_, Overlay>,
    id: u64,
    reaction: ReactionPayload,
) -> Result<(), String> {
    // 片付いた補足への反応は捨てる
    let Some(suggestion) = overlay.take(id) else {
        return Ok(());
    };
    overlay.send(OverlayEvent::Reaction {
        suggestion,
        reaction: reaction.into(),
    })
}

/// 提案の操作を受け付ける。操作の中身は画面から受け取らず、出した補足のものを使う。
/// 受け付けは「役立った」を兼ねる。
#[tauri::command]
pub fn accept_action(overlay: State<'_, Overlay>, id: u64) -> Result<(), String> {
    let suggestion = overlay.take(id).ok_or("この補足はもう片付いている")?;
    let action = suggestion.action.clone().ok_or("この補足に操作はない")?;
    overlay.send(OverlayEvent::ActionAccepted { action })?;
    overlay.send(OverlayEvent::Reaction {
        suggestion,
        reaction: Reaction::Helpful,
    })
}

/// 既定のブラウザで開く。画面側の確認に頼らず、ここでも https に絞る。
#[tauri::command]
pub fn open_external<R: Runtime>(app: AppHandle<R>, url: String) -> Result<(), String> {
    let url = safe_https_url(&url).ok_or("https のリンクだけを開ける")?;
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|error| error.to_string())
}
