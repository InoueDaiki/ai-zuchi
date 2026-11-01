//! ベクトル検索の所要時間の計測。普段は動かさない。
//!
//! `cargo test --release -p zuchi-memory --test bench -- --ignored --nocapture`

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use zuchi_core::ports::{Llm, LlmRequest, MemoryStore, Result};
use zuchi_core::types::{AudioChannel, Utterance};
use zuchi_memory::{MemoryConfig, MemoryKey, SqliteMemory};

const DIM: usize = 1024;

/// 毎回違う擬似乱数のベクトルを返す
struct RandomLlm(Mutex<u64>);

impl Llm for RandomLlm {
    fn complete(&self, _request: &LlmRequest) -> Result<String> {
        Err("使わない".into())
    }

    fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        let mut state = self.0.lock().unwrap();
        Ok((0..DIM)
            .map(|_| {
                *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (*state >> 40) as f32 / (1u64 << 24) as f32 - 0.5
            })
            .collect())
    }
}

#[test]
#[ignore = "計測用"]
fn 全件コサインの所要時間() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.sqlite");
    let memory = SqliteMemory::open(
        &path,
        &MemoryKey::from_bytes([3; 32]),
        Arc::new(RandomLlm(Mutex::new(1))),
        MemoryConfig {
            retention: None,
            ..Default::default()
        },
    )
    .unwrap();
    let now = SystemTime::now();
    let mut count = 0;
    for target in [10_000, 50_000, 150_000] {
        while count < target {
            memory
                .remember_utterance(&Utterance {
                    channel: AudioChannel::Mic,
                    text: format!("測定用の架空の発言です 番号 {count}"),
                    started_at: now - Duration::from_secs(count),
                    ended_at: now,
                })
                .unwrap();
            count += 1;
        }
        let started = Instant::now();
        for _ in 0..5 {
            memory.recall("測定", 5).unwrap();
        }
        let size = std::fs::metadata(&path).unwrap().len() >> 20;
        println!(
            "{count} 件、{DIM} 次元: 1 回 {:?}、ファイル {size} MB",
            started.elapsed() / 5
        );
    }
}
