//! スキル。LLM の出力は信用せず、形を確かめてから使う。

use serde::Deserialize;
use zuchi_core::calc::{PeriodUnit, RateInput, convert_rate, format_ja, parse_ja_number};
use zuchi_core::ports::{Llm, LlmRequest, MemoryStore, Result, Searcher};
use zuchi_core::types::{Evidence, ProposedAction, Skill, Suggestion};

use crate::prompts;

fn ask<T: for<'de> Deserialize<'de>>(
    llm: &dyn Llm,
    system: &str,
    schema: &str,
    user: String,
    max_tokens: u32,
) -> Result<T> {
    let text = llm.complete(&LlmRequest {
        system: system.to_string(),
        user,
        json_schema: Some(schema.to_string()),
        max_tokens: Some(max_tokens),
    })?;
    Ok(serde_json::from_str(&text)?)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Extracted {
    op: String,
    total: String,
    unit: String,
    period: String,
    by: String,
    by_unit: String,
    a: String,
    a_unit: String,
    b: String,
}

/// 「3カ月」「2週間」「1日」を読む。
pub fn parse_period(text: &str) -> Option<(f64, PeriodUnit)> {
    let text = text.trim();
    for (suffix, unit) in [
        ("カ月", PeriodUnit::Month),
        ("ヶ月", PeriodUnit::Month),
        ("ケ月", PeriodUnit::Month),
        ("か月", PeriodUnit::Month),
        ("ヵ月", PeriodUnit::Month),
        ("週間", PeriodUnit::Week),
        ("時間", PeriodUnit::Hour),
        ("日間", PeriodUnit::Day),
        ("年間", PeriodUnit::Year),
        ("分間", PeriodUnit::Minute),
        ("秒間", PeriodUnit::Second),
        ("週", PeriodUnit::Week),
        ("日", PeriodUnit::Day),
        ("年", PeriodUnit::Year),
        ("分", PeriodUnit::Minute),
        ("秒", PeriodUnit::Second),
    ] {
        if let Some(number) = text.strip_suffix(suffix) {
            return Some((parse_ja_number(number)?, unit));
        }
    }
    None
}

pub fn calc(llm: &dyn Llm, context: &str) -> Result<Option<Suggestion>> {
    let extracted: Extracted = ask(
        llm,
        prompts::EXTRACT_SYSTEM,
        prompts::EXTRACT_SCHEMA,
        format!("{context}\n出力:"),
        200,
    )?;
    Ok(compute(&extracted).map(|(text, evidence)| Suggestion {
        skill: Skill::Calc,
        text,
        evidence,
        action: None,
    }))
}

fn compute(e: &Extracted) -> Option<(String, Vec<Evidence>)> {
    match e.op.as_str() {
        "rate" => {
            let (period, period_unit) = parse_period(&e.period)?;
            let output = convert_rate(&RateInput {
                amount: parse_ja_number(&e.total)?,
                unit: e.unit.clone(),
                period,
                period_unit,
            })?;
            let evidence = output
                .assumption
                .map(Evidence::Assumption)
                .into_iter()
                .collect();
            Some((output.text, evidence))
        }
        "divide" => {
            let by = parse_ja_number(&e.by)?;
            let value = parse_ja_number(&e.total)? / by;
            (by > 0.0 && value.is_finite() && value > 0.0).then(|| {
                let per = if e.by_unit.is_empty() {
                    "1あたり"
                } else {
                    &format!("1{}あたり", e.by_unit)
                };
                (
                    format!("{per}約{}{}ですね", format_ja(value), e.unit),
                    vec![],
                )
            })
        }
        "multiply" => {
            let value = parse_ja_number(&e.a)? * parse_ja_number(&e.b)?;
            (value.is_finite() && value > 0.0).then(|| {
                let unit = e.a_unit.split('/').next().unwrap_or("").trim();
                (format!("合計で約{}{unit}ですね", format_ja(value)), vec![])
            })
        }
        _ => None,
    }
}

#[derive(Deserialize)]
struct Query {
    query: String,
}

#[derive(Deserialize)]
struct Answer {
    found: bool,
    answer: String,
    source: i64,
}

/// 検索の補足と、話題を表す検索語を返す。LLM に渡す検索語の材料は会話と画面だけにする。
pub fn search(
    llm: &dyn Llm,
    searcher: &dyn Searcher,
    context: &str,
) -> Result<Option<(Suggestion, String)>> {
    let query: Query = ask(
        llm,
        prompts::QUERY_SYSTEM,
        prompts::QUERY_SCHEMA,
        format!("{context}\n出力:"),
        60,
    )?;
    let query = query.query.trim().to_string();
    if query.is_empty() {
        return Ok(None);
    }
    let hits = searcher.search(&query, 5)?;
    if hits.is_empty() {
        return Ok(None);
    }
    let mut materials = String::new();
    for (i, hit) in hits.iter().enumerate() {
        materials.push_str(&format!("{}. {} {}\n", i + 1, hit.title, hit.snippet));
    }
    let answer: Answer = ask(
        llm,
        prompts::ANSWER_SYSTEM,
        prompts::ANSWER_SCHEMA,
        format!("[質問の文脈]\n{context}\n[資料]\n{materials}出力:"),
        120,
    )?;
    if !answer.found || answer.answer.trim().is_empty() {
        return Ok(None);
    }
    let Some(hit) = usize::try_from(answer.source - 1)
        .ok()
        .and_then(|i| hits.get(i))
    else {
        return Ok(None);
    };
    let suggestion = Suggestion {
        skill: Skill::Search,
        text: answer.answer.trim().to_string(),
        evidence: vec![Evidence::Web {
            title: hit.title.clone(),
            url: hit.url.clone(),
        }],
        action: None,
    };
    Ok(Some((suggestion, query)))
}

pub fn memory(
    llm: &dyn Llm,
    store: &dyn MemoryStore,
    context: &str,
    last_text: &str,
) -> Result<Option<Suggestion>> {
    let recalled = store.recall(last_text, 5)?;
    if recalled.is_empty() {
        return Ok(None);
    }
    let mut materials = String::new();
    for (i, item) in recalled.iter().enumerate() {
        materials.push_str(&format!("{}. {}\n", i + 1, item.text));
    }
    let answer: Answer = ask(
        llm,
        prompts::ANSWER_SYSTEM,
        prompts::ANSWER_SCHEMA,
        format!("[質問の文脈]\n{context}\n[資料]\n{materials}出力:"),
        120,
    )?;
    if !answer.found || answer.answer.trim().is_empty() {
        return Ok(None);
    }
    let Some(item) = usize::try_from(answer.source - 1)
        .ok()
        .and_then(|i| recalled.get(i))
    else {
        return Ok(None);
    };
    Ok(Some(Suggestion {
        skill: Skill::Memory,
        text: answer.answer.trim().to_string(),
        evidence: vec![item.evidence.clone()],
        action: None,
    }))
}

#[derive(Deserialize)]
struct Reminder {
    title: String,
}

pub fn suggest(llm: &dyn Llm, context: &str) -> Result<Option<Suggestion>> {
    let reminder: Reminder = ask(
        llm,
        prompts::SUGGEST_SYSTEM,
        prompts::SUGGEST_SCHEMA,
        format!("{context}\n出力:"),
        60,
    )?;
    let title = reminder.title.trim().to_string();
    if title.is_empty() {
        return Ok(None);
    }
    Ok(Some(Suggestion {
        skill: Skill::Suggest,
        text: format!("「{title}」のリマインドをセットしますか?"),
        evidence: vec![],
        action: Some(ProposedAction::SetReminder { title, due: None }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 期間を読む() {
        assert_eq!(parse_period("3カ月"), Some((3.0, PeriodUnit::Month)));
        assert_eq!(parse_period("2週間"), Some((2.0, PeriodUnit::Week)));
        assert_eq!(parse_period("1日"), Some((1.0, PeriodUnit::Day)));
        assert_eq!(parse_period("そのうち"), None);
    }

    #[test]
    fn 割り算と掛け算をコードで計算する() {
        let divide = Extracted {
            op: "divide".into(),
            total: "1200万".into(),
            unit: "円".into(),
            by: "4".into(),
            by_unit: "人".into(),
            ..Default::default()
        };
        assert_eq!(compute(&divide).unwrap().0, "1人あたり約300万円ですね");

        let multiply = Extracted {
            op: "multiply".into(),
            a: "500".into(),
            a_unit: "円/個".into(),
            b: "3万".into(),
            ..Default::default()
        };
        assert_eq!(compute(&multiply).unwrap().0, "合計で約1,500万円ですね");
    }

    #[test]
    fn 読めない数では黙る() {
        let bad = Extracted {
            op: "rate".into(),
            total: "たくさん".into(),
            unit: "件".into(),
            period: "3カ月".into(),
            ..Default::default()
        };
        assert!(compute(&bad).is_none());
    }
}
