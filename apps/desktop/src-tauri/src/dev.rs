//! 開発時だけ、偽の補足を本物の境界に流す。リリースのビルドには含まれない。
//!
//! メニューの「偽の補足を出す」か、環境変数 `ZUCHI_FAKE_INTERVAL_SECS` の間隔で流す。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use tauri::{AppHandle, Manager, Runtime};
use zuchi_core::types::{Evidence, ProposedAction, Skill, Suggestion};

/// `src/bridge/mock.ts` と同じ内容。
fn samples() -> Vec<Suggestion> {
    vec![
        Suggestion {
            skill: Skill::Calc,
            text: "秒間約127件、1日約1,100万件ですね".into(),
            evidence: vec![Evidence::Assumption("3カ月を91日とする".into())],
            action: None,
        },
        Suggestion {
            skill: Skill::Search,
            text: "IP ボイスインカムの中で、ハンズフリー用途で人気のようです".into(),
            evidence: vec![
                Evidence::Web {
                    title: "BONX の製品情報".into(),
                    url: "https://bonx.co.jp/".into(),
                },
                // 悪意のある検索結果を模した例。文字として出て、リンクにならないことを見る
                Evidence::Web {
                    title: "<img src=x onerror=\"alert(1)\"> 外部の悪い題名".into(),
                    url: "javascript:alert(1)".into(),
                },
            ],
            action: None,
        },
        Suggestion {
            skill: Skill::Suggest,
            text: "リマインドをセットしますか?".into(),
            evidence: vec![],
            action: Some(ProposedAction::SetReminder {
                title: "資料の確認".into(),
                due: None,
            }),
        },
        Suggestion {
            skill: Skill::Memory,
            text: "10/20 の打ち合わせでは、A 案に決まっていました".into(),
            evidence: vec![Evidence::Utterance {
                text: "では A 案で進めましょう".into(),
                at: SystemTime::now() - Duration::from_secs(12 * 24 * 60 * 60),
            }],
            action: None,
        },
    ]
}

pub fn show_next<R: Runtime>(app: &AppHandle<R>) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let all = samples();
    let suggestion = all[NEXT.fetch_add(1, Ordering::Relaxed) % all.len()].clone();
    let overlay = app.state::<crate::overlay::Overlay>();
    if let Err(error) = overlay.show_suggestion(app, suggestion) {
        eprintln!("偽の補足を出せなかった: {error}");
    }
}

pub fn start_from_env<R: Runtime>(app: &AppHandle<R>) {
    let Some(seconds) = std::env::var("ZUCHI_FAKE_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
    else {
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(seconds));
            show_next(&app);
        }
    });
}
