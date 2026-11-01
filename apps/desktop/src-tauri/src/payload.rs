//! 画面へ渡す形。`zuchi-core` の型は serde に依存しないので、ここで写す。
//! 形を変えるときは `src/types.ts` も直す。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::Url;
use zuchi_core::types::{
    DEFAULT_REMINDER_DELAY, Evidence, ProposedAction, Reaction, Skill, Suggestion,
};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SuggestionPayload {
    pub id: u64,
    pub kind: &'static str,
    pub text: String,
    pub evidence: Vec<EvidencePayload>,
    pub action: Option<ActionPayload>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EvidencePayload {
    Assumption { text: String },
    Web { title: String, url: Option<String> },
    Utterance { text: String, at_ms: u64 },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ActionPayload {
    SetReminder {
        title: String,
        due_ms: Option<u64>,
        /// `due_ms` がないときに知らせるまでの分数。実行の前に本人へ見せる
        default_delay_minutes: u64,
    },
}

/// 画面から届く反応。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReactionPayload {
    Helpful,
    Unwanted,
    Ignored,
}

impl From<ReactionPayload> for Reaction {
    fn from(reaction: ReactionPayload) -> Self {
        match reaction {
            ReactionPayload::Helpful => Reaction::Helpful,
            ReactionPayload::Unwanted => Reaction::Unwanted,
            ReactionPayload::Ignored => Reaction::Ignored,
        }
    }
}

fn millis(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 開いてよい URL だけを返す。https のみで、認証情報つきは見た目をごまかせるので通さない。
pub fn safe_https_url(url: &str) -> Option<Url> {
    let parsed = Url::parse(url).ok()?;
    let allowed = parsed.scheme() == "https"
        && parsed.has_host()
        && parsed.username().is_empty()
        && parsed.password().is_none();
    allowed.then_some(parsed)
}

pub fn to_payload(id: u64, suggestion: &Suggestion) -> SuggestionPayload {
    SuggestionPayload {
        id,
        kind: match suggestion.skill {
            Skill::Calc => "calc",
            Skill::Search => "search",
            Skill::Memory => "memory",
            Skill::Suggest => "suggest",
        },
        text: suggestion.text.clone(),
        evidence: suggestion.evidence.iter().map(evidence_payload).collect(),
        action: suggestion.action.as_ref().map(|action| match action {
            ProposedAction::SetReminder { title, due } => ActionPayload::SetReminder {
                title: title.clone(),
                due_ms: due.map(millis),
                default_delay_minutes: DEFAULT_REMINDER_DELAY.as_secs() / 60,
            },
        }),
    }
}

fn evidence_payload(evidence: &Evidence) -> EvidencePayload {
    match evidence {
        Evidence::Assumption(text) => EvidencePayload::Assumption { text: text.clone() },
        Evidence::Web { title, url } => EvidencePayload::Web {
            title: title.clone(),
            url: safe_https_url(url).map(String::from),
        },
        Evidence::Utterance { text, at } => EvidencePayload::Utterance {
            text: text.clone(),
            at_ms: millis(*at),
        },
    }
}

/// 表示中の補足。反応は 1 件につき 1 度だけ受け付け、操作の中身は画面から受け取らない。
#[derive(Debug, Default)]
pub struct Pending {
    next_id: u64,
    shown: HashMap<u64, Suggestion>,
}

impl Pending {
    pub fn insert(&mut self, suggestion: Suggestion) -> u64 {
        self.next_id += 1;
        self.shown.insert(self.next_id, suggestion);
        self.next_id
    }

    pub fn take(&mut self, id: u64) -> Option<Suggestion> {
        self.shown.remove(&id)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    fn suggestion(evidence: Vec<Evidence>, action: Option<ProposedAction>) -> Suggestion {
        Suggestion {
            skill: Skill::Search,
            text: "本文".into(),
            evidence,
            action,
        }
    }

    #[test]
    fn 画面の型と同じ形に写す() {
        let at = UNIX_EPOCH + Duration::from_millis(1_500);
        let payload = to_payload(
            7,
            &suggestion(
                vec![
                    Evidence::Assumption("前提".into()),
                    Evidence::Web {
                        title: "題名".into(),
                        url: "https://example.com/a".into(),
                    },
                    Evidence::Utterance {
                        text: "発言".into(),
                        at,
                    },
                ],
                Some(ProposedAction::SetReminder {
                    title: "確認".into(),
                    due: Some(at),
                }),
            ),
        );
        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "id": 7,
                "kind": "search",
                "text": "本文",
                "evidence": [
                    { "type": "assumption", "text": "前提" },
                    { "type": "web", "title": "題名", "url": "https://example.com/a" },
                    { "type": "utterance", "text": "発言", "atMs": 1500 },
                ],
                "action": {
                    "type": "setReminder",
                    "title": "確認",
                    "dueMs": 1500,
                    "defaultDelayMinutes": 30,
                },
            })
        );
    }

    #[test]
    fn https_以外の出典はリンクを外して題名だけ残す() {
        for url in [
            "http://example.com/",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://user:pass@example.com/",
            "壊れた URL",
        ] {
            let payload = to_payload(
                1,
                &suggestion(
                    vec![Evidence::Web {
                        title: "題名".into(),
                        url: url.into(),
                    }],
                    None,
                ),
            );
            assert_eq!(
                payload.evidence,
                vec![EvidencePayload::Web {
                    title: "題名".into(),
                    url: None,
                }],
                "{url}"
            );
        }
    }

    #[test]
    fn 反応の名前を画面の値から読める() {
        let reaction: ReactionPayload = serde_json::from_value(json!("unwanted")).unwrap();
        assert_eq!(Reaction::from(reaction), Reaction::Unwanted);
    }

    #[test]
    fn 同じ補足の反応は_1_度だけ受け付ける() {
        let mut pending = Pending::default();
        let id = pending.insert(suggestion(vec![], None));
        assert!(pending.take(id).is_some());
        assert!(pending.take(id).is_none());
        assert!(pending.take(id + 1).is_none());
    }
}
