//! 段階 2 の軽いルールの絞り込み。手がかりは緩く取る。

use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

const NUM: &str = "[0-9〇一二三四五六七八九十百千万億兆]";

static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"{NUM}+(\.[0-9]+)?\s*[万億兆]|[0-9]{{3,}}|[0-9],[0-9]{{3}}|[0-9]+[kKmM]\b|千(件|人|円|台|個)"
    ))
    .expect("正規表現")
});
static QUESTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(ですか|ますか|ましたか|でしたっけ|だっけ|たっけ|ですかね|ますかね|かな|かなあ|かなぁ|かね|だろう|でしょう|どうなん|なんなん|なんで|なぜ|どこ|どう|いつ|だれ|誰|何|なに|いくら|どのくらい|どれくらい|[?])").expect("正規表現")
});
static REQUEST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(ください|下さい|お願い|おねがい|までに|期限|締め切り|締切|リマインド|送ります|やっておきます|確認します|対応します|提出)").expect("正規表現")
});
static MEMORY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(この前|このあいだ|こないだ|前回|前に|以前|先週|先月|決めた|決まった|言ってた|話した|覚えて)").expect("正規表現")
});

fn normalize(text: &str) -> String {
    text.nfkc().collect()
}

/// 発話に手がかりがあれば `true`。`false` なら LLM を呼ばない。
pub fn utterance_passes(text: &str) -> bool {
    let text = normalize(text);
    [&*NUMBER, &*QUESTION, &*REQUEST, &*MEMORY]
        .iter()
        .any(|re| re.is_match(&text))
}

/// 画面の文字に依頼の手がかりがあれば `true`。
pub fn screen_passes(text: &str) -> bool {
    REQUEST.is_match(&normalize(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 手がかりのある発話を通す() {
        assert!(utterance_passes("3カ月で10億件かぁ"));
        assert!(utterance_passes("BONXの差別化ってなんなんですかね"));
        assert!(utterance_passes("前回決めたやつ"));
        assert!(utterance_passes("3カ月で\u{FF11}\u{FF10}億件"));
    }

    #[test]
    fn 手がかりのない発話は通さない() {
        assert!(!utterance_passes("そうですね"));
        assert!(!utterance_passes("3時に行きます"));
    }

    #[test]
    fn 画面の依頼を通す() {
        assert!(screen_passes("こちらご確認ください"));
        assert!(!screen_passes("ファイル 編集 表示"));
    }
}
