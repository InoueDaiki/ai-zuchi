//! [`MemoryStore`] の SQLite 実装。
//!
//! - ファイル全体を SQLCipher で暗号化する。鍵の保管は呼び出し側が OS の鍵置き場で行う
//! - 文字列を残し、音声は受け取らない
//! - 検索は全文検索(trigram)とベクトル検索を順位で併せる
//! - 埋め込みに失敗しても文字列は残す。後で [`SqliteMemory::embed_pending`] で埋める
//! - リマインドの追加は [`MemoryStore`] に載せない。判定の側は trait しか持たないので、
//!   LLM の出力からは登録できない

mod query;
mod schema;

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use zuchi_core::ports::{Llm, MemoryStore, Recalled, Result};
use zuchi_core::types::{
    AudioChannel, Evidence, Reaction, ScreenText, Skill, Suggestion, Utterance,
};

use query::{Terms, fts_query, like_pattern, split_terms};

/// 順位の併合(RRF)の定数。論文の標準の値
const RRF_K: f32 = 60.0;
/// 各検索から集める候補の下限
const MIN_CANDIDATES: usize = 50;
/// 保持期間の削除を、書き込みの合間に行う間隔
const PURGE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// 256 ビットの鍵。文字列のパスワードは使わず、乱数の鍵を OS の鍵置き場に置く想定。
#[derive(Clone)]
pub struct MemoryKey([u8; 32]);

impl MemoryKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    fn pragma(&self) -> String {
        let hex: String = self.0.iter().map(|b| format!("{b:02x}")).collect();
        format!("PRAGMA key = \"x'{hex}'\";")
    }
}

impl fmt::Debug for MemoryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MemoryKey(<隠す>)")
    }
}

#[derive(Clone, Debug)]
pub struct MemoryConfig {
    /// 出来事の保持期間。`None` なら消さない
    pub retention: Option<Duration>,
    /// これより似ていないものはベクトル検索の結果にしない。
    /// 近いものを必ず返すと、無関係な記憶で口を出してしまう。埋め込みのモデルを決めたら測り直す
    pub min_similarity: f32,
    /// ベクトル検索で見る新しい記憶の件数。より古い記憶は全文検索だけで探す
    pub max_vector_scan: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            retention: Some(Duration::from_secs(90 * 24 * 60 * 60)),
            min_similarity: 0.5,
            max_vector_scan: 50_000,
        }
    }
}

#[derive(Debug)]
pub enum MemoryError {
    /// 鍵が違うか、ファイルが壊れている
    CannotDecrypt,
    /// 新しい版のアプリが書いたファイル
    NewerSchema {
        found: usize,
        supported: usize,
    },
    Sqlite(rusqlite::Error),
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CannotDecrypt => write!(f, "記憶のファイルを開けない(鍵が違うか、壊れている)"),
            Self::NewerSchema { found, supported } => write!(
                f,
                "記憶のファイルの版が新しい(ファイル {found}、対応 {supported})"
            ),
            Self::Sqlite(error) => write!(f, "記憶のデータベースのエラー: {error}"),
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<rusqlite::Error> for MemoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// 未完了のリマインドの 1 件。題名は画面や会話から作られるので、文字列としてだけ扱う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reminder {
    pub id: i64,
    pub title: String,
    pub due: SystemTime,
}

/// 反応の 1 件。判定の例として使う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReactionRecord {
    pub skill: String,
    pub text: String,
    pub reaction: String,
    pub at: SystemTime,
}

pub struct SqliteMemory {
    conn: Mutex<Connection>,
    llm: Arc<dyn Llm>,
    config: MemoryConfig,
    last_purge: Mutex<SystemTime>,
}

impl SqliteMemory {
    /// ファイルを開き、必要ならマイグレーションして、期限切れを消す。
    pub fn open(
        path: impl AsRef<Path>,
        key: &MemoryKey,
        llm: Arc<dyn Llm>,
        config: MemoryConfig,
    ) -> std::result::Result<Self, MemoryError> {
        let mut conn = open_encrypted(path.as_ref(), key)?;
        schema::migrate(&mut conn)?;
        let now = SystemTime::now();
        let memory = Self {
            conn: Mutex::new(conn),
            llm,
            config,
            last_purge: Mutex::new(now),
        };
        memory.purge_expired(now)?;
        Ok(memory)
    }

    /// 保持期間を過ぎた出来事を消す。消した件数を返す。
    pub fn purge_expired(&self, now: SystemTime) -> std::result::Result<usize, MemoryError> {
        let Some(retention) = self.config.retention else {
            return Ok(0);
        };
        let cutoff = to_millis(now.checked_sub(retention).unwrap_or(UNIX_EPOCH));
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut removed = 0;
        for sql in [
            "DELETE FROM memories WHERE at_ms < ?1",
            "DELETE FROM reactions WHERE at_ms < ?1",
            "DELETE FROM search_history WHERE at_ms < ?1",
            // 未完了のものは、本人が完了にするまで消さない
            "DELETE FROM reminders WHERE done_at_ms < ?1",
        ] {
            removed += tx.execute(sql, [cutoff])?;
        }
        tx.commit()?;
        if removed > 0 {
            // WAL に残った古いページも片づける
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        }
        Ok(removed)
    }

    /// 埋め込みのない記憶を、新しいものから埋める。Ollama の復旧後などに定期的に呼ぶ。
    /// 埋められた件数を返す。
    pub fn embed_pending(&self, limit: usize) -> Result<usize> {
        let pending: Vec<(i64, String)> = {
            let conn = self.lock();
            let mut statement = conn.prepare(
                "SELECT m.id, m.text FROM memories m
                 LEFT JOIN embeddings e ON e.memory_id = m.id
                 WHERE e.memory_id IS NULL ORDER BY m.at_ms DESC LIMIT ?1",
            )?;
            statement
                .query_map([limit as i64], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut done = 0;
        for (id, text) in pending {
            let vector = self.llm.embed(&text)?;
            self.store_embedding(id, vector)?;
            done += 1;
        }
        Ok(done)
    }

    /// 外へ出た検索語を残す。`zuchi-search` の `SearchLog` はアプリ側でここへつなぐ。
    pub fn record_search(&self, query: &str, at: SystemTime) -> Result<()> {
        self.lock().execute(
            "INSERT INTO search_history (query, at_ms) VALUES (?1, ?2)",
            params![query, to_millis(at)],
        )?;
        Ok(())
    }

    /// 検索の履歴。新しい順。
    pub fn search_history(&self, limit: usize) -> Result<Vec<(String, SystemTime)>> {
        let conn = self.lock();
        let mut statement = conn.prepare(
            "SELECT query, at_ms FROM search_history ORDER BY at_ms DESC, id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], |row| {
            Ok((row.get(0)?, from_millis(row.get(1)?)))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 補足への反応。新しい順。
    pub fn recent_reactions(&self, limit: usize) -> Result<Vec<ReactionRecord>> {
        let conn = self.lock();
        let mut statement = conn.prepare(
            "SELECT skill, text, reaction, at_ms FROM reactions
             ORDER BY at_ms DESC, id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], |row| {
            Ok(ReactionRecord {
                skill: row.get(0)?,
                text: row.get(1)?,
                reaction: row.get(2)?,
                at: from_millis(row.get(3)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// リマインドを追加する。本人の操作を受けた層からだけ呼ぶ。
    pub fn add_reminder(&self, title: &str, due: SystemTime, now: SystemTime) -> Result<Reminder> {
        let title = title.trim();
        if title.is_empty() {
            return Err("リマインドの題名が空".into());
        }
        let conn = self.lock();
        conn.execute(
            "INSERT INTO reminders (title, due_at_ms, created_at_ms) VALUES (?1, ?2, ?3)",
            params![title, to_millis(due), to_millis(now)],
        )?;
        Ok(Reminder {
            id: conn.last_insert_rowid(),
            title: title.to_owned(),
            due,
        })
    }

    /// 期限が来ていて、まだ知らせていない未完了のもの。期限の早い順。
    pub fn due_reminders(&self, now: SystemTime) -> Result<Vec<Reminder>> {
        self.select_reminders(
            "SELECT id, title, due_at_ms FROM reminders
             WHERE done_at_ms IS NULL AND notified_at_ms IS NULL AND due_at_ms <= ?1
             ORDER BY due_at_ms, id",
            params![to_millis(now)],
        )
    }

    /// 未完了のもの全部。期限の早い順。
    pub fn open_reminders(&self) -> Result<Vec<Reminder>> {
        self.select_reminders(
            "SELECT id, title, due_at_ms FROM reminders
             WHERE done_at_ms IS NULL ORDER BY due_at_ms, id",
            [],
        )
    }

    /// 知らせ終えた印をつける。次の確認や次の起動で、同じものを知らせない。
    pub fn mark_reminders_notified(&self, ids: &[i64], at: SystemTime) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        for id in ids {
            tx.execute(
                "UPDATE reminders SET notified_at_ms = ?2 WHERE id = ?1",
                params![id, to_millis(at)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 完了にする。古いメニューから押されることがあるので、対象がなくても失敗にしない。
    /// 完了にしたら `true` を返す。
    pub fn complete_reminder(&self, id: i64, at: SystemTime) -> Result<bool> {
        let changed = self.lock().execute(
            "UPDATE reminders SET done_at_ms = ?2 WHERE id = ?1 AND done_at_ms IS NULL",
            params![id, to_millis(at)],
        )?;
        Ok(changed > 0)
    }

    fn select_reminders(&self, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Reminder>> {
        let conn = self.lock();
        let mut statement = conn.prepare_cached(sql)?;
        let rows = statement.query_map(args, |row| {
            Ok(Reminder {
                id: row.get(0)?,
                title: row.get(1)?,
                due: from_millis(row.get(2)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn insert(
        &self,
        kind: &str,
        text: &str,
        at: SystemTime,
        app: Option<&str>,
        window_title: Option<&str>,
    ) -> Result<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        self.purge_if_due()?;
        let id = {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO memories (kind, text, at_ms, app, window_title)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![kind, text, to_millis(at), app, window_title],
            )?;
            conn.last_insert_rowid()
        };
        // 埋め込みは接続のロックを持たずに行う。失敗しても文字列は残り、全文検索で見つかる
        if let Ok(vector) = self.llm.embed(text) {
            self.store_embedding(id, vector)?;
        }
        Ok(())
    }

    /// 常駐して動き続けるので、起動時だけでなく書き込みの合間にも消す。
    fn purge_if_due(&self) -> Result<()> {
        let now = SystemTime::now();
        {
            let mut last = self.last_purge.lock().unwrap_or_else(|e| e.into_inner());
            let due = now
                .duration_since(*last)
                .is_ok_and(|elapsed| elapsed >= PURGE_INTERVAL);
            if !due {
                return Ok(());
            }
            *last = now;
        }
        self.purge_expired(now)?;
        Ok(())
    }

    fn store_embedding(&self, id: i64, mut vector: Vec<f32>) -> Result<()> {
        normalize(&mut vector);
        let blob: Vec<u8> = vector.iter().flat_map(|v| v.to_le_bytes()).collect();
        // 埋め込みの間に保持期間の削除が走ることがあるので、残っている記憶にだけ書く
        self.lock().execute(
            "INSERT OR REPLACE INTO embeddings (memory_id, dim, vector)
             SELECT id, ?2, ?3 FROM memories WHERE id = ?1",
            params![id, vector.len() as i64, blob],
        )?;
        Ok(())
    }

    fn lexical_ids(&self, terms: &Terms, limit: usize) -> rusqlite::Result<Vec<i64>> {
        let conn = self.lock();
        let mut ids = Vec::new();
        if !terms.long.is_empty() {
            let mut statement = conn.prepare_cached(
                "SELECT rowid FROM memories_fts WHERE memories_fts MATCH ?1
                 ORDER BY bm25(memories_fts) LIMIT ?2",
            )?;
            let rows = statement
                .query_map(params![fts_query(&terms.long), limit as i64], |row| {
                    row.get(0)
                })?;
            ids = rows.collect::<rusqlite::Result<Vec<i64>>>()?;
        }
        // trigram は 2 文字の語を引けない
        for term in &terms.short {
            let mut statement = conn.prepare_cached(
                "SELECT id FROM memories WHERE text LIKE ?1 ESCAPE '\\'
                 ORDER BY at_ms DESC LIMIT ?2",
            )?;
            let rows = statement.query_map(params![like_pattern(term), limit as i64], |row| {
                row.get::<_, i64>(0)
            })?;
            for id in rows {
                let id = id?;
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        Ok(ids)
    }

    /// 新しい記憶から順にコサイン類似度を計算する。sqlite-vec は使わない。
    ///
    /// 発話は 1 日 3 時間として月 4 万件ほどの見込み。1024 次元を M シリーズの Mac で測ると、
    /// 暗号化したファイルで 1 万件 78 ms、5 万件 310 ms、15 万件 2.7 秒だった
    /// (`tests/bench.rs`)。15 万件ではページキャッシュから外れて急に遅くなり、その間は書き込みも
    /// 待たされるので、見る件数に上限を置く。上限より古い記憶は全文検索で探す。
    /// sqlite-vec は執筆時点で alpha の版しかなく、全件探索である点も同じなので見送った。
    fn vector_ids(&self, query: &[f32], limit: usize) -> rusqlite::Result<Vec<i64>> {
        let conn = self.lock();
        let mut statement = conn.prepare_cached(
            "SELECT memory_id, vector FROM embeddings WHERE dim = ?1
                 ORDER BY memory_id DESC LIMIT ?2",
        )?;
        let mut rows = statement.query(params![
            query.len() as i64,
            self.config.max_vector_scan as i64
        ])?;
        let mut scored: Vec<(f32, i64)> = Vec::new();
        while let Some(row) = rows.next()? {
            let score = dot(query, row.get_ref(1)?.as_blob()?);
            if score >= self.config.min_similarity {
                scored.push((score, row.get(0)?));
            }
        }
        scored.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
        scored.truncate(limit);
        Ok(scored.into_iter().map(|(_, id)| id).collect())
    }

    fn load(&self, id: i64, score: f32) -> rusqlite::Result<Option<Recalled>> {
        let conn = self.lock();
        let mut statement = conn.prepare_cached(
            "SELECT kind, text, at_ms, app, window_title FROM memories WHERE id = ?1",
        )?;
        statement
            .query_row([id], |row| {
                let kind: String = row.get(0)?;
                let text: String = row.get(1)?;
                let at = from_millis(row.get(2)?);
                let app: Option<String> = row.get(3)?;
                let title: Option<String> = row.get(4)?;
                // Evidence に画面の種類がないので、発言と取り違えないよう本文に出どころを書く
                let evidence_text = if kind == "screen" {
                    format!(
                        "[画面: {} / {}] {text}",
                        app.unwrap_or_default(),
                        title.unwrap_or_default()
                    )
                } else {
                    text.clone()
                };
                Ok(Recalled {
                    text,
                    evidence: Evidence::Utterance {
                        text: evidence_text,
                        at,
                    },
                    score,
                })
            })
            .optional()
    }
}

impl MemoryStore for SqliteMemory {
    fn remember_utterance(&self, utterance: &Utterance) -> Result<()> {
        let kind = match utterance.channel {
            AudioChannel::Mic => "mic",
            AudioChannel::System => "system",
        };
        self.insert(kind, &utterance.text, utterance.started_at, None, None)
    }

    fn remember_screen(&self, screen: &ScreenText) -> Result<()> {
        self.insert(
            "screen",
            &screen.text,
            screen.captured_at,
            Some(&screen.app),
            Some(&screen.window_title),
        )
    }

    fn remember_reaction(&self, suggestion: &Suggestion, reaction: Reaction) -> Result<()> {
        self.purge_if_due()?;
        let skill = match suggestion.skill {
            Skill::Calc => "calc",
            Skill::Search => "search",
            Skill::Memory => "memory",
            Skill::Suggest => "suggest",
        };
        let reaction = match reaction {
            Reaction::Helpful => "helpful",
            Reaction::Unwanted => "unwanted",
            Reaction::Ignored => "ignored",
        };
        self.lock().execute(
            "INSERT INTO reactions (skill, text, evidence, reaction, at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                skill,
                suggestion.text,
                evidence_json(&suggestion.evidence).to_string(),
                reaction,
                to_millis(SystemTime::now()),
            ],
        )?;
        Ok(())
    }

    fn recall(&self, query: &str, limit: usize) -> Result<Vec<Recalled>> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let candidates = (limit * 5).max(MIN_CANDIDATES);
        let lexical = self.lexical_ids(&split_terms(query), candidates)?;
        // Ollama が止まっていても、全文検索の結果は返す
        let vector = match self.llm.embed(query) {
            Ok(mut embedding) => {
                normalize(&mut embedding);
                self.vector_ids(&embedding, candidates)?
            }
            Err(_) => Vec::new(),
        };

        let mut scores: HashMap<i64, f32> = HashMap::new();
        for ids in [&lexical, &vector] {
            for (rank, id) in ids.iter().enumerate() {
                *scores.entry(*id).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
            }
        }
        // 両方の検索で 1 位のときに 1.0 になるようにそろえる
        let scale = (RRF_K + 1.0) / 2.0;
        let mut ranked: Vec<(i64, f32)> = scores.into_iter().collect();
        ranked.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));

        let mut recalled = Vec::with_capacity(limit.min(ranked.len()));
        for (id, score) in ranked.into_iter().take(limit) {
            if let Some(item) = self.load(id, score * scale)? {
                recalled.push(item);
            }
        }
        Ok(recalled)
    }
}

/// 通常の表の定義。テストで、音声の列がないことを確かめるために使う。
#[doc(hidden)]
pub fn schema_sql(memory: &SqliteMemory) -> Result<String> {
    let conn = memory.lock();
    let sql = conn.query_row(
        "SELECT group_concat(sql, char(10)) FROM sqlite_master
         WHERE type = 'table' AND name NOT LIKE 'memories_fts%'",
        [],
        |row| row.get(0),
    )?;
    Ok(sql)
}

fn open_encrypted(path: &Path, key: &MemoryKey) -> std::result::Result<Connection, MemoryError> {
    let conn = Connection::open(path)?;
    // 鍵の指定は最初の操作でなければならない
    conn.execute_batch(&key.pragma())?;
    // 鍵の誤りは、最初にファイルを読むときに分かる
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(|_| MemoryError::CannotDecrypt)?;
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA synchronous = NORMAL;
         PRAGMA secure_delete = ON;",
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

fn evidence_json(evidence: &[Evidence]) -> serde_json::Value {
    evidence
        .iter()
        .map(|item| match item {
            Evidence::Assumption(text) => json!({"type": "assumption", "text": text}),
            Evidence::Web { title, url } => json!({"type": "web", "title": title, "url": url}),
            Evidence::Utterance { text, at } => {
                json!({"type": "utterance", "text": text, "at_ms": to_millis(*at)})
            }
        })
        .collect()
}

fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        vector.iter_mut().for_each(|v| *v /= norm);
    }
}

/// `blob` は little endian の f32 の並び。保存時に正規化してあるので、内積がコサイン類似度になる。
fn dot(query: &[f32], blob: &[u8]) -> f32 {
    query
        .iter()
        .zip(blob.as_chunks::<4>().0)
        .map(|(a, b)| a * f32::from_le_bytes(*b))
        .sum()
}

fn to_millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn from_millis(millis: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(millis.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::schema::{MIGRATIONS, migrate, migrate_to};
    use super::*;

    fn temp_key() -> MemoryKey {
        MemoryKey::from_bytes([7; 32])
    }

    #[test]
    fn 古い版のファイルを保ったまま上げる() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.sqlite");
        {
            let mut conn = open_encrypted(&path, &temp_key()).unwrap();
            migrate_to(&mut conn, 1).unwrap();
            conn.execute(
                "INSERT INTO memories (kind, text, at_ms) VALUES ('mic', 'A案に決定しました', 1)",
                [],
            )
            .unwrap();
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
            assert_eq!(version, 1);
            let has_history: bool = conn
                .query_row(
                    "SELECT count(*) > 0 FROM sqlite_master WHERE name = 'search_history'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(!has_history);
        }

        let mut conn = open_encrypted(&path, &temp_key()).unwrap();
        migrate(&mut conn).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version as usize, MIGRATIONS.len());
        conn.execute(
            "INSERT INTO search_history (query, at_ms) VALUES ('BONX', 2)",
            [],
        )
        .unwrap();
        let text: String = conn
            .query_row(
                "SELECT m.text FROM memories_fts f JOIN memories m ON m.id = f.rowid
                 WHERE memories_fts MATCH '\"決定し\"'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(text, "A案に決定しました");

        // 2 回目は何もしない
        migrate(&mut conn).unwrap();
    }

    #[test]
    fn リマインドの表がない版から上げても記憶は残る() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.sqlite");
        {
            let mut conn = open_encrypted(&path, &temp_key()).unwrap();
            migrate_to(&mut conn, 2).unwrap();
            conn.execute(
                "INSERT INTO search_history (query, at_ms) VALUES ('BONX', 2)",
                [],
            )
            .unwrap();
            let has_reminders: bool = conn
                .query_row(
                    "SELECT count(*) > 0 FROM sqlite_master WHERE name = 'reminders'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(!has_reminders);
        }

        let mut conn = open_encrypted(&path, &temp_key()).unwrap();
        migrate(&mut conn).unwrap();
        let kept: String = conn
            .query_row("SELECT query FROM search_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, "BONX");
        conn.execute(
            "INSERT INTO reminders (title, due_at_ms, created_at_ms) VALUES ('確認', 1, 1)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn 新しい版のファイルは開かない() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.sqlite");
        let mut conn = open_encrypted(&path, &temp_key()).unwrap();
        conn.pragma_update(None, "user_version", MIGRATIONS.len() as i64 + 1)
            .unwrap();
        assert!(matches!(
            migrate(&mut conn),
            Err(MemoryError::NewerSchema { .. })
        ));
    }

    #[test]
    fn 正規化すると内積がコサインになる() {
        let mut a = vec![3.0, 4.0];
        normalize(&mut a);
        let blob: Vec<u8> = a.iter().flat_map(|v| v.to_le_bytes()).collect();
        assert!((dot(&a, &blob) - 1.0).abs() < 1e-6);
    }
}
