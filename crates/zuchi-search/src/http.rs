//! 検索の実装で共通の HTTP の設定。

use std::time::Duration;

use ureq::Agent;

use crate::SearchError;

/// 検索の補足は発話の終わりから 10 秒以内が目標なので、長く待たない
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);

pub fn agent(timeout: Duration) -> Agent {
    Agent::config_builder()
        // 検索語の行き先を設定の URL だけにする
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .build()
        .new_agent()
}

pub fn map_error(error: ureq::Error) -> SearchError {
    match error {
        ureq::Error::Timeout(_) => SearchError::Timeout,
        // ureq のエラーは URL を含むことがあり、URL には検索語が入る
        other => SearchError::Network(redact_query(&other.to_string())),
    }
}

fn redact_query(message: &str) -> String {
    match message.split_once('?') {
        Some((head, _)) => format!("{head}?…"),
        None => message.to_owned(),
    }
}

/// 根拠として画面に出すので、http と https の URL だけを通す。
pub fn is_web_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// 検索の結果の断片から HTML のタグを除く。
pub fn strip_tags(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => output.push(c),
            _ => {}
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn タグを除く() {
        assert_eq!(strip_tags("<strong>BONX</strong> の特徴"), "BONX の特徴");
    }

    #[test]
    fn エラーの文から検索語を除く() {
        assert_eq!(
            redact_query("failed http://x/search?q=秘密&format=json"),
            "failed http://x/search?…"
        );
    }
}
