//! ブラウザのグループポリシー (レジストリ) によるブロック。
//!
//! - Chrome / Edge / Brave: URLBlocklist にルールを書く (サブドメインとパス指定に対応)
//! - Firefox: WebsiteFilter\Block にパターンを書く
//! - 全ブラウザ: 暗号化 DNS (DoH) を無効化し、hosts ファイルを迂回されないようにする
//!
//! ユーザーが元々設定していたポリシー値は残し、このアプリが書いた値だけを出し入れする。

use std::collections::BTreeMap;
use std::io;

use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};

const CHROMIUM_BASES: &[&str] = &[
    r"SOFTWARE\Policies\Google\Chrome",
    r"SOFTWARE\Policies\Microsoft\Edge",
    r"SOFTWARE\Policies\BraveSoftware\Brave",
];
const FIREFOX_BASE: &str = r"SOFTWARE\Policies\Mozilla\Firefox";

/// Firefox WebsiteFilter に書く値 (match pattern 形式)
pub fn firefox_patterns(rules: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for r in rules {
        let (host, path) = match r.split_once('/') {
            Some((h, p)) => (h, format!("/{p}*")),
            None => (r.as_str(), "/*".to_string()),
        };
        out.push(format!("*://{host}{path}"));
        out.push(format!("*://*.{host}{path}"));
    }
    out
}

/// 現在の値から「前回このアプリが書いた値」を除き、今回の値を足す。
/// 戻り値は (書き込むべき値, このアプリが実際に足した値)。ユーザーが元々持っていた値は自分のものとして記録しない。
pub fn merge_list(current: &[String], previously_ours: &[String], ours_now: &[String]) -> (Vec<String>, Vec<String>) {
    let mut out: Vec<String> = current
        .iter()
        .filter(|v| !previously_ours.contains(v))
        .cloned()
        .collect();
    let mut added = Vec::new();
    for v in ours_now {
        if !out.contains(v) {
            out.push(v.clone());
            added.push(v.clone());
        }
    }
    (out, added)
}

fn read_numbered(key: &RegKey) -> Vec<(u32, String)> {
    let mut values: Vec<(u32, String)> = key
        .enum_values()
        .filter_map(|r| r.ok())
        .filter_map(|(name, _)| {
            let n: u32 = name.parse().ok()?;
            let v: String = key.get_value(&name).ok()?;
            Some((n, v))
        })
        .collect();
    values.sort_by_key(|(n, _)| *n);
    values
}

/// 番号付きリストのキーを `ours_now` が入った状態にする。戻り値は (変更したか, 実際に足した値)。
fn apply_list(path: &str, previously_ours: &[String], ours_now: &[String]) -> io::Result<(bool, Vec<String>)> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let (key, _) = hklm.create_subkey(path)?;
    let numbered = read_numbered(&key);
    let current: Vec<String> = numbered.iter().map(|(_, v)| v.clone()).collect();
    let (desired, added) = merge_list(&current, previously_ours, ours_now);
    if desired == current {
        return Ok((false, added));
    }
    for (n, _) in &numbered {
        key.delete_value(n.to_string())?;
    }
    for (i, v) in desired.iter().enumerate() {
        key.set_value((i + 1).to_string(), v)?;
    }
    Ok((true, added))
}

/// ブロック状態に応じてポリシーを書き換える。`ours` はキーごとに前回書いた値 (更新される)。
pub fn apply(rules: &[String], blocked: bool, ours: &mut BTreeMap<String, Vec<String>>) -> io::Result<bool> {
    let mut changed = false;
    let mut targets: Vec<(String, Vec<String>)> = CHROMIUM_BASES
        .iter()
        .map(|b| (format!(r"{b}\URLBlocklist"), rules.to_vec()))
        .collect();
    targets.push((format!(r"{FIREFOX_BASE}\WebsiteFilter\Block"), firefox_patterns(rules)));

    for (path, patterns) in targets {
        let now: Vec<String> = if blocked { patterns } else { Vec::new() };
        let prev = ours.get(&path).cloned().unwrap_or_default();
        let (c, added) = apply_list(&path, &prev, &now)?;
        changed |= c;
        ours.insert(path, added);
    }
    changed |= enforce_doh_off()?;
    Ok(changed)
}

/// 暗号化 DNS を常に無効にしておく (インストール中は時間帯に関係なく維持)。
fn enforce_doh_off() -> io::Result<bool> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut changed = false;
    for base in CHROMIUM_BASES {
        let (key, _) = hklm.create_subkey(base)?;
        if key.get_value::<String, _>("DnsOverHttpsMode").ok().as_deref() != Some("off") {
            key.set_value("DnsOverHttpsMode", &"off")?;
            changed = true;
        }
    }
    let (doh, _) = hklm.create_subkey(format!(r"{FIREFOX_BASE}\DNSOverHTTPS"))?;
    for (name, want) in [("Enabled", 0u32), ("Locked", 1u32)] {
        if doh.get_value::<u32, _>(name).ok() != Some(want) {
            doh.set_value(name, &want)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// アンインストール時: このアプリが書いた値をすべて消す。
pub fn remove_all(ours: &mut BTreeMap<String, Vec<String>>) -> io::Result<()> {
    for (path, prev) in ours.iter() {
        apply_list(path, prev, &[])?;
    }
    ours.clear();
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for base in CHROMIUM_BASES {
        if let Ok(key) = hklm.open_subkey_with_flags(base, KEY_ALL_ACCESS) {
            let _ = key.delete_value("DnsOverHttpsMode");
        }
    }
    let _ = hklm.delete_subkey(format!(r"{FIREFOX_BASE}\DNSOverHTTPS"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn merge_keeps_user_values() {
        let current = v(&["user.com", "old.com"]);
        let (merged, added) = merge_list(&current, &v(&["old.com"]), &v(&["a.com", "user.com"]));
        assert_eq!(merged, v(&["user.com", "a.com"]));
        assert_eq!(added, v(&["a.com"]));
        // 解除してもユーザーが元々入れていた user.com は残る
        assert_eq!(merge_list(&merged, &added, &[]).0, v(&["user.com"]));
    }

    #[test]
    fn firefox_patterns_cover_subdomains_and_paths() {
        let p = firefox_patterns(&v(&["a.com", "b.jp/video"]));
        assert_eq!(p, v(&["*://a.com/*", "*://*.a.com/*", "*://b.jp/video*", "*://*.b.jp/video*"]));
    }
}
