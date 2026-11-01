//! 暗号化されたファイルに対する、保存、検索、削除の検証。

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tempfile::TempDir;
use zuchi_core::ports::{Llm, LlmRequest, MemoryStore, Result};
use zuchi_core::types::{
    AudioChannel, Evidence, Reaction, ScreenText, Skill, Suggestion, Utterance,
};
use zuchi_memory::{MemoryConfig, MemoryError, MemoryKey, SqliteMemory};

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// 文字の 2-gram を数える偽の埋め込み。似た文字列は似たベクトルになる。
struct FakeLlm {
    offline: AtomicBool,
}

impl FakeLlm {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            offline: AtomicBool::new(false),
        })
    }
}

impl Llm for FakeLlm {
    fn complete(&self, _request: &LlmRequest) -> Result<String> {
        Err("使わない".into())
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        if self.offline.load(Ordering::SeqCst) {
            return Err("Ollama が止まっている".into());
        }
        let chars: Vec<char> = text.chars().collect();
        let mut vector = vec![0.0; 64];
        for pair in chars.windows(2) {
            let mut hasher = DefaultHasher::new();
            pair.hash(&mut hasher);
            vector[(hasher.finish() % 64) as usize] += 1.0;
        }
        Ok(vector)
    }
}

fn key() -> MemoryKey {
    MemoryKey::from_bytes([42; 32])
}

fn open(dir: &TempDir, llm: Arc<FakeLlm>, config: MemoryConfig) -> SqliteMemory {
    SqliteMemory::open(dir.path().join("memory.sqlite"), &key(), llm, config).unwrap()
}

fn utterance(text: &str, at: SystemTime) -> Utterance {
    Utterance {
        channel: AudioChannel::System,
        text: text.to_owned(),
        started_at: at,
        ended_at: at,
    }
}

/// 比べやすいよう、ミリ秒に丸めた現在時刻
fn now() -> SystemTime {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    UNIX_EPOCH + Duration::from_millis(ms as u64)
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

fn all_files(dir: &Path) -> Vec<u8> {
    fs::read_dir(dir)
        .unwrap()
        .flat_map(|entry| fs::read(entry.unwrap().path()).unwrap())
        .collect()
}

#[test]
fn 日本語の部分文字列で思い出す() {
    let dir = tempfile::tempdir().unwrap();
    let llm = FakeLlm::new();
    llm.offline.store(true, Ordering::SeqCst);
    let memory = open(&dir, llm, MemoryConfig::default());
    let at = now();
    memory
        .remember_utterance(&utterance("打ち合わせではA案に決定しました", at))
        .unwrap();
    memory
        .remember_utterance(&utterance("今日の昼食はカレーでした", at))
        .unwrap();

    let found = memory
        .recall("打ち合わせの結論はどうなったっけ", 5)
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].text, "打ち合わせではA案に決定しました");
    assert_eq!(
        found[0].evidence,
        Evidence::Utterance {
            text: "打ち合わせではA案に決定しました".to_owned(),
            at
        }
    );
}

#[test]
fn 二文字の語でも思い出す() {
    let dir = tempfile::tempdir().unwrap();
    let llm = FakeLlm::new();
    llm.offline.store(true, Ordering::SeqCst);
    let memory = open(&dir, llm, MemoryConfig::default());
    memory
        .remember_utterance(&utterance("価格は来週に決めます", now()))
        .unwrap();
    assert_eq!(memory.recall("価格", 5).unwrap().len(), 1);
}

#[test]
fn ベクトル検索だけでも見つかる() {
    let dir = tempfile::tempdir().unwrap();
    let memory = open(&dir, FakeLlm::new(), MemoryConfig::default());
    memory
        .remember_utterance(&utterance("かきくけこさしすせそ", now()))
        .unwrap();
    // 全文検索では一致しない(語を切り出すと全体が 1 語になり、部分文字列でない)
    let found = memory.recall("かきくけこさしすせそた", 5).unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].score > 0.0);
}

#[test]
fn 似ていないものはベクトル検索で返さない() {
    let dir = tempfile::tempdir().unwrap();
    let memory = open(&dir, FakeLlm::new(), MemoryConfig::default());
    memory
        .remember_utterance(&utterance("今日の昼食はカレーでした", now()))
        .unwrap();
    assert!(
        memory
            .recall("まったくかんけいないはなし", 5)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn ベクトル検索は新しいものから上限まで見る() {
    let dir = tempfile::tempdir().unwrap();
    let config = MemoryConfig {
        max_vector_scan: 1,
        ..Default::default()
    };
    let memory = open(&dir, FakeLlm::new(), config);
    memory
        .remember_utterance(&utterance("かきくけこさしすせそ", now()))
        .unwrap();
    memory
        .remember_utterance(&utterance("たちつてとなにぬねの", now()))
        .unwrap();
    assert!(
        memory
            .recall("かきくけこさしすせそた", 5)
            .unwrap()
            .is_empty()
    );
    assert_eq!(memory.recall("たちつてとなにぬねのは", 5).unwrap().len(), 1);
}

#[test]
fn 両方で当たるものを上にする() {
    let dir = tempfile::tempdir().unwrap();
    let memory = open(&dir, FakeLlm::new(), MemoryConfig::default());
    let at = now();
    memory.remember_utterance(&utterance("差別化", at)).unwrap();
    memory
        .remember_utterance(&utterance(
            "BONXの差別化の話ですが、長い前置きがあります",
            at,
        ))
        .unwrap();
    let found = memory.recall("差別化", 5).unwrap();
    assert_eq!(found[0].text, "差別化");
    assert!((found[0].score - 1.0).abs() < 0.01, "{}", found[0].score);
}

#[test]
fn 検索語の記号で壊れない() {
    let dir = tempfile::tempdir().unwrap();
    let memory = open(&dir, FakeLlm::new(), MemoryConfig::default());
    memory
        .remember_utterance(&utterance("100%の確率", now()))
        .unwrap();
    for query in [
        "\"",
        "a\" OR \"b",
        "NEAR(",
        "text:*",
        "%%",
        "'; DROP TABLE memories;--",
    ] {
        memory.recall(query, 5).unwrap();
    }
    assert_eq!(memory.recall("確率", 5).unwrap().len(), 1);
}

#[test]
fn 画面の文字は発言と区別できる() {
    let dir = tempfile::tempdir().unwrap();
    let llm = FakeLlm::new();
    llm.offline.store(true, Ordering::SeqCst);
    let memory = open(&dir, llm, MemoryConfig::default());
    memory
        .remember_screen(&ScreenText {
            app: "Mail".into(),
            window_title: "受信".into(),
            text: "こちらご確認ください".into(),
            captured_at: now(),
        })
        .unwrap();
    let found = memory.recall("確認", 5).unwrap();
    let Evidence::Utterance { text, .. } = &found[0].evidence else {
        panic!("{:?}", found[0].evidence);
    };
    assert_eq!(text, "[画面: Mail / 受信] こちらご確認ください");
}

#[test]
fn 埋め込みに失敗しても文字列は残り後で埋まる() {
    let dir = tempfile::tempdir().unwrap();
    let llm = FakeLlm::new();
    llm.offline.store(true, Ordering::SeqCst);
    let memory = open(&dir, llm.clone(), MemoryConfig::default());
    memory
        .remember_utterance(&utterance("かきくけこさしすせそ", now()))
        .unwrap();
    assert!(memory.embed_pending(10).is_err());

    llm.offline.store(false, Ordering::SeqCst);
    assert_eq!(memory.embed_pending(10).unwrap(), 1);
    assert_eq!(memory.embed_pending(10).unwrap(), 0);
    assert_eq!(memory.recall("かきくけこさしすせそた", 5).unwrap().len(), 1);
}

#[test]
fn 保持期間を過ぎたものを消す() {
    let dir = tempfile::tempdir().unwrap();
    let config = MemoryConfig {
        retention: Some(30 * DAY),
        ..Default::default()
    };
    let memory = open(&dir, FakeLlm::new(), config);
    let current = now();
    memory
        .remember_utterance(&utterance("古い決定事項の発言", current - 31 * DAY))
        .unwrap();
    memory
        .remember_utterance(&utterance("新しい決定事項の発言", current - DAY))
        .unwrap();
    memory
        .record_search("古い検索語", current - 31 * DAY)
        .unwrap();
    memory.record_search("新しい検索語", current).unwrap();

    assert_eq!(memory.purge_expired(current).unwrap(), 2);

    let found = memory.recall("決定事項", 5).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].text, "新しい決定事項の発言");
    assert_eq!(memory.search_history(10).unwrap().len(), 1);
    // 全文検索と埋め込みにも残らず、2 回目は消すものがない
    assert_eq!(memory.purge_expired(current).unwrap(), 0);
}

#[test]
fn 開くときにも期限切れを消す() {
    let dir = tempfile::tempdir().unwrap();
    let old = now() - 100 * DAY;
    {
        let memory = open(
            &dir,
            FakeLlm::new(),
            MemoryConfig {
                retention: None,
                ..Default::default()
            },
        );
        memory
            .remember_utterance(&utterance("古い決定事項の発言", old))
            .unwrap();
    }
    let memory = open(&dir, FakeLlm::new(), MemoryConfig::default());
    assert!(memory.recall("決定事項", 5).unwrap().is_empty());
}

#[test]
fn 反応を残す() {
    let dir = tempfile::tempdir().unwrap();
    let memory = open(&dir, FakeLlm::new(), MemoryConfig::default());
    let suggestion = Suggestion {
        skill: Skill::Calc,
        text: "秒間約127件ですね".into(),
        evidence: vec![Evidence::Assumption("3 カ月を 91 日とする".into())],
        action: None,
    };
    memory
        .remember_reaction(&suggestion, Reaction::Helpful)
        .unwrap();
    let records = memory.recent_reactions(5).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].skill, "calc");
    assert_eq!(records[0].reaction, "helpful");
}

#[test]
fn ディスクに平文を残さない() {
    let dir = tempfile::tempdir().unwrap();
    let secret = "CONFIDENTIAL-MARKER-社外秘の発言";
    let memory = open(&dir, FakeLlm::new(), MemoryConfig::default());
    memory
        .remember_utterance(&utterance(secret, now()))
        .unwrap();
    memory.record_search("SEARCH-MARKER", now()).unwrap();

    // 開いたまま(WAL がある状態)と、閉じた後の両方を確かめる
    let bytes = all_files(dir.path());
    assert!(!bytes.is_empty());
    assert!(!contains(&bytes, "CONFIDENTIAL-MARKER"));
    assert!(!contains(&bytes, "社外秘"));
    assert!(!contains(&bytes, "SEARCH-MARKER"));
    assert!(!contains(&bytes, "SQLite format 3"));
    drop(memory);
    let bytes = all_files(dir.path());
    assert!(!contains(&bytes, "CONFIDENTIAL-MARKER"));
    assert!(!contains(&bytes, "SQLite format 3"));
}

#[test]
fn 違う鍵では開けない() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.sqlite");
    drop(SqliteMemory::open(&path, &key(), FakeLlm::new(), MemoryConfig::default()).unwrap());

    let result = SqliteMemory::open(
        &path,
        &MemoryKey::from_bytes([1; 32]),
        FakeLlm::new(),
        MemoryConfig::default(),
    );
    assert!(matches!(result, Err(MemoryError::CannotDecrypt)));
}

#[test]
fn 閉じて開き直しても残る() {
    let dir = tempfile::tempdir().unwrap();
    {
        let memory = open(&dir, FakeLlm::new(), MemoryConfig::default());
        memory
            .remember_utterance(&utterance("決定事項の発言", now()))
            .unwrap();
    }
    let memory = open(&dir, FakeLlm::new(), MemoryConfig::default());
    assert_eq!(memory.recall("決定事項", 5).unwrap().len(), 1);
}

#[test]
fn 鍵を_debug_に出さない() {
    assert_eq!(format!("{:?}", key()), "MemoryKey(<隠す>)");
}

#[test]
fn 音声の列を持たない() {
    // 型の上でも音声を受け取らないが、BLOB の列が埋め込みだけであることも確かめる
    let dir = tempfile::tempdir().unwrap();
    let schema =
        zuchi_memory::schema_sql(&open(&dir, FakeLlm::new(), MemoryConfig::default())).unwrap();
    assert_eq!(schema.matches("BLOB").count(), 1, "{schema}");
    assert!(!schema.to_lowercase().contains("audio"));
    assert!(!schema.to_lowercase().contains("samples"));
}

#[test]
fn リマインドは期限が来たものを一度だけ返す() {
    let dir = TempDir::new().unwrap();
    let memory = open(&dir, FakeLlm::new(), MemoryConfig::default());
    let now = now();
    let later = memory.add_reminder("後の用事", now + DAY, now).unwrap();
    let due = memory.add_reminder(" 資料の確認 ", now - DAY, now).unwrap();
    assert_eq!(due.title, "資料の確認");

    assert_eq!(memory.due_reminders(now).unwrap(), vec![due.clone()]);
    // 早い順に並ぶ
    assert_eq!(
        memory.open_reminders().unwrap(),
        vec![due.clone(), later.clone()]
    );

    memory.mark_reminders_notified(&[due.id], now).unwrap();
    assert!(memory.due_reminders(now).unwrap().is_empty());
    // 知らせただけでは未完了のまま
    assert_eq!(memory.open_reminders().unwrap().len(), 2);
    assert_eq!(memory.due_reminders(now + DAY).unwrap(), vec![later]);
}

#[test]
fn 完了にしたリマインドは出てこない() {
    let dir = TempDir::new().unwrap();
    let memory = open(&dir, FakeLlm::new(), MemoryConfig::default());
    let now = now();
    let reminder = memory.add_reminder("確認", now, now).unwrap();

    assert!(memory.complete_reminder(reminder.id, now).unwrap());
    assert!(memory.due_reminders(now).unwrap().is_empty());
    assert!(memory.open_reminders().unwrap().is_empty());
    // 古いメニューから二度押されても、ないものを押されても失敗にしない
    assert!(!memory.complete_reminder(reminder.id, now).unwrap());
    assert!(!memory.complete_reminder(reminder.id + 1, now).unwrap());
}

#[test]
fn 空の題名のリマインドは作らない() {
    let dir = TempDir::new().unwrap();
    let memory = open(&dir, FakeLlm::new(), MemoryConfig::default());
    let now = now();
    assert!(memory.add_reminder("  ", now, now).is_err());
    assert!(memory.open_reminders().unwrap().is_empty());
}

#[test]
fn 再び開いても未通知のリマインドが残り本文は暗号化される() {
    let dir = TempDir::new().unwrap();
    let llm = FakeLlm::new();
    let at = now();
    let memory = open(&dir, llm.clone(), MemoryConfig::default());
    memory.add_reminder("秘密の納期の確認", at, at).unwrap();
    drop(memory);

    let file = fs::read(dir.path().join("memory.sqlite")).unwrap();
    assert!(!contains(&file, "秘密の納期の確認"));

    let memory = open(&dir, llm, MemoryConfig::default());
    let due = memory.due_reminders(at + DAY).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].title, "秘密の納期の確認");
}

#[test]
fn 保持期間で消えるのは完了したリマインドだけ() {
    let dir = TempDir::new().unwrap();
    let memory = open(
        &dir,
        FakeLlm::new(),
        MemoryConfig {
            retention: Some(DAY),
            ..MemoryConfig::default()
        },
    );
    let now = now();
    let old = now - 10 * DAY;
    let done = memory.add_reminder("済んだ用事", old, old).unwrap();
    memory.complete_reminder(done.id, old).unwrap();
    let open_one = memory.add_reminder("まだの用事", old, old).unwrap();

    assert_eq!(memory.purge_expired(now).unwrap(), 1);
    assert_eq!(memory.open_reminders().unwrap(), vec![open_one]);
}
