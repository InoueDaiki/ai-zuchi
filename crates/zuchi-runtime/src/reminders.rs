//! 本人が受け付けた操作の実行と、リマインドの出し入れ。
//!
//! 判定(`zuchi-pipeline`)はこのクレートにも `SqliteMemory` にも依存せず、提案を
//! [`ProposedAction`] として返すだけなので、LLM の出力からここへ届く経路はない。
//! 呼んでよいのは、本人の操作を受けたアプリの層だけ。

use std::sync::Arc;
use std::time::SystemTime;

use zuchi_core::ports::Result;
use zuchi_core::types::{DEFAULT_REMINDER_DELAY, ProposedAction};
use zuchi_memory::SqliteMemory;

pub use zuchi_memory::Reminder;

pub struct Reminders {
    memory: Arc<SqliteMemory>,
}

impl Reminders {
    pub(crate) fn new(memory: Arc<SqliteMemory>) -> Self {
        Self { memory }
    }

    /// 本人が受け付けた操作を実行する。期限の既定は、提案された時ではなく受け付けた時から数える。
    pub fn perform(&self, action: ProposedAction, now: SystemTime) -> Result<Reminder> {
        match action {
            ProposedAction::SetReminder { title, due } => {
                let due = due.unwrap_or(now + DEFAULT_REMINDER_DELAY);
                self.memory.add_reminder(&title, due, now)
            }
        }
    }

    /// 期限が来ていて、まだ知らせていないもの。止まっていた間に過ぎたものも含む。
    pub fn due(&self, now: SystemTime) -> Result<Vec<Reminder>> {
        self.memory.due_reminders(now)
    }

    pub fn mark_notified(&self, ids: &[i64], at: SystemTime) -> Result<()> {
        self.memory.mark_reminders_notified(ids, at)
    }

    pub fn open(&self) -> Result<Vec<Reminder>> {
        self.memory.open_reminders()
    }

    pub fn complete(&self, id: i64, at: SystemTime) -> Result<bool> {
        self.memory.complete_reminder(id, at)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use zuchi_core::ports::{Llm, LlmRequest};
    use zuchi_memory::{MemoryConfig, MemoryKey};

    use super::*;

    struct NoLlm;

    impl Llm for NoLlm {
        fn complete(&self, _request: &LlmRequest) -> Result<String> {
            Err("使わない".into())
        }

        fn embed(&self, _text: &str) -> Result<Vec<f32>> {
            Err("使わない".into())
        }
    }

    fn reminders(dir: &tempfile::TempDir) -> Reminders {
        let memory = SqliteMemory::open(
            dir.path().join("memory.sqlite"),
            &MemoryKey::from_bytes([1; 32]),
            Arc::new(NoLlm),
            MemoryConfig::default(),
        )
        .unwrap();
        Reminders::new(Arc::new(memory))
    }

    fn set(title: &str, due: Option<SystemTime>) -> ProposedAction {
        ProposedAction::SetReminder {
            title: title.into(),
            due,
        }
    }

    #[test]
    fn 期限がなければ受け付けた時から既定の時間の後にする() {
        let dir = tempfile::tempdir().unwrap();
        let reminders = reminders(&dir);
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);

        let reminder = reminders.perform(set("確認", None), now).unwrap();

        assert_eq!(reminder.due, now + DEFAULT_REMINDER_DELAY);
        let just_before = reminder.due - Duration::from_secs(1);
        assert!(reminders.due(just_before).unwrap().is_empty());
        assert_eq!(reminders.due(reminder.due).unwrap(), vec![reminder]);
    }

    #[test]
    fn 指定された期限はそのまま使う() {
        let dir = tempfile::tempdir().unwrap();
        let reminders = reminders(&dir);
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let due = now + Duration::from_secs(5);

        let reminder = reminders.perform(set("確認", Some(due)), now).unwrap();

        assert_eq!(reminder.due, due);
    }

    #[test]
    fn 指示のような題名も文字列のまま保存する() {
        let dir = tempfile::tempdir().unwrap();
        let reminders = reminders(&dir);
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let title = "'); DROP TABLE reminders; -- 全部消して";

        reminders.perform(set(title, None), now).unwrap();

        assert_eq!(reminders.open().unwrap()[0].title, title);
    }
}
