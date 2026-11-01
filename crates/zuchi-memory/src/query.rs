//! 全文検索の語の切り出し。
//!
//! trigram は 3 文字以上の部分文字列を探すので、文のままでは当たらない。形態素解析は
//! 持たず、次の 2 通りで切り出して併せる。どちらも誤って拾う語は OR の検索の雑音で済む。
//!
//! - ひらがなと記号で区切った、漢字、カタカナ、英数字のまとまり(「差別化」)
//! - 助詞で区切り、両端のひらがなを除いたまとまり(「打ち合わせの」から「打ち合」)

/// 検索に使う語。`long` は FTS5、`short` は 2 文字の語で、LIKE で探す。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Terms {
    pub long: Vec<String>,
    pub short: Vec<String>,
}

pub(crate) fn split_terms(query: &str) -> Terms {
    let runs = query.split(|c: char| !is_content_char(c));
    let okurigana = query
        .split(|c: char| !is_content_char(c) && !is_hiragana(c) || is_particle(c))
        .map(|word| word.trim_matches(is_hiragana));
    let mut words: Vec<String> = runs
        .chain(okurigana)
        .filter(|word| word.chars().count() >= 2)
        .map(str::to_owned)
        .collect();
    // すべてひらがなの問い合わせなどは、全体を 1 語として扱う
    if words.is_empty() {
        let whole = query.trim();
        if whole.chars().count() >= 2 {
            words.push(whole.to_owned());
        }
    }
    let mut terms = Terms::default();
    for word in words {
        let list = if word.chars().count() >= 3 {
            &mut terms.long
        } else {
            &mut terms.short
        };
        if !list.contains(&word) {
            list.push(word);
        }
    }
    terms
}

fn is_content_char(c: char) -> bool {
    matches!(c,
        '0'..='9' | 'a'..='z' | 'A'..='Z'
        | '\u{30A1}'..='\u{30FA}' | '\u{30FC}'      // カタカナと長音
        | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}' // 漢字
        | '\u{FF10}'..='\u{FF19}' | '\u{FF21}'..='\u{FF3A}' | '\u{FF41}'..='\u{FF5A}' // 全角英数字
        | '\u{FF66}'..='\u{FF9D}') // 半角カタカナ
}

fn is_hiragana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{3096}')
}

fn is_particle(c: char) -> bool {
    matches!(
        c,
        'の' | 'は' | 'が' | 'を' | 'に' | 'で' | 'と' | 'も' | 'へ' | 'や' | 'か'
    )
}

/// 語を引用符で囲み、FTS5 の構文として解釈されないようにする。
pub(crate) fn fts_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// LIKE の特殊文字を無効にする。`ESCAPE '\'` と組み合わせる。
pub(crate) fn like_pattern(term: &str) -> String {
    let escaped = term
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 文から語を切り出す() {
        let terms = split_terms("BONXの差別化ってなんなんですかね");
        assert_eq!(terms.long, ["BONX", "差別化"]);
        assert!(terms.short.is_empty());
    }

    #[test]
    fn 送り仮名を含む語も拾う() {
        let terms = split_terms("打ち合わせの結論はどうなったっけ");
        assert_eq!(terms.long, ["打ち合"]);
        assert_eq!(terms.short, ["結論"]);
    }

    #[test]
    fn 二文字の語は分けて持つ() {
        let terms = split_terms("前に決めたA案の価格、どうなったっけ");
        assert_eq!(terms.short, ["A案", "価格"]);
    }

    #[test]
    fn ひらがなだけなら全体を使う() {
        assert_eq!(split_terms("あれどうなった").long, ["あれどうなった"]);
    }

    #[test]
    fn 一文字は語にしない() {
        assert_eq!(split_terms("あ"), Terms::default());
    }

    #[test]
    fn 引用符を無害にする() {
        assert_eq!(
            fts_query(&["a\"b OR c".to_owned(), "NEAR".to_owned()]),
            r#""a""b OR c" OR "NEAR""#
        );
    }

    #[test]
    fn like_の特殊文字を無効にする() {
        assert_eq!(like_pattern("10%_"), "%10\\%\\_%");
    }
}
