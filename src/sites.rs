//! ブロック対象サイトの既定リストと、ユーザー入力の正規化。
//!
//! ルールは `host` または `host/path` の形をとる。
//! - `host` : hosts ファイルとブラウザポリシーの両方でブロックする (ポリシーはサブドメインも含む)
//! - `host/path` : ブラウザポリシーだけでブロックする (hosts はパス単位で止められないため)

/// 既定のブロック対象。hosts はワイルドカード非対応なので、主要なサブドメインも列挙する。
pub const DEFAULT_SITES: &[&str] = &[
    // YouTube
    "youtube.com",
    "m.youtube.com",
    "music.youtube.com",
    "youtu.be",
    "youtube-nocookie.com",
    "youtubei.googleapis.com",
    "googlevideo.com",
    // dアニメストア / Lemino (docomo)
    "animestore.docomo.ne.jp",
    "lemino.docomo.ne.jp",
    // ABEMA
    "abema.tv",
    // Netflix
    "netflix.com",
    // Prime Video
    "primevideo.com",
    "amazon.co.jp/gp/video",
    "amazon.co.jp/prime-video",
    // U-NEXT
    "unext.jp",
    "video.unext.jp",
    // DMM TV
    "tv.dmm.com",
    // Disney+
    "disneyplus.com",
    // Hulu (日本)
    "hulu.jp",
    // FOD / TVer / TELASA
    "fod.fujitv.co.jp",
    "tver.jp",
    "telasa.jp",
    // バンダイチャンネル
    "b-ch.com",
    // ニコニコ
    "nicovideo.jp",
    "sp.nicovideo.jp",
    "live.nicovideo.jp",
    "ch.nicovideo.jp",
    "nicochannel.jp",
    // 海外のアニメ配信
    "crunchyroll.com",
    "hidive.com",
    "bilibili.com",
    "bilibili.tv",
];

/// ユーザー入力を `host` または `host/path` に正規化する。
pub fn normalize_rule(input: &str) -> Result<String, String> {
    let mut s = input.trim().to_ascii_lowercase();
    for prefix in ["https://", "http://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
        }
    }
    if let Some(rest) = s.strip_prefix("*.") {
        s = rest.to_string();
    }
    // クエリ・フラグメントは捨てる
    if let Some(i) = s.find(['?', '#']) {
        s.truncate(i);
    }
    let s = s.trim_end_matches('/').to_string();

    let (host, path) = match s.split_once('/') {
        Some((h, p)) => (h, Some(p)),
        None => (s.as_str(), None),
    };
    if host.contains(':') {
        return Err(format!("ポート番号は指定できません: {input}"));
    }
    let host_ok = !host.is_empty()
        && host.contains('.')
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && !host.contains("..")
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !host_ok {
        return Err(format!("ドメインとして解釈できません: {input}"));
    }
    if let Some(p) = path {
        let path_ok = p
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~/%".contains(c));
        if !path_ok {
            return Err(format!("パスに使えない文字があります: {input}"));
        }
    }
    Ok(s)
}

/// hosts ファイルへ書くホスト名の一覧 (パス付きルールは除外、www. を補完、重複除去・整列済み)。
pub fn hostnames(rules: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for rule in rules.iter().filter(|r| !r.contains('/')) {
        out.push(rule.clone());
        if !rule.starts_with("www.") {
            out.push(format!("www.{rule}"));
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_urls() {
        assert_eq!(normalize_rule("https://www.Example.com/").unwrap(), "www.example.com");
        assert_eq!(normalize_rule("*.example.com").unwrap(), "example.com");
        assert_eq!(
            normalize_rule("http://amazon.co.jp/gp/video?ref=x#top").unwrap(),
            "amazon.co.jp/gp/video"
        );
        assert_eq!(normalize_rule("  anime.example.net  ").unwrap(), "anime.example.net");
    }

    #[test]
    fn rejects_invalid() {
        for bad in ["", "localhost", "exa mple.com", "example..com", ".example.com", "example.com:8080", "例え.jp"] {
            assert!(normalize_rule(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn default_sites_are_normalized() {
        for s in DEFAULT_SITES {
            assert_eq!(normalize_rule(s).unwrap(), *s);
        }
    }

    #[test]
    fn hostnames_expand_www_and_skip_paths() {
        let rules = vec![
            "example.com".to_string(),
            "www.foo.jp".to_string(),
            "amazon.co.jp/gp/video".to_string(),
            "example.com".to_string(),
        ];
        assert_eq!(hostnames(&rules), vec!["example.com", "www.example.com", "www.foo.jp"]);
    }
}
