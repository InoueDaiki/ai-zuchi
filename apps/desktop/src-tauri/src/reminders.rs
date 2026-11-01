//! リマインドの実行、期限の確認、OS の通知。データベースを待つので専用のスレッドで動かす。
//!
//! 通知はプラグインの Rust の API だけで出す。デスクトップでは許可の状態を読めず、届かなかった
//! ことも分からない(プラグインは常に許可済みと返し、失敗を捨てる)。本人が通知を切っていても
//! 気づけるよう、期限切れはメニューバーの一覧にも出す。

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

use tauri::{AppHandle, Runtime};
use tauri_plugin_notification::NotificationExt;
use zuchi_core::types::ProposedAction;
use zuchi_runtime::reminders::{Reminder, Reminders};

use crate::tray;

/// 期限を確かめる間隔。通知はこの分だけ遅れることがある
const CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// これより多く同時に期限が来たら、1 件にまとめて知らせる。止まっていた間の分で通知が連なるのを防ぐ
const MAX_SEPARATE_NOTICES: usize = 3;
/// 通知とメニューに出す題名の最大の文字数
const MAX_TITLE_CHARS: usize = 60;
const NOTICE_TITLE: &str = "リマインド";

#[derive(Debug)]
pub enum ReminderCommand {
    /// 本人が受け付けた操作を実行する
    Perform(ProposedAction),
    /// 本人がメニューから完了にした
    Complete(i64),
}

/// リマインドのスレッドの受付。Tauri の state として持つ。
pub struct ReminderInbox(Sender<ReminderCommand>);

impl ReminderInbox {
    pub fn new(commands: Sender<ReminderCommand>) -> Self {
        Self(commands)
    }

    pub fn perform(&self, action: ProposedAction) {
        self.send(ReminderCommand::Perform(action));
    }

    pub fn complete(&self, id: i64) {
        self.send(ReminderCommand::Complete(id));
    }

    fn send(&self, command: ReminderCommand) {
        // 取得の準備に失敗すると受け手がいない。題名はログに出さない
        if self.0.send(command).is_err() {
            eprintln!("リマインドの操作を受け取れる状態ではない");
        }
    }
}

pub fn spawn<R: Runtime>(
    app: AppHandle<R>,
    reminders: Arc<Reminders>,
    commands: Receiver<ReminderCommand>,
) {
    thread::spawn(move || run(&app, &reminders, &commands));
}

fn run<R: Runtime>(
    app: &AppHandle<R>,
    reminders: &Reminders,
    commands: &Receiver<ReminderCommand>,
) {
    // 印をつけられなかったものを、この起動の間に繰り返し知らせない
    let mut announced = HashSet::new();
    let mut list_changed = true;
    loop {
        // 最初の一周で、起動していない間に過ぎたものも知らせる
        list_changed |= announce_due(app, reminders, &mut announced);
        if list_changed {
            let now = SystemTime::now();
            match reminders.open() {
                Ok(open) => {
                    tray::show_reminders(app, &open, now);
                    list_changed = false;
                }
                Err(_) => eprintln!("リマインドを読めなかった"),
            }
        }
        match commands.recv_timeout(CHECK_INTERVAL) {
            Ok(ReminderCommand::Perform(action)) => {
                if reminders.perform(action, SystemTime::now()).is_err() {
                    // カードは受け付けたと表示しているので、失敗は本人に伝える
                    eprintln!("リマインドをセットできなかった");
                    let notice = Notice {
                        title: NOTICE_TITLE.into(),
                        body: "リマインドをセットできませんでした".into(),
                    };
                    if show(app, &notice).is_err() {
                        eprintln!("通知を出せなかった");
                    }
                }
                list_changed = true;
            }
            Ok(ReminderCommand::Complete(id)) => {
                if reminders.complete(id, SystemTime::now()).is_err() {
                    eprintln!("リマインドを完了にできなかった");
                }
                list_changed = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// 期限が来たものを知らせる。知らせるものがあれば `true` を返す。
fn announce_due<R: Runtime>(
    app: &AppHandle<R>,
    reminders: &Reminders,
    announced: &mut HashSet<i64>,
) -> bool {
    let now = SystemTime::now();
    let Ok(mut due) = reminders.due(now) else {
        eprintln!("期限のリマインドを読めなかった");
        return false;
    };
    due.retain(|reminder| !announced.contains(&reminder.id));
    if due.is_empty() {
        return false;
    }
    if notices(&due)
        .iter()
        .any(|notice| show(app, notice).is_err())
    {
        // 印をつけず、次の確認でもう一度知らせる
        eprintln!("通知を出せなかった");
        return true;
    }
    let ids: Vec<i64> = due.iter().map(|reminder| reminder.id).collect();
    announced.extend(&ids);
    if reminders.mark_notified(&ids, now).is_err() {
        eprintln!("通知の記録に失敗した");
    }
    true
}

fn show<R: Runtime>(app: &AppHandle<R>, notice: &Notice) -> tauri_plugin_notification::Result<()> {
    app.notification()
        .builder()
        .title(&notice.title)
        .body(&notice.body)
        .show()
}

#[derive(Debug, PartialEq, Eq)]
struct Notice {
    title: String,
    body: String,
}

fn notices(due: &[Reminder]) -> Vec<Notice> {
    if due.len() > MAX_SEPARATE_NOTICES {
        return vec![Notice {
            title: NOTICE_TITLE.into(),
            body: format!(
                "期限が来たリマインドが {} 件あります。メニューバーの「リマインド一覧」で確かめられます",
                due.len()
            ),
        }];
    }
    due.iter()
        .map(|reminder| Notice {
            title: NOTICE_TITLE.into(),
            body: display_title(&reminder.title),
        })
        .collect()
}

/// 題名を 1 行の文字列に直す。題名は画面や会話から作られ、他人が書けるので、
/// 改行や制御文字、文字の向きを変える書式の文字で見た目をごまかせないようにする。
pub fn display_title(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control() && !is_format_char(*c))
        .collect();
    let words: Vec<&str> = cleaned.split(' ').filter(|word| !word.is_empty()).collect();
    let line = words.join(" ");
    if line.is_empty() {
        return "(題名なし)".into();
    }
    if line.chars().count() <= MAX_TITLE_CHARS {
        return line;
    }
    let mut short: String = line.chars().take(MAX_TITLE_CHARS - 1).collect();
    short.push('…');
    short
}

/// 見えないまま表示を変える文字(幅のない文字、文字の向きの指定)。
fn is_format_char(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reminder(id: i64, title: &str) -> Reminder {
        Reminder {
            id,
            title: title.into(),
            due: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn 題名を_1_行の文字列にする() {
        assert_eq!(
            display_title(" 資料の\n確認\t\tお願いします "),
            "資料の 確認 お願いします"
        );
        assert_eq!(display_title("a\u{202E}gnp.exe\u{0007}"), "agnp.exe");
        assert_eq!(display_title("<b>&amp;</b>"), "<b>&amp;</b>");
        assert_eq!(display_title(" \n\u{200B}"), "(題名なし)");
    }

    #[test]
    fn 長い題名は切り詰める() {
        let line = display_title(&"あ".repeat(100));
        assert_eq!(line.chars().count(), MAX_TITLE_CHARS);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn 少なければ_1_件ずつ知らせる() {
        let due = [reminder(1, "資料の確認"), reminder(2, "返信")];
        let bodies: Vec<_> = notices(&due).into_iter().map(|n| n.body).collect();
        assert_eq!(bodies, ["資料の確認", "返信"]);
    }

    #[test]
    fn 多ければまとめて知らせて題名を並べない() {
        let due: Vec<_> = (0..=MAX_SEPARATE_NOTICES as i64)
            .map(|id| reminder(id, "資料の確認"))
            .collect();
        let notices = notices(&due);
        assert_eq!(notices.len(), 1);
        assert!(notices[0].body.contains("4 件"));
        assert!(!notices[0].body.contains("資料の確認"));
    }
}
