//! メニューバー(Windows は通知領域)の常駐。聞いているかどうかを常に見せる。

use std::time::SystemTime;
use tauri::image::Image;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};
use zuchi_runtime::reminders::Reminder;

use crate::overlay::Overlay;
use crate::reminders::{ReminderInbox, display_title};

const TOGGLE_ID: &str = "toggle";
const QUIT_ID: &str = "quit";
#[cfg(debug_assertions)]
const FAKE_ID: &str = "dev-fake";
/// 続けて数値のリマインドの ID を置く。題名は ID に入れない
const REMINDER_DONE_PREFIX: &str = "reminder-done:";
/// メニューに並べる件数の上限。多いと画面に収まらない
const MAX_REMINDER_ROWS: usize = 20;

const ICON_SIZE: u32 = 44;

/// 状態を変えるときに一緒に直す部品。
struct TrayParts<R: Runtime> {
    icon: TrayIcon<R>,
    status: MenuItem<R>,
    toggle: MenuItem<R>,
    reminders: Submenu<R>,
}

/// 「リマインド一覧」の 1 行。
#[derive(Debug, PartialEq, Eq)]
enum ReminderRow {
    /// 押せない説明の行
    Note(String),
    /// 押すと完了にする行
    Done { id: i64, label: String },
}

/// 未完了のものを行にする。`reminders` は期限の早い順に並んでいる。
/// 通知の許可がなくても気づけるよう、期限切れはここでも分かるようにする。
fn reminder_rows(reminders: &[Reminder], now: SystemTime) -> Vec<ReminderRow> {
    if reminders.is_empty() {
        return vec![ReminderRow::Note("未完了のリマインドはありません".into())];
    }
    let mut rows = vec![ReminderRow::Note("押すと完了にします".into())];
    rows.extend(reminders.iter().take(MAX_REMINDER_ROWS).map(|reminder| {
        let title = menu_text(&display_title(&reminder.title));
        ReminderRow::Done {
            id: reminder.id,
            label: if reminder.due <= now {
                format!("期限切れ: {title}")
            } else {
                title
            },
        }
    }));
    let rest = reminders.len().saturating_sub(MAX_REMINDER_ROWS);
    if rest > 0 {
        rows.push(ReminderRow::Note(format!("ほか {rest} 件")));
    }
    rows
}

/// メニューの文字列で `&` はショートカットの印になるので、文字として出るようにする。
fn menu_text(text: &str) -> String {
    text.replace('&', "&&")
}

fn reminder_menu_id(id: i64) -> String {
    format!("{REMINDER_DONE_PREFIX}{id}")
}

fn parse_reminder_menu_id(menu_id: &str) -> Option<i64> {
    menu_id.strip_prefix(REMINDER_DONE_PREFIX)?.parse().ok()
}

fn apply_reminders<R: Runtime>(
    app: &AppHandle<R>,
    submenu: &Submenu<R>,
    rows: &[ReminderRow],
) -> tauri::Result<()> {
    for item in submenu.items()? {
        submenu.remove(&item)?;
    }
    for row in rows {
        let item = match row {
            ReminderRow::Note(text) => MenuItem::new(app, text, false, None::<&str>)?,
            ReminderRow::Done { id, label } => {
                MenuItem::with_id(app, reminder_menu_id(*id), label, true, None::<&str>)?
            }
        };
        submenu.append(&item)?;
    }
    Ok(())
}

/// 「リマインド一覧」を今の内容に置き換える。
pub fn show_reminders<R: Runtime>(app: &AppHandle<R>, reminders: &[Reminder], now: SystemTime) {
    let rows = reminder_rows(reminders, now);
    let submenu = &app.state::<TrayParts<R>>().reminders;
    if let Err(error) = apply_reminders(app, submenu, &rows) {
        eprintln!("リマインド一覧を更新できなかった: {error}");
    }
}

fn status_text(paused: bool) -> &'static str {
    if paused {
        "一時停止中(聞いていません)"
    } else {
        "聞いています"
    }
}

fn toggle_text(paused: bool) -> &'static str {
    if paused { "再開" } else { "一時停止" }
}

/// 色だけに頼らず形でも区別する。
/// 聞いている間は塗りの円に音の棒、一時停止中は輪に斜線。
fn icon_rgba(paused: bool, color: [u8; 3]) -> Vec<u8> {
    const SAMPLES: u32 = 4;
    let size = ICON_SIZE as f32;
    let center = size / 2.0;
    let radius = size / 2.0 - 2.0;
    let covered = |x: f32, y: f32| -> bool {
        let (dx, dy) = (x - center, y - center);
        let distance = (dx * dx + dy * dy).sqrt();
        if paused {
            let ring = (radius - 5.0..=radius).contains(&distance);
            let slash = (dx - dy).abs() / std::f32::consts::SQRT_2 <= 2.5 && distance <= radius;
            ring || slash
        } else {
            // 中央の 3 本の棒を抜く
            let bar = [(-9.0, 5.0), (0.0, 11.0), (9.0, 7.0)]
                .iter()
                .any(|(offset, half)| (dx - offset).abs() <= 2.0 && dy.abs() <= *half);
            distance <= radius && !bar
        }
    };

    let mut rgba = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let mut hits = 0;
            for sub_y in 0..SAMPLES {
                for sub_x in 0..SAMPLES {
                    let offset = |sub: u32| (sub as f32 + 0.5) / SAMPLES as f32;
                    if covered(x as f32 + offset(sub_x), y as f32 + offset(sub_y)) {
                        hits += 1;
                    }
                }
            }
            rgba.extend_from_slice(&color);
            rgba.push((hits * 255 / (SAMPLES * SAMPLES)) as u8);
        }
    }
    rgba
}

fn icon_image(paused: bool) -> Image<'static> {
    // macOS はテンプレート画像として使うので、色は透明度だけが効く。
    // Windows は明暗どちらの通知領域でも見えるよう、色をつける
    let color = match (cfg!(target_os = "macos"), paused) {
        (true, _) => [0, 0, 0],
        (false, false) => [46, 160, 110],
        (false, true) => [140, 140, 140],
    };
    Image::new_owned(icon_rgba(paused, color), ICON_SIZE, ICON_SIZE)
}

fn apply_state<R: Runtime>(parts: &TrayParts<R>, paused: bool) -> tauri::Result<()> {
    parts
        .icon
        .set_icon_with_as_template(Some(icon_image(paused)), cfg!(target_os = "macos"))?;
    // アイコンの形だけでなく、言葉でも分かるようにする(macOS のみ表示される)
    parts
        .icon
        .set_title(Some(if paused { "停止中" } else { "聴取中" }))?;
    parts
        .icon
        .set_tooltip(Some(format!("ai-zuchi: {}", status_text(paused))))?;
    parts.status.set_text(status_text(paused))?;
    parts.toggle.set_text(toggle_text(paused))
}

fn toggle<R: Runtime>(app: &AppHandle<R>) {
    let overlay = app.state::<Overlay>();
    let paused = !overlay.is_paused();
    // 取得の準備ができていないのに「聴取中」と表示しない
    if !paused && !overlay.is_ready() {
        return;
    }
    // 表示の更新に失敗しても、止める方を先に確実に行う
    overlay.set_paused(paused);
    if let Err(error) = apply_state(&app.state::<TrayParts<R>>(), paused) {
        eprintln!("メニューバーの表示を更新できなかった: {error}");
    }
}

/// 一時停止の状態を外から変えたあとに、表示を合わせる。
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let paused = app.state::<Overlay>().is_paused();
    if let Err(error) = apply_state(&app.state::<TrayParts<R>>(), paused) {
        eprintln!("メニューバーの表示を更新できなかった: {error}");
    }
}

pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let paused = app.state::<Overlay>().is_paused();
    let status = MenuItem::new(app, status_text(paused), false, None::<&str>)?;
    let toggle_item = MenuItem::with_id(app, TOGGLE_ID, toggle_text(paused), true, None::<&str>)?;
    let reminders = Submenu::with_items(
        app,
        "リマインド一覧",
        true,
        // 保存先を開くまでの間、「ありません」と誤って見せない
        &[&MenuItem::new(app, "読み込み中", false, None::<&str>)?],
    )?;
    let quit = MenuItem::with_id(app, QUIT_ID, "終了", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &toggle_item,
            &reminders,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    #[cfg(debug_assertions)]
    menu.insert(
        &MenuItem::with_id(app, FAKE_ID, "偽の補足を出す(開発用)", true, None::<&str>)?,
        2,
    )?;

    let icon = TrayIconBuilder::with_id("main")
        .icon(icon_image(paused))
        .icon_as_template(cfg!(target_os = "macos"))
        .menu(&menu)
        // 左クリックは一時停止の切り替え。メニューは右クリックで開く
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            TOGGLE_ID => toggle(app),
            QUIT_ID => app.exit(0),
            #[cfg(debug_assertions)]
            FAKE_ID => crate::dev::show_next(app),
            other => {
                if let Some(id) = parse_reminder_menu_id(other) {
                    app.state::<ReminderInbox>().complete(id);
                }
            }
        })
        .build(app)?;

    let parts = TrayParts {
        icon,
        status,
        toggle: toggle_item,
        reminders,
    };
    apply_state(&parts, paused)?;
    app.manage(parts);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn reminder(id: i64, title: &str, due: SystemTime) -> Reminder {
        Reminder {
            id,
            title: title.into(),
            due,
        }
    }

    #[test]
    fn 未完了がなければ押せる行を出さない() {
        assert_eq!(
            reminder_rows(&[], SystemTime::now()),
            vec![ReminderRow::Note("未完了のリマインドはありません".into())]
        );
    }

    #[test]
    fn 題名は文字のまま出して期限切れが分かる() {
        let now = SystemTime::now();
        let rows = reminder_rows(
            &[
                reminder(1, "A&B\tCtrl+Q", now),
                reminder(2, "資料の確認", now + Duration::from_secs(60)),
            ],
            now,
        );
        assert_eq!(
            rows[1..],
            [
                ReminderRow::Done {
                    id: 1,
                    label: "期限切れ: A&&B Ctrl+Q".into(),
                },
                ReminderRow::Done {
                    id: 2,
                    label: "資料の確認".into(),
                },
            ]
        );
    }

    #[test]
    fn 多すぎる分は件数だけ出す() {
        let now = SystemTime::now();
        let many: Vec<_> = (0..23).map(|id| reminder(id, "用事", now)).collect();
        let rows = reminder_rows(&many, now);
        assert_eq!(rows.len(), MAX_REMINDER_ROWS + 2);
        assert_eq!(rows.last(), Some(&ReminderRow::Note("ほか 3 件".into())));
    }

    #[test]
    fn メニューの_id_は完了の操作だけを読む() {
        assert_eq!(parse_reminder_menu_id(&reminder_menu_id(12)), Some(12));
        assert_eq!(parse_reminder_menu_id("reminder-done:x"), None);
        assert_eq!(parse_reminder_menu_id(TOGGLE_ID), None);
    }

    #[test]
    fn 状態ごとにアイコンの形が違う() {
        let listening = icon_rgba(false, [0, 0, 0]);
        let paused = icon_rgba(true, [0, 0, 0]);
        assert_eq!(listening.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        assert_ne!(listening, paused);
    }

    #[test]
    fn 一時停止の文言で聞いていないと分かる() {
        assert!(status_text(true).contains("聞いていません"));
        assert_eq!(toggle_text(true), "再開");
        assert_eq!(toggle_text(false), "一時停止");
    }
}
