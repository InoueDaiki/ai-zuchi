//! 実際の画面で動くかを本人が確かめる。読んだ文字と画像は表示も保存もせず、回数だけを出す。
//!
//! ```text
//! cargo run -p zuchi-screen --release --example screen_check -- [秒数]
//! cargo run -p zuchi-screen --release --example screen_check -- --request   # OS の確認を出す
//! ```
//!
//! 手順は docs/検証-画面.md を参照。

use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use zuchi_core::ports::ScreenSource;
use zuchi_screen::{ScreenConfig, is_permission_denied, platform_source};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    #[cfg(target_os = "macos")]
    if args.iter().any(|a| a == "--request") {
        // 本人が明示的に頼んだときだけ OS の確認を出す
        let granted = zuchi_screen::macos::request_permission();
        println!("権限: {}", if granted { "あり" } else { "なし" });
        return ExitCode::SUCCESS;
    }

    let seconds: u64 = args.first().and_then(|v| v.parse().ok()).unwrap_or(30);
    let mut source = platform_source(ScreenConfig::default());
    let stats = source.stats();
    let (tx, rx) = mpsc::channel();
    let handle = match source.start(tx) {
        Ok(handle) => handle,
        Err(error) if is_permission_denied(&error) => {
            eprintln!("権限エラー: {error}");
            return ExitCode::from(2);
        }
        Err(error) => {
            eprintln!("起動できません: {error}");
            return ExitCode::FAILURE;
        }
    };

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut characters = 0;
    while Instant::now() < deadline {
        if let Ok(text) = rx.recv_timeout(Duration::from_millis(200)) {
            characters += text.text.chars().count();
        }
    }
    handle.stop();

    println!(
        "周期 {} 回、取得 {} 回、対象外などで読まず {} 回、OCR {} 回、流した数 {}({} 文字)、失敗 {} 回",
        stats.ticks.load(Ordering::Relaxed),
        stats.captures.load(Ordering::Relaxed),
        stats.skipped.load(Ordering::Relaxed),
        stats.ocr_runs.load(Ordering::Relaxed),
        stats.emitted.load(Ordering::Relaxed),
        characters,
        stats.errors.load(Ordering::Relaxed),
    );
    if let Some(error) = source.fatal_error() {
        eprintln!("途中で止まりました: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
