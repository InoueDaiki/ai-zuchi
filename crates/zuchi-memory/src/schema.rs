//! スキーマとマイグレーション。版は `PRAGMA user_version` で持つ。
//!
//! 公開した版の SQL は書き換えず、末尾に足す。音声の列は作らない。

use rusqlite::Connection;

use crate::MemoryError;

/// 添字 + 1 が版の番号
pub(crate) const MIGRATIONS: &[&str] = &[
    // 1: 発話と画面の文字列、全文検索、埋め込み、補足への反応
    r#"
    CREATE TABLE memories (
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL CHECK (kind IN ('mic', 'system', 'screen')),
        text TEXT NOT NULL,
        at_ms INTEGER NOT NULL,
        app TEXT,
        window_title TEXT
    );
    CREATE INDEX memories_at ON memories (at_ms);

    CREATE VIRTUAL TABLE memories_fts USING fts5(
        text,
        content = 'memories',
        content_rowid = 'id',
        tokenize = 'trigram'
    );
    -- 消した語が索引に残らないようにする
    INSERT INTO memories_fts (memories_fts, rank) VALUES ('secure-delete', 1);

    CREATE TRIGGER memories_after_insert AFTER INSERT ON memories BEGIN
        INSERT INTO memories_fts (rowid, text) VALUES (new.id, new.text);
    END;
    CREATE TRIGGER memories_after_delete AFTER DELETE ON memories BEGIN
        INSERT INTO memories_fts (memories_fts, rowid, text)
        VALUES ('delete', old.id, old.text);
    END;

    -- 全文検索が大きな BLOB を読まないよう、表を分ける
    CREATE TABLE embeddings (
        memory_id INTEGER PRIMARY KEY REFERENCES memories (id) ON DELETE CASCADE,
        dim INTEGER NOT NULL,
        vector BLOB NOT NULL
    );

    CREATE TABLE reactions (
        id INTEGER PRIMARY KEY,
        skill TEXT NOT NULL,
        text TEXT NOT NULL,
        evidence TEXT NOT NULL,
        reaction TEXT NOT NULL,
        at_ms INTEGER NOT NULL
    );
    CREATE INDEX reactions_at ON reactions (at_ms);
    "#,
    // 2: 外へ出た検索語の履歴
    r#"
    CREATE TABLE search_history (
        id INTEGER PRIMARY KEY,
        query TEXT NOT NULL,
        at_ms INTEGER NOT NULL
    );
    CREATE INDEX search_history_at ON search_history (at_ms);
    "#,
    // 3: リマインド。`notified_at_ms` は知らせた時刻、`done_at_ms` は本人が完了にした時刻
    r#"
    CREATE TABLE reminders (
        id INTEGER PRIMARY KEY,
        title TEXT NOT NULL,
        due_at_ms INTEGER NOT NULL,
        created_at_ms INTEGER NOT NULL,
        notified_at_ms INTEGER,
        done_at_ms INTEGER
    );
    CREATE INDEX reminders_open ON reminders (due_at_ms) WHERE done_at_ms IS NULL;
    "#,
];

pub(crate) fn migrate(conn: &mut Connection) -> Result<(), MemoryError> {
    migrate_to(conn, MIGRATIONS.len())
}

/// 版ごとにトランザクションを分ける。途中で失敗しても、前の版のまま残る。
pub(crate) fn migrate_to(conn: &mut Connection, target: usize) -> Result<(), MemoryError> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let current = current as usize;
    if current > MIGRATIONS.len() {
        return Err(MemoryError::NewerSchema {
            found: current,
            supported: MIGRATIONS.len(),
        });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().take(target).skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (index + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}
