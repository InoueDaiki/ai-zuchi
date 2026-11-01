//! 読まない窓を決める。読むかどうか分からない窓は読まない。

/// OS から得た窓の情報。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: u64,
    pub pid: u32,
    /// macOS は bundle identifier、Windows は実行ファイル名
    pub app_id: String,
    pub app_name: String,
    /// 分からないときは None
    pub title: Option<String>,
}

/// 読まない理由。窓の名前は持たせない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// 読める窓がない
    NoWindow,
    /// 対象外のアプリ
    ExcludedApp,
    /// 対象外の語を題名かアプリ名に含む
    ExcludedTitle,
    /// アプリや題名が分からず、対象外かどうかを判断できない
    Unknown,
}

/// 対象外の一覧。比較は大文字と小文字を区別しない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcludeList {
    /// 自分のアプリの識別子。前面にあっても読み飛ばし、その後ろの窓を見る
    pub own_apps: Vec<String>,
    /// 読まないアプリの識別子(bundle identifier または実行ファイル名)
    pub apps: Vec<String>,
    /// 題名かアプリ名に含まれていたら読まない語
    pub keywords: Vec<String>,
}

fn to_strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

impl Default for ExcludeList {
    fn default() -> Self {
        Self {
            own_apps: to_strings(&["dev.inouedaiki.ai-zuchi", "desktop.exe", "ai-zuchi.exe"]),
            apps: to_strings(&[
                // パスワードマネージャー(macOS)
                "com.1password.1password",
                "com.agilebits.onepassword7",
                "com.bitwarden.desktop",
                "com.dashlane.dashlanephonefinal",
                "com.lastpass.LastPass",
                "org.keepassxc.keepassxc",
                "com.apple.Passwords",
                "com.apple.keychainaccess",
                // パスワードマネージャー(Windows)
                "1Password.exe",
                "Bitwarden.exe",
                "Dashlane.exe",
                "KeePass.exe",
                "KeePassXC.exe",
                "CredentialUIBroker.exe",
            ]),
            keywords: to_strings(&[
                // ブラウザの拡張や、Web 版のパスワードマネージャー
                "1Password",
                "Bitwarden",
                "LastPass",
                "パスワード",
                "password",
                // 銀行は Web で使うことが多いので、題名で止める
                "銀行",
                "バンキング",
                "bank",
                "三菱UFJ",
                "三井住友",
                "みずほ",
                "ゆうちょ",
                "りそな",
                "SMBC",
                "証券",
            ]),
        }
    }
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

impl ExcludeList {
    fn is_own(&self, window: &WindowInfo) -> bool {
        window.pid == std::process::id()
            || self
                .own_apps
                .iter()
                .any(|a| eq_ignore_case(a, &window.app_id))
    }

    /// 読んでよければ `None`、読まないなら理由を返す。
    pub fn check(&self, window: &WindowInfo) -> Option<SkipReason> {
        if self.is_own(window) || self.apps.iter().any(|a| eq_ignore_case(a, &window.app_id)) {
            return Some(SkipReason::ExcludedApp);
        }
        let title = window.title.as_deref().unwrap_or("").trim();
        let haystack = format!("{title}\n{}", window.app_name).to_lowercase();
        let excluded = self
            .keywords
            .iter()
            .any(|keyword| haystack.contains(&keyword.to_lowercase()));
        if excluded {
            return Some(SkipReason::ExcludedTitle);
        }
        if window.app_id.trim().is_empty() || title.is_empty() {
            return Some(SkipReason::Unknown);
        }
        None
    }

    /// 手前から順に並んだ窓から、読む窓を 1 つ選ぶ。
    ///
    /// 自分の窓は読み飛ばす。最も手前の他人の窓が対象外なら、その後ろも読まない。
    /// 本人が見ているのは手前の窓で、後ろを読むと文脈が食い違うため。
    pub fn pick<'a>(&self, windows: &'a [WindowInfo]) -> Result<&'a WindowInfo, SkipReason> {
        let front = windows
            .iter()
            .find(|window| !self.is_own(window))
            .ok_or(SkipReason::NoWindow)?;
        match self.check(front) {
            Some(reason) => Err(reason),
            None => Ok(front),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(app_id: &str, title: Option<&str>) -> WindowInfo {
        WindowInfo {
            id: 1,
            pid: u32::MAX,
            app_id: app_id.into(),
            app_name: "アプリ".into(),
            title: title.map(Into::into),
        }
    }

    #[test]
    fn 普通の窓は読む() {
        let list = ExcludeList::default();
        assert_eq!(
            list.check(&window("com.tinyspeck.slackmacgap", Some("Slack"))),
            None
        );
    }

    #[test]
    fn パスワードマネージャーは読まない() {
        let list = ExcludeList::default();
        for app in [
            "com.1password.1password",
            "COM.1PASSWORD.1PASSWORD",
            "1Password.exe",
        ] {
            assert_eq!(
                list.check(&window(app, Some("金庫"))),
                Some(SkipReason::ExcludedApp)
            );
        }
    }

    #[test]
    fn 銀行のサイトは題名で読まない() {
        let list = ExcludeList::default();
        for title in ["○○銀行 - ログイン", "Online BANKING", "ネットバンキング"] {
            assert_eq!(
                list.check(&window("com.google.Chrome", Some(title))),
                Some(SkipReason::ExcludedTitle)
            );
        }
    }

    #[test]
    fn 対象外は設定で足せる() {
        let mut list = ExcludeList::default();
        list.apps.push("com.example.secret".into());
        list.keywords.push("社外秘".into());
        assert_eq!(
            list.check(&window("com.example.secret", Some("x"))),
            Some(SkipReason::ExcludedApp)
        );
        assert_eq!(
            list.check(&window("com.example.other", Some("社外秘の資料"))),
            Some(SkipReason::ExcludedTitle)
        );
    }

    #[test]
    fn 題名が分からない窓は読まない() {
        let list = ExcludeList::default();
        for title in [None, Some(""), Some("  ")] {
            assert_eq!(
                list.check(&window("com.example.app", title)),
                Some(SkipReason::Unknown)
            );
        }
        assert_eq!(
            list.check(&window("", Some("題名"))),
            Some(SkipReason::Unknown)
        );
    }

    #[test]
    fn 自分の窓は飛ばして後ろを読む() {
        let list = ExcludeList::default();
        let windows = [
            window("dev.inouedaiki.ai-zuchi", Some("補足")),
            window("com.apple.Safari", Some("記事")),
        ];
        assert_eq!(list.pick(&windows), Ok(&windows[1]));
    }

    #[test]
    fn 自分の処理の窓も飛ばす() {
        let list = ExcludeList::default();
        let mut own = window("com.example.app", Some("題名"));
        own.pid = std::process::id();
        assert_eq!(list.check(&own), Some(SkipReason::ExcludedApp));
    }

    #[test]
    fn 手前が対象外なら後ろも読まない() {
        let list = ExcludeList::default();
        let windows = [
            window("com.1password.1password", Some("金庫")),
            window("com.apple.Safari", Some("記事")),
        ];
        assert_eq!(list.pick(&windows), Err(SkipReason::ExcludedApp));
    }

    #[test]
    fn 窓がなければ読まない() {
        let list = ExcludeList::default();
        assert_eq!(list.pick(&[]), Err(SkipReason::NoWindow));
    }
}
