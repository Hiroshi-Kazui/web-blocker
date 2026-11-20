//! hosts ファイルの内容を組み立てる純粋関数群。
//! このアプリが書く行はマーカーで囲み、それ以外の行には触れない。

pub const BEGIN: &str = "# >>> animeblock: managed section (do not edit) >>>";
pub const END: &str = "# <<< animeblock: managed section <<<";

/// 管理区間を取り除いた内容を返す (末尾の空行は詰める)。
pub fn strip_managed(content: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in content.lines() {
        let t = line.trim();
        if t == BEGIN {
            inside = true;
            continue;
        }
        if t == END {
            inside = false;
            continue;
        }
        if !inside {
            out.push(line);
        }
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\r\n")
}

/// 管理区間を `hostnames` で置き換えた内容を返す。空なら管理区間ごと消す。
pub fn render(content: &str, hostnames: &[String]) -> String {
    let mut out = strip_managed(content);
    if !hostnames.is_empty() {
        if !out.is_empty() {
            out.push_str("\r\n\r\n");
        }
        out.push_str(BEGIN);
        out.push_str("\r\n");
        for h in hostnames {
            out.push_str(&format!("0.0.0.0 {h}\r\n::1 {h}\r\n"));
        }
        out.push_str(END);
    }
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &str = "# comment\r\n127.0.0.1 localhost\r\n";

    #[test]
    fn adds_and_removes_section_without_touching_other_lines() {
        let hosts = vec!["a.com".to_string(), "www.a.com".to_string()];
        let blocked = render(ORIGINAL, &hosts);
        assert!(blocked.starts_with("# comment\r\n127.0.0.1 localhost\r\n\r\n"));
        assert!(blocked.contains("0.0.0.0 a.com\r\n::1 a.com\r\n"));
        assert!(blocked.contains("0.0.0.0 www.a.com\r\n"));

        let unblocked = render(&blocked, &[]);
        assert_eq!(unblocked, ORIGINAL);
    }

    #[test]
    fn render_is_idempotent() {
        let hosts = vec!["a.com".to_string()];
        let once = render(ORIGINAL, &hosts);
        assert_eq!(render(&once, &hosts), once);
    }

    #[test]
    fn replaces_tampered_section() {
        let hosts = vec!["a.com".to_string()];
        let tampered = format!("{ORIGINAL}{BEGIN}\r\n# 0.0.0.0 a.com\r\n{END}\r\n");
        assert_eq!(render(&tampered, &hosts), render(ORIGINAL, &hosts));
    }

    #[test]
    fn handles_lf_and_empty_file() {
        assert_eq!(render("127.0.0.1 x\n", &[]), "127.0.0.1 x\r\n");
        let r = render("", &["a.com".to_string()]);
        assert!(r.starts_with(BEGIN));
    }
}
