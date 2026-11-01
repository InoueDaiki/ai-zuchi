//! 判定の本体。入力を受け取るたびに、1 件ずつ順番に処理する。

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use zuchi_core::check::{CheckPolicy, Checker};
use zuchi_core::context::ContextWindow;
use zuchi_core::ports::{Llm, LlmRequest, MemoryStore, Result, Searcher};
use zuchi_core::types::{Reaction, ScreenText, Skill, Suggestion, Utterance};

use crate::{prompts, rules, skills};

#[derive(Clone, Debug)]
pub enum Input {
    Utterance(Utterance),
    Screen(ScreenText),
    Reaction(Reaction),
}

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub context_span: Duration,
    /// これより確信度が低い判定では黙る
    pub min_confidence: f64,
    pub check: CheckPolicy,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            context_span: Duration::from_secs(5 * 60),
            min_confidence: 0.6,
            check: CheckPolicy::default(),
        }
    }
}

#[derive(Deserialize)]
struct Decision {
    skill: String,
    confidence: f64,
}

pub struct Engine {
    llm: Arc<dyn Llm>,
    searcher: Option<Arc<dyn Searcher>>,
    memory: Option<Arc<dyn MemoryStore>>,
    context: ContextWindow,
    checker: Checker,
    min_confidence: f64,
}

impl Engine {
    pub fn new(
        llm: Arc<dyn Llm>,
        searcher: Option<Arc<dyn Searcher>>,
        memory: Option<Arc<dyn MemoryStore>>,
        config: EngineConfig,
    ) -> Self {
        Self {
            llm,
            searcher,
            memory,
            context: ContextWindow::new(config.context_span),
            checker: Checker::new(config.check),
            min_confidence: config.min_confidence,
        }
    }

    /// 入力を文脈へ入れる。LLM を呼ぶ価値があれば、判定の手がかりの文字列を返す。
    fn absorb(&mut self, input: Input) -> Option<(String, SystemTime)> {
        match input {
            Input::Utterance(u) => {
                let cue = rules::utterance_passes(&u.text).then(|| (u.text.clone(), u.ended_at));
                self.context.push_utterance(u);
                cue
            }
            Input::Screen(s) => {
                let cue = rules::screen_passes(&s.text).then(|| (s.text.clone(), s.captured_at));
                self.context.push_screen(s);
                cue
            }
            Input::Reaction(r) => {
                self.checker.observe(r);
                None
            }
        }
    }

    /// 1 件を処理する。テストと再生で使う。
    pub fn handle(&mut self, input: Input) -> Result<Option<Suggestion>> {
        match self.absorb(input) {
            Some((text, at)) => self.decide(&text, at),
            None => Ok(None),
        }
    }

    /// 入力が尽きるまで動く。遅れている間の入力は文脈にだけ入れ、最新の手がかりで判定する。
    pub fn run(&mut self, inputs: Receiver<Input>, outputs: Sender<Suggestion>) {
        while let Ok(first) = inputs.recv() {
            let mut cue = self.absorb(first);
            while let Ok(next) = inputs.try_recv() {
                cue = self.absorb(next).or(cue);
            }
            let Some((text, at)) = cue else { continue };
            match self.decide(&text, at) {
                Ok(Some(suggestion)) => {
                    if outputs.send(suggestion).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                // LLM や検索の失敗では黙る。内容はログに出さない
                Err(_) => eprintln!("判定に失敗したので黙る"),
            }
        }
    }

    fn decide(&mut self, cue: &str, at: SystemTime) -> Result<Option<Suggestion>> {
        let context = self.context.render();
        let Some(skill) = self.gate(&context)? else {
            return Ok(None);
        };
        let llm = self.llm.as_ref();
        let result = match skill {
            Skill::Calc => skills::calc(llm, &context)?.map(|s| (s, cue.to_string())),
            Skill::Search => match &self.searcher {
                Some(searcher) => skills::search(llm, searcher.as_ref(), &context)?,
                None => None,
            },
            Skill::Memory => match &self.memory {
                Some(store) => skills::memory(llm, store.as_ref(), &context, cue)?
                    .map(|s| (s, cue.to_string())),
                None => None,
            },
            Skill::Suggest => skills::suggest(llm, &context)?.map(|s| {
                let topic = s.text.clone();
                (s, topic)
            }),
        };
        let Some((suggestion, topic)) = result else {
            return Ok(None);
        };
        Ok(self
            .checker
            .admit(&suggestion, &topic, at)
            .is_ok()
            .then_some(suggestion))
    }

    fn gate(&self, context: &str) -> Result<Option<Skill>> {
        let text = self.llm.complete(&LlmRequest {
            system: format!("{}\n\n{}", prompts::GATE_SYSTEM, prompts::GATE_EXAMPLES),
            user: format!("{context}\n出力:"),
            json_schema: Some(prompts::GATE_SCHEMA.to_string()),
            max_tokens: Some(120),
        })?;
        let decision: Decision = serde_json::from_str(&text)?;
        if decision.confidence < self.min_confidence {
            return Ok(None);
        }
        Ok(match decision.skill.as_str() {
            "calc" => Some(Skill::Calc),
            "search" => Some(Skill::Search),
            "memory" => Some(Skill::Memory),
            "suggest" => Some(Skill::Suggest),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use zuchi_core::ports::{Error, Recalled};
    use zuchi_core::types::{AudioChannel, Evidence, SearchHit};

    use super::*;

    /// system の内容で応答を選ぶ偽の LLM。
    struct FakeLlm {
        gate: String,
        calls: Mutex<Vec<String>>,
    }

    impl FakeLlm {
        fn new(gate: &str) -> Arc<Self> {
            Arc::new(Self {
                gate: gate.to_string(),
                calls: Mutex::new(vec![]),
            })
        }

        fn calls(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl Llm for FakeLlm {
        fn complete(&self, request: &LlmRequest) -> Result<String> {
            self.calls.lock().unwrap().push(request.user.clone());
            let s = &request.system;
            Ok(if s.starts_with("あなたは会議") {
                self.gate.clone()
            } else if s == prompts::EXTRACT_SYSTEM {
                r#"{"op":"rate","total":"10億","unit":"件","period":"3カ月"}"#.into()
            } else if s == prompts::QUERY_SYSTEM {
                r#"{"query":"BONX 差別化"}"#.into()
            } else if s == prompts::SUGGEST_SYSTEM {
                r#"{"title":"資料の確認"}"#.into()
            } else {
                r#"{"found":true,"answer":"ハンズフリー用途で人気のようです","source":1}"#.into()
            })
        }

        fn embed(&self, _: &str) -> Result<Vec<f32>> {
            Err::<_, Error>("使わない".into())
        }
    }

    struct FakeSearch(Mutex<Vec<String>>);

    impl Searcher for FakeSearch {
        fn search(&self, query: &str, _: usize) -> Result<Vec<SearchHit>> {
            self.0.lock().unwrap().push(query.to_string());
            Ok(vec![SearchHit {
                title: "BONX".into(),
                url: "https://example.com/bonx".into(),
                snippet: "ハンズフリー".into(),
            }])
        }
    }

    struct FakeMemory;

    impl MemoryStore for FakeMemory {
        fn remember_utterance(&self, _: &Utterance) -> Result<()> {
            Ok(())
        }
        fn remember_screen(&self, _: &ScreenText) -> Result<()> {
            Ok(())
        }
        fn remember_reaction(&self, _: &Suggestion, _: Reaction) -> Result<()> {
            Ok(())
        }
        fn recall(&self, _: &str, _: usize) -> Result<Vec<Recalled>> {
            Ok(vec![Recalled {
                text: "A案に決定".into(),
                evidence: Evidence::Assumption("偽".into()),
                score: 1.0,
            }])
        }
    }

    fn utterance(text: &str) -> Input {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        Input::Utterance(Utterance {
            channel: AudioChannel::System,
            text: text.into(),
            started_at: at,
            ended_at: at,
        })
    }

    fn engine(llm: Arc<FakeLlm>, search: Option<Arc<FakeSearch>>) -> Engine {
        let searcher = search.map(|s| s as Arc<dyn Searcher>);
        Engine::new(
            llm,
            searcher,
            Some(Arc::new(FakeMemory)),
            EngineConfig::default(),
        )
    }

    #[test]
    fn 手がかりのない発話では_llm_を呼ばない() {
        let llm = FakeLlm::new(r#"{"skill":"calc","confidence":0.9,"reason":""}"#);
        let mut engine = engine(llm.clone(), None);
        assert_eq!(engine.handle(utterance("そうですね")).unwrap(), None);
        assert_eq!(llm.calls(), 0);
    }

    #[test]
    fn 計算の補足を作る() {
        let llm = FakeLlm::new(r#"{"skill":"calc","confidence":0.9,"reason":""}"#);
        let mut engine = engine(llm, None);
        let suggestion = engine
            .handle(utterance("3カ月で10億件かぁ"))
            .unwrap()
            .unwrap();
        assert_eq!(suggestion.text, "秒間約127件、1日約1,100万件ですね");
    }

    #[test]
    fn 検索語は固定の形で_searcher_へ渡り出典が付く() {
        let llm = FakeLlm::new(r#"{"skill":"search","confidence":0.9,"reason":""}"#);
        let search = Arc::new(FakeSearch(Mutex::new(vec![])));
        let mut engine = engine(llm, Some(search.clone()));
        let suggestion = engine
            .handle(utterance("BONXの差別化ってなんなんですかね"))
            .unwrap()
            .unwrap();
        assert_eq!(*search.0.lock().unwrap(), vec!["BONX 差別化"]);
        assert!(matches!(suggestion.evidence[0], Evidence::Web { .. }));
    }

    #[test]
    fn 検索がなければ黙る() {
        let llm = FakeLlm::new(r#"{"skill":"search","confidence":0.9,"reason":""}"#);
        let mut engine = engine(llm, None);
        assert_eq!(
            engine.handle(utterance("BONXってなんですかね")).unwrap(),
            None
        );
    }

    #[test]
    fn 確信度が低ければ黙る() {
        let llm = FakeLlm::new(r#"{"skill":"calc","confidence":0.3,"reason":""}"#);
        let mut engine = engine(llm, None);
        assert_eq!(engine.handle(utterance("10億件かな")).unwrap(), None);
    }

    #[test]
    fn 画面の依頼からリマインドを提案するが実行はしない() {
        let llm = FakeLlm::new(r#"{"skill":"suggest","confidence":0.9,"reason":""}"#);
        let mut engine = engine(llm, None);
        let suggestion = engine
            .handle(Input::Screen(ScreenText {
                app: "Mail".into(),
                window_title: String::new(),
                text: "こちらご確認ください".into(),
                captured_at: SystemTime::UNIX_EPOCH,
            }))
            .unwrap()
            .unwrap();
        assert!(suggestion.text.ends_with("セットしますか?"));
        assert!(suggestion.action.is_some());
    }

    #[test]
    fn 同じ話題を二度言わない() {
        let llm = FakeLlm::new(r#"{"skill":"calc","confidence":0.9,"reason":""}"#);
        let mut engine = engine(llm, None);
        assert!(engine.handle(utterance("10億件かな")).unwrap().is_some());
        assert_eq!(engine.handle(utterance("10億件かな")).unwrap(), None);
    }

    #[test]
    fn 遅れた分はまとめて一度だけ判定する() {
        let llm = FakeLlm::new(r#"{"skill":"none","confidence":0.9,"reason":""}"#);
        let mut engine = engine(llm.clone(), None);
        let (input, inputs) = std::sync::mpsc::channel();
        let (outputs, _output) = std::sync::mpsc::channel();
        input.send(utterance("10億件かな")).unwrap();
        input.send(utterance("20億件かな")).unwrap();
        drop(input);
        engine.run(inputs, outputs);
        assert_eq!(llm.calls(), 1);
    }
}
