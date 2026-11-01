//! 数秒取得して、フレーム数とピーク音量を表示する確認用のプログラム。音声は保存しない。
//!
//! ```text
//! cargo run -p zuchi-audio --example capture_check -- system --seconds 5 --play
//! ```
//!
//! - 第 1 引数: `mic` または `system`
//! - `--seconds N`: 取得する秒数(既定は 5)
//! - `--play`: 取得中に効果音を鳴らす(macOS)。`system` で無音の原因を見分けるのに使う
//!
//! 終了コード: 0 音が取れた、1 無音または取得の失敗、2 権限が原因、64 引数の誤り

use std::process::{Child, ExitCode};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use zuchi_audio::{
    Diagnosis, FRAME_SAMPLES, Level, MicSource, SystemSource, TARGET_RATE, diagnose, permission,
};
use zuchi_core::ports::AudioSource;
use zuchi_core::types::AudioChannel;

struct Options {
    channel: AudioChannel,
    seconds: u64,
    play: bool,
}

fn parse() -> Option<Options> {
    let mut args = std::env::args().skip(1);
    let channel = match args.next()?.as_str() {
        "mic" => AudioChannel::Mic,
        "system" => AudioChannel::System,
        _ => return None,
    };
    let mut options = Options {
        channel,
        seconds: 5,
        play: false,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seconds" => options.seconds = args.next()?.parse().ok()?,
            "--play" => options.play = true,
            _ => return None,
        }
    }
    Some(options)
}

#[cfg(target_os = "macos")]
fn play_sound() -> Option<Child> {
    std::process::Command::new("afplay")
        .arg("/System/Library/Sounds/Ping.aiff")
        .spawn()
        .ok()
}

#[cfg(not(target_os = "macos"))]
fn play_sound() -> Option<Child> {
    None
}

fn main() -> ExitCode {
    let Some(options) = parse() else {
        eprintln!("使い方: capture_check <mic|system> [--seconds N] [--play]");
        return ExitCode::from(64);
    };

    let status = permission(options.channel);
    println!("権限の状態: {status:?}");

    let (tx, rx) = channel();
    let mut source: Box<dyn AudioSource> = match options.channel {
        AudioChannel::Mic => Box::new(MicSource::new()),
        AudioChannel::System => Box::new(SystemSource::new()),
    };
    let handle = match source.start(tx) {
        Ok(handle) => handle,
        Err(error) => {
            println!("開始に失敗: {error}");
            return ExitCode::from(1);
        }
    };

    let mut player = None;
    let mut level = Level::default();
    let started = Instant::now();
    let until = started + Duration::from_secs(options.seconds);
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        // 開始の直後は無音のことがあるので、少し待ってから鳴らす
        if options.play && player.is_none() && started.elapsed() > Duration::from_millis(500) {
            player = play_sound();
        }
        match rx.recv_timeout(left.min(Duration::from_millis(100))) {
            Ok(frame) => level.add(&frame.samples),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                println!("取得が途中で終わりました");
                break;
            }
        }
    }
    handle.stop();
    let played = player.is_some_and(|mut child| child.wait().is_ok_and(|s| s.success()));
    rx.try_iter().for_each(|frame| level.add(&frame.samples));

    let seconds = (level.frames * FRAME_SAMPLES) as f64 / f64::from(TARGET_RATE);
    println!("フレーム数: {}(約 {seconds:.1} 秒)", level.frames);
    if level.peak > 0.0 {
        let db = 20.0 * level.peak.log10();
        println!("ピーク: {:.4}({db:.1} dBFS)", level.peak);
    } else {
        println!("ピーク: 0(完全な無音)");
    }

    let (message, code) = match diagnose(level, status, played) {
        Diagnosis::Sound => ("結果: 音が取れています", 0),
        Diagnosis::NoFrames => (
            "結果: 音を鳴らしたのにフレームが届きません。出力デバイスを確認してください",
            1,
        ),
        Diagnosis::SilentPermissionDenied => (
            "結果: 権限が拒否されていて無音です。docs/検証-音声取得.md の手順で許可してください",
            2,
        ),
        Diagnosis::SilentPermissionSuspected => (
            "結果: 音を鳴らしたのに無音です。権限がない可能性が高いです。docs/検証-音声取得.md を確認してください",
            2,
        ),
        Diagnosis::Silent => (
            "結果: 無音です。音が鳴っていないだけかもしれません。--play をつけて見分けてください",
            1,
        ),
    };
    println!("{message}");
    ExitCode::from(code)
}
