//! OCR の結果から、前に見ていない行だけを取り出す。
//!
//! OCR は同じ画面でも空白や数文字が揺れるので、正規化と近似の一致で吸収する。

use std::collections::{HashSet, VecDeque};

/// 空白を除き、全角の英数字を半角へ、英字を小文字へそろえる。比較にだけ使う。
pub fn normalize_line(line: &str) -> String {
    line.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// 時刻や記号だけの行、1 文字の行は、補足の材料にならないうえに揺れやすい。
fn is_noise(normalized: &str) -> bool {
    normalized.chars().count() < 2 || !normalized.chars().any(char::is_alphabetic)
}

fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let cost = diagonal + usize::from(x != y);
            diagonal = row[j + 1];
            row[j + 1] = cost.min(row[j] + 1).min(row[j + 1] + 1);
        }
    }
    row[b.len()]
}

/// 8 文字につき 1 文字までの違いを、OCR の揺れとみなす。
fn is_jitter(a: &[char], b: &[char]) -> bool {
    let allowed = a.len().max(b.len()) / 8;
    allowed > 0 && a.len().abs_diff(b.len()) <= allowed && edit_distance(a, b) <= allowed
}

/// 1 つの窓で見た行の記憶。
#[derive(Debug, Default)]
struct WindowLines {
    seen: HashSet<String>,
    order: VecDeque<String>,
    previous: Vec<Vec<char>>,
}

impl WindowLines {
    fn remember(&mut self, line: String, capacity: usize) {
        if self.seen.insert(line.clone()) {
            self.order.push_back(line);
        }
        while self.order.len() > capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.seen.remove(&oldest);
            }
        }
    }

    fn take_new(&mut self, lines: &[String], capacity: usize) -> Vec<String> {
        let mut fresh = Vec::new();
        let mut current = Vec::with_capacity(lines.len());
        for line in lines {
            let normalized = normalize_line(line);
            if is_noise(&normalized) {
                continue;
            }
            let chars: Vec<char> = normalized.chars().collect();
            let known = self.seen.contains(&normalized)
                || self.previous.iter().any(|p| is_jitter(p, &chars));
            if !known {
                fresh.push(line.trim().to_string());
            }
            self.remember(normalized, capacity);
            current.push(chars);
        }
        self.previous = current;
        fresh
    }
}

/// 窓ごとに記憶を分ける。窓を行き来しても同じ文字を流し直さない。
#[derive(Debug)]
pub struct TextDiffer {
    windows: VecDeque<(u64, WindowLines)>,
    max_windows: usize,
    max_lines: usize,
}

impl Default for TextDiffer {
    fn default() -> Self {
        Self::new(32, 2000)
    }
}

impl TextDiffer {
    pub fn new(max_windows: usize, max_lines: usize) -> Self {
        Self {
            windows: VecDeque::new(),
            max_windows: max_windows.max(1),
            max_lines,
        }
    }

    /// 新しい行を画面の順に返し、見た行として覚える。
    pub fn take_new(&mut self, window_id: u64, lines: &[String]) -> Vec<String> {
        let position = self.windows.iter().position(|(id, _)| *id == window_id);
        let mut entry = position
            .and_then(|at| self.windows.remove(at))
            .unwrap_or_else(|| (window_id, WindowLines::default()));
        let fresh = entry.1.take_new(lines, self.max_lines);
        self.windows.push_back(entry);
        if self.windows.len() > self.max_windows {
            self.windows.pop_front();
        }
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn 正規化は空白と全角の違いを消す() {
        assert_eq!(normalize_line("Ａ Ｂ１ こ ん に ち は"), "ab1こんにちは");
    }

    #[test]
    fn 初回はすべて新しい行になる() {
        let mut d = TextDiffer::default();
        assert_eq!(
            d.take_new(1, &lines(&["こちらご確認ください", "明日の予定"])),
            lines(&["こちらご確認ください", "明日の予定"])
        );
    }

    #[test]
    fn 増えた行だけを返す() {
        let mut d = TextDiffer::default();
        d.take_new(1, &lines(&["一行目の文章", "二行目の文章"]));
        assert_eq!(
            d.take_new(
                1,
                &lines(&["一行目の文章", "二行目の文章", "三行目を足した"])
            ),
            lines(&["三行目を足した"])
        );
    }

    #[test]
    fn スクロールで戻った行は流さない() {
        let mut d = TextDiffer::default();
        d.take_new(1, &lines(&["最初の見出し", "二番目の見出し"]));
        d.take_new(1, &lines(&["三番目の見出し"]));
        assert!(d.take_new(1, &lines(&["最初の見出し"])).is_empty());
    }

    #[test]
    fn 読み取りの一文字の揺れは同じ行とみなす() {
        let mut d = TextDiffer::default();
        d.take_new(1, &lines(&["来週の打ち合わせの資料をご確認ください"]));
        assert!(
            d.take_new(1, &lines(&["来週の打ち合わせの資料をご確詔ください"]))
                .is_empty()
        );
    }

    #[test]
    fn 短い行の一文字の違いは別の行とみなす() {
        let mut d = TextDiffer::default();
        d.take_new(1, &lines(&["A案に決定"]));
        assert_eq!(d.take_new(1, &lines(&["B案に決定"])), lines(&["B案に決定"]));
    }

    #[test]
    fn 時刻や記号だけの行は流さない() {
        let mut d = TextDiffer::default();
        assert_eq!(
            d.take_new(1, &lines(&["12:34", "---", "あ", "OK です"])),
            lines(&["OK です"])
        );
    }

    #[test]
    fn 窓ごとに別々に覚える() {
        let mut d = TextDiffer::default();
        d.take_new(1, &lines(&["同じ文章です"]));
        assert_eq!(
            d.take_new(2, &lines(&["同じ文章です"])),
            lines(&["同じ文章です"])
        );
        assert!(d.take_new(1, &lines(&["同じ文章です"])).is_empty());
    }

    #[test]
    fn 窓の記憶は上限を超えると古いものから消える() {
        let mut d = TextDiffer::new(2, 100);
        d.take_new(1, &lines(&["同じ文章です"]));
        d.take_new(2, &lines(&["別の文章です"]));
        d.take_new(3, &lines(&["別の文章です"]));
        assert_eq!(
            d.take_new(1, &lines(&["同じ文章です"])),
            lines(&["同じ文章です"])
        );
    }

    #[test]
    fn 行の記憶は上限を超えない() {
        let mut d = TextDiffer::new(1, 2);
        d.take_new(1, &lines(&["一番目の行", "二番目の行", "三番目の行"]));
        d.take_new(1, &lines(&["空の画面にする"]));
        assert_eq!(
            d.take_new(1, &lines(&["一番目の行"])),
            lines(&["一番目の行"])
        );
    }
}
