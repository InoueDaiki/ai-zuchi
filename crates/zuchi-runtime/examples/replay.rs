//! 文字起こしの段階から台本を流して、補足を確かめる。
//!
//! `cargo run -p zuchi-runtime --example replay -- <台本> <チャットのモデル> <埋め込みのモデル>`
//!
//! 台本は 1 行に 1 発話で、`自分:` か `相手:` で始める。一時ディレクトリの記憶を使う。

use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use zuchi_core::types::{AudioChannel, ScreenText, Utterance};
use zuchi_memory::MemoryKey;
use zuchi_runtime::{Runtime, RuntimeConfig};

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, script, chat_model, embed_model] = args.as_slice() else {
        return Err("台本、チャットのモデル、埋め込みのモデルを指定する".into());
    };
    let data_dir = std::env::temp_dir().join(format!("zuchi-replay-{}", std::process::id()));
    let (suggestions, received) = mpsc::channel();
    let runtime = Runtime::start(
        RuntimeConfig {
            data_dir: data_dir.clone(),
            models_dir: "models".into(),
            chat_model: chat_model.clone(),
            embed_model: embed_model.clone(),
            blocked_terms: vec![],
            listen_screen: false,
        },
        MemoryKey::from_bytes([7; 32]),
        suggestions,
    )?;

    for line in std::fs::read_to_string(script)?.lines() {
        let Some((speaker, text)) = line.split_once(':') else {
            continue;
        };
        let at = SystemTime::now();
        if speaker == "画面" {
            runtime.inject_screen(ScreenText {
                app: String::new(),
                window_title: String::new(),
                text: text.trim().to_string(),
                captured_at: at,
            });
            println!("{line}");
            match received.recv_timeout(Duration::from_secs(45)) {
                Ok(suggestion) => println!("  -> {} {:?}", suggestion.text, suggestion.evidence),
                Err(_) => println!("  -> (黙る)"),
            }
            continue;
        }
        runtime.inject(Utterance {
            channel: if speaker == "自分" {
                AudioChannel::Mic
            } else {
                AudioChannel::System
            },
            text: text.trim().to_string(),
            started_at: at,
            ended_at: at,
        });
        println!("{line}");
        match received.recv_timeout(Duration::from_secs(45)) {
            Ok(suggestion) => println!("  -> {} {:?}", suggestion.text, suggestion.evidence),
            Err(_) => println!("  -> (黙る)"),
        }
    }
    drop(runtime);
    let _ = std::fs::remove_dir_all(data_dir);
    Ok(())
}
