//! 計算スキル。LLM は数値の抽出だけを行い、換算はここで行う。

const MINUTE: f64 = 60.0;
const HOUR: f64 = 3_600.0;
const DAY: f64 = 86_400.0;
const DAYS_PER_MONTH: f64 = 365.0 / 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeriodUnit {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

/// 「3カ月で10億件」の形の量。
#[derive(Clone, Debug, PartialEq)]
pub struct RateInput {
    pub amount: f64,
    pub unit: String,
    pub period: f64,
    pub period_unit: PeriodUnit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RateOutput {
    pub text: String,
    /// 計算の前提。日数に丸めたときだけ入る
    pub assumption: Option<String>,
}

/// 量を別の単位時間に換算する。入力がおかしいときは黙るために `None` を返す。
pub fn convert_rate(input: &RateInput) -> Option<RateOutput> {
    let valid = input.amount.is_finite()
        && input.amount > 0.0
        && input.period.is_finite()
        && input.period > 0.0;
    if !valid {
        return None;
    }
    let (seconds, assumption) = period_seconds(input.period, input.period_unit);
    let per_second = input.amount / seconds;
    let unit = &input.unit;

    let mut parts = Vec::new();
    if per_second >= 1.0 {
        parts.push(format!("秒間約{}{unit}", format_ja(per_second)));
    } else if per_second * MINUTE >= 1.0 {
        parts.push(format!("毎分約{}{unit}", format_ja(per_second * MINUTE)));
    } else if per_second * HOUR >= 1.0 {
        parts.push(format!("1時間約{}{unit}", format_ja(per_second * HOUR)));
    }
    if seconds > DAY && per_second * DAY >= 1.0 {
        parts.push(format!("1日約{}{unit}", format_ja(per_second * DAY)));
    }
    if parts.is_empty() {
        return None;
    }
    Some(RateOutput {
        text: format!("{}ですね", parts.join("、")),
        assumption,
    })
}

fn period_seconds(period: f64, unit: PeriodUnit) -> (f64, Option<String>) {
    match unit {
        PeriodUnit::Second => (period, None),
        PeriodUnit::Minute => (period * MINUTE, None),
        PeriodUnit::Hour => (period * HOUR, None),
        PeriodUnit::Day => (period * DAY, None),
        PeriodUnit::Week => (period * 7.0 * DAY, None),
        PeriodUnit::Month => {
            let days = (period * DAYS_PER_MONTH).round().max(1.0);
            let assumption = format!("{}カ月を{}日として計算", trim(period), days);
            (days * DAY, Some(assumption))
        }
        PeriodUnit::Year => (period * 365.0 * DAY, None),
    }
}

/// 有効数字 3 桁で、万や億を使って書く。
pub fn format_ja(value: f64) -> String {
    let rounded = round_significant(value, 3);
    for (scale, name) in [(1e12, "兆"), (1e8, "億"), (1e4, "万")] {
        if rounded >= scale {
            return format!("{}{name}", group(rounded / scale));
        }
    }
    group(rounded)
}

fn round_significant(value: f64, digits: i32) -> f64 {
    if value == 0.0 {
        return 0.0;
    }
    let factor = 10f64.powi(digits - 1 - value.abs().log10().floor() as i32);
    (value * factor).round() / factor
}

fn group(value: f64) -> String {
    let text = trim(value);
    let (integer, fraction) = match text.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (text.as_str(), None),
    };
    let mut grouped = String::new();
    for (i, c) in integer.chars().enumerate() {
        if i > 0 && (integer.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    match fraction {
        Some(f) => format!("{grouped}.{f}"),
        None => grouped,
    }
}

fn trim(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// 「10億」「1.5万」「3千」「2億5000万」を数にする。読めなければ `None`。
pub fn parse_ja_number(text: &str) -> Option<f64> {
    let cleaned: String = text
        .chars()
        .filter(|c| !matches!(c, ',' | '\u{FF0C}' | ' ' | '\u{3000}'))
        .map(|c| match c {
            '\u{FF10}'..='\u{FF19}' => {
                char::from_u32(c as u32 - '\u{FF10}' as u32 + '0' as u32).unwrap_or(c)
            }
            '\u{FF0E}' => '.',
            '〇' | '零' => '0',
            '一' => '1',
            '二' => '2',
            '三' => '3',
            '四' => '4',
            '五' => '5',
            '六' => '6',
            '七' => '7',
            '八' => '8',
            '九' => '9',
            _ => c,
        })
        .collect();
    let normalized = ["約", "およそ"]
        .iter()
        .find_map(|prefix| cleaned.strip_prefix(prefix))
        .unwrap_or(&cleaned);
    let normalized = ["くらい", "ほど", "弱", "強", "円", "件", "人"]
        .iter()
        .find_map(|suffix| normalized.strip_suffix(suffix))
        .unwrap_or(normalized);
    if normalized.is_empty() {
        return None;
    }

    let mut total = 0.0;
    let mut section = 0.0;
    let mut digits = String::new();
    for c in normalized.chars() {
        if c.is_ascii_digit() || c == '.' {
            digits.push(c);
            continue;
        }
        let number = take_number(&mut digits)?;
        match c {
            '十' | '百' | '千' => {
                let scale = match c {
                    '十' => 10.0,
                    '百' => 100.0,
                    _ => 1_000.0,
                };
                section += number.unwrap_or(1.0) * scale;
            }
            '万' | '億' | '兆' => {
                let scale = match c {
                    '万' => 1e4,
                    '億' => 1e8,
                    _ => 1e12,
                };
                let value = section + number.unwrap_or(if section > 0.0 { 0.0 } else { 1.0 });
                total += value * scale;
                section = 0.0;
            }
            _ => return None,
        }
    }
    let rest = take_number(&mut digits)?.unwrap_or(0.0);
    Some(total + section + rest)
}

/// 数字がなければ `Some(None)`、数字として読めなければ `None`。
fn take_number(digits: &mut String) -> Option<Option<f64>> {
    if digits.is_empty() {
        return Some(None);
    }
    let parsed = digits.parse().ok()?;
    digits.clear();
    Some(Some(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(amount: f64, unit: &str, period: f64, period_unit: PeriodUnit) -> Option<RateOutput> {
        convert_rate(&RateInput {
            amount,
            unit: unit.to_string(),
            period,
            period_unit,
        })
    }

    #[test]
    fn 三カ月で十億件を換算する() {
        let output = rate(1e9, "件", 3.0, PeriodUnit::Month).unwrap();
        assert_eq!(output.text, "秒間約127件、1日約1,100万件ですね");
        assert_eq!(output.assumption.as_deref(), Some("3カ月を91日として計算"));
    }

    #[test]
    fn 少ない量は単位時間を変える() {
        let output = rate(600.0, "件", 1.0, PeriodUnit::Day).unwrap();
        assert_eq!(output.text, "1時間約25件ですね");
    }

    #[test]
    fn 意味のない入力では黙る() {
        assert_eq!(rate(0.0, "件", 3.0, PeriodUnit::Month), None);
        assert_eq!(rate(1.0, "件", 0.0, PeriodUnit::Month), None);
        assert_eq!(rate(1.0, "件", 1.0, PeriodUnit::Year), None);
        assert_eq!(rate(f64::NAN, "件", 1.0, PeriodUnit::Day), None);
    }

    #[test]
    fn 大きな数を日本語で書く() {
        assert_eq!(format_ja(127.187), "127");
        assert_eq!(format_ja(10_989_011.0), "1,100万");
        assert_eq!(format_ja(1_234_567_890.0), "12.3億");
        assert_eq!(format_ja(0.4166), "0.42");
    }

    #[test]
    fn 日本語の数を読む() {
        assert_eq!(parse_ja_number("五千万"), Some(5e7));
        assert_eq!(parse_ja_number("約10億件"), Some(1e9));
        assert_eq!(parse_ja_number("3万くらい"), Some(3e4));
        assert_eq!(parse_ja_number("10億"), Some(1e9));
        assert_eq!(parse_ja_number("1.5万"), Some(15_000.0));
        assert_eq!(parse_ja_number("3千"), Some(3_000.0));
        assert_eq!(parse_ja_number("2億5000万"), Some(250_000_000.0));
        assert_eq!(parse_ja_number("1,200"), Some(1_200.0));
        assert_eq!(parse_ja_number("3千万"), Some(30_000_000.0));
        assert_eq!(parse_ja_number("\u{FF11}\u{FF10}億"), Some(1e9));
        assert_eq!(parse_ja_number("1\u{FF0C}200"), Some(1_200.0));
        assert_eq!(parse_ja_number("たくさん"), None);
        assert_eq!(parse_ja_number(""), None);
    }
}
