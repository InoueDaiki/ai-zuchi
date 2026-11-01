//! 補足を画面に出す直前の機械的な確認。

use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

use crate::types::{Evidence, Reaction, Skill, Suggestion};

#[derive(Clone, Debug)]
pub struct CheckPolicy {
    /// 同じ話題を繰り返さない時間
    pub repeat_window: Duration,
    /// 1 時間あたりの上限
    pub hourly_limit: usize,
    /// 次の補足までの最短の間隔
    pub min_interval: Duration,
    /// 無視や不要が続いたときに、間隔を延ばす上限の倍率
    pub max_backoff: u32,
}

impl Default for CheckPolicy {
    fn default() -> Self {
        Self {
            repeat_window: Duration::from_secs(10 * 60),
            hourly_limit: 15,
            min_interval: Duration::from_secs(30),
            max_backoff: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    NoEvidence,
    Repeated,
    TooFrequent,
    OverLimit,
}

const HOUR: Duration = Duration::from_secs(3_600);

/// 迷ったら黙るための門番。時刻は外から渡す。
#[derive(Debug)]
pub struct Checker {
    policy: CheckPolicy,
    shown: VecDeque<(SystemTime, String)>,
    backoff: u32,
}

impl Checker {
    pub fn new(policy: CheckPolicy) -> Self {
        Self {
            policy,
            shown: VecDeque::new(),
            backoff: 1,
        }
    }

    /// 通れば、表示したものとして記録する。`topic` は検索語など、話題を表す短い文字列。
    pub fn admit(
        &mut self,
        suggestion: &Suggestion,
        topic: &str,
        now: SystemTime,
    ) -> Result<(), Rejection> {
        if !has_required_evidence(suggestion) {
            return Err(Rejection::NoEvidence);
        }
        while let Some((at, _)) = self.shown.front() {
            if elapsed(*at, now) < HOUR.max(self.policy.repeat_window) {
                break;
            }
            self.shown.pop_front();
        }

        let topic = normalize(topic);
        let repeated = self
            .shown
            .iter()
            .any(|(at, shown)| *shown == topic && elapsed(*at, now) < self.policy.repeat_window);
        if repeated {
            return Err(Rejection::Repeated);
        }
        if let Some((last, _)) = self.shown.back()
            && elapsed(*last, now) < self.policy.min_interval * self.backoff
        {
            return Err(Rejection::TooFrequent);
        }
        let in_hour = self
            .shown
            .iter()
            .filter(|(at, _)| elapsed(*at, now) < HOUR)
            .count();
        if in_hour >= self.policy.hourly_limit {
            return Err(Rejection::OverLimit);
        }

        self.shown.push_back((now, topic));
        Ok(())
    }

    /// 反応に応じて間隔を調整する。喜ばれたら元に戻す。
    pub fn observe(&mut self, reaction: Reaction) {
        self.backoff = match reaction {
            Reaction::Helpful => 1,
            Reaction::Unwanted | Reaction::Ignored => {
                (self.backoff * 2).min(self.policy.max_backoff)
            }
        };
    }
}

fn has_required_evidence(suggestion: &Suggestion) -> bool {
    let has = |matches: fn(&Evidence) -> bool| suggestion.evidence.iter().any(matches);
    match suggestion.skill {
        Skill::Search => has(|e| matches!(e, Evidence::Web { .. })),
        Skill::Memory => has(|e| matches!(e, Evidence::Utterance { .. })),
        Skill::Calc | Skill::Suggest => true,
    }
}

fn elapsed(from: SystemTime, to: SystemTime) -> Duration {
    to.duration_since(from).unwrap_or(Duration::ZERO)
}

fn normalize(topic: &str) -> String {
    topic
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn calc() -> Suggestion {
        Suggestion {
            skill: Skill::Calc,
            text: "秒間約127件ですね".into(),
            evidence: vec![],
            action: None,
        }
    }

    fn search(evidence: Vec<Evidence>) -> Suggestion {
        Suggestion {
            skill: Skill::Search,
            text: "人気のようです".into(),
            evidence,
            action: None,
        }
    }

    #[test]
    fn 根拠のない検索と記憶は出さない() {
        let mut checker = Checker::new(CheckPolicy::default());
        assert_eq!(
            checker.admit(&search(vec![]), "BONX", at(0)),
            Err(Rejection::NoEvidence)
        );
        let web = Evidence::Web {
            title: "t".into(),
            url: "https://example.com".into(),
        };
        assert_eq!(checker.admit(&search(vec![web]), "BONX", at(0)), Ok(()));
    }

    #[test]
    fn 同じ話題は繰り返さない() {
        let mut checker = Checker::new(CheckPolicy::default());
        assert_eq!(checker.admit(&calc(), "10億 件", at(0)), Ok(()));
        assert_eq!(
            checker.admit(&calc(), "10億件", at(100)),
            Err(Rejection::Repeated)
        );
        assert_eq!(checker.admit(&calc(), "10億件", at(601)), Ok(()));
    }

    #[test]
    fn 間隔が短いと出さず無視が続くと延ばす() {
        let mut checker = Checker::new(CheckPolicy::default());
        assert_eq!(checker.admit(&calc(), "a", at(0)), Ok(()));
        assert_eq!(
            checker.admit(&calc(), "b", at(10)),
            Err(Rejection::TooFrequent)
        );
        checker.observe(Reaction::Ignored);
        assert_eq!(
            checker.admit(&calc(), "b", at(40)),
            Err(Rejection::TooFrequent)
        );
        assert_eq!(checker.admit(&calc(), "b", at(60)), Ok(()));
        checker.observe(Reaction::Helpful);
        assert_eq!(checker.admit(&calc(), "c", at(90)), Ok(()));
    }

    #[test]
    fn 一時間の上限を超えない() {
        let policy = CheckPolicy {
            hourly_limit: 2,
            min_interval: Duration::from_secs(1),
            ..CheckPolicy::default()
        };
        let mut checker = Checker::new(policy);
        assert_eq!(checker.admit(&calc(), "a", at(0)), Ok(()));
        assert_eq!(checker.admit(&calc(), "b", at(10)), Ok(()));
        assert_eq!(
            checker.admit(&calc(), "c", at(20)),
            Err(Rejection::OverLimit)
        );
        assert_eq!(checker.admit(&calc(), "c", at(3_601)), Ok(()));
    }
}
