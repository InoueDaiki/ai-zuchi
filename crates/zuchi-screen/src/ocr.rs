//! OCR の結果の整形。OS に依存しない部分。

/// OCR の 1 行。座標は左上を原点に、画像の幅と高さを 1 とした割合。
#[derive(Clone, Debug, PartialEq)]
pub struct OcrLine {
    pub text: String,
    pub confidence: f32,
    pub x: f32,
    pub y: f32,
    pub height: f32,
}

/// 上から下、同じ高さなら左から右へ並べる。
///
/// 縦の位置の差が行の高さの半分までなら、同じ段として扱う。
pub fn sort_reading_order(lines: &mut [OcrLine]) {
    lines.sort_by(|a, b| a.y.total_cmp(&b.y));
    let mut start = 0;
    while start < lines.len() {
        let top = lines[start].y;
        let tolerance = lines[start].height / 2.0;
        let end = lines[start..]
            .iter()
            .position(|line| line.y - top > tolerance)
            .map_or(lines.len(), |offset| start + offset);
        lines[start..end].sort_by(|a, b| a.x.total_cmp(&b.x));
        start = end;
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{30FF}' // 句読点、ひらがな、カタカナ
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FF00}'..='\u{FFEF}')
}

/// Windows.Media.Ocr は日本語の 1 文字ごとに語を分けるので、語をつなぐ。
/// 英単語の間の空白は残す。
pub fn join_words<S: AsRef<str>>(words: &[S]) -> String {
    let mut line = String::new();
    for word in words {
        let word = word.as_ref().trim();
        let (Some(first), Some(last)) = (word.chars().next(), line.chars().last()) else {
            line.push_str(word);
            continue;
        };
        if !is_cjk(first) && !is_cjk(last) {
            line.push(' ');
        }
        line.push_str(word);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f32, y: f32) -> OcrLine {
        OcrLine {
            text: text.into(),
            confidence: 1.0,
            x,
            y,
            height: 0.04,
        }
    }

    fn texts(lines: &[OcrLine]) -> Vec<&str> {
        lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn 上から下へ並べる() {
        let mut lines = [line("下", 0.1, 0.8), line("上", 0.1, 0.1)];
        sort_reading_order(&mut lines);
        assert_eq!(texts(&lines), ["上", "下"]);
    }

    #[test]
    fn 同じ段は左から右へ並べる() {
        let mut lines = [
            line("時刻", 0.8, 0.101),
            line("名前", 0.1, 0.1),
            line("本文", 0.1, 0.2),
        ];
        sort_reading_order(&mut lines);
        assert_eq!(texts(&lines), ["名前", "時刻", "本文"]);
    }

    #[test]
    fn 日本語の語は空白なしでつなぐ() {
        assert_eq!(join_words(&["こ", "ん", "に", "ち", "は"]), "こんにちは");
    }

    #[test]
    fn 英単語の間は空白を残す() {
        assert_eq!(join_words(&["Hello", "World"]), "Hello World");
    }

    #[test]
    fn 日本語と英語の境目は詰める() {
        assert_eq!(
            join_words(&["議", "事", "録", "AI", "zuchi", "で", "す"]),
            "議事録AI zuchiです"
        );
    }

    #[test]
    fn 空の語は無視する() {
        assert_eq!(join_words(&["", "あ", " ", "い"]), "あい");
    }
}
