//! 設定 (config.json) と実行状態 (state.json) と、各ファイルの置き場所。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::schedule::parse_hhmm;
use crate::sites::{PERMANENT_DEFAULTS, TIMED_DEFAULTS};

pub const SERVICE_NAME: &str = "AnimeBlock";
pub const SERVICE_DISPLAY_NAME: &str = "AnimeBlock 夜間ブロッカー";

#[derive(Debug, Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub install_dir: PathBuf,
    pub hosts: PathBuf,
}

impl Paths {
    pub fn system() -> Self {
        let program_data = std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into());
        let program_files = std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into());
        let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        Paths {
            data_dir: Path::new(&program_data).join(SERVICE_NAME),
            install_dir: Path::new(&program_files).join(SERVICE_NAME),
            hosts: Path::new(&windir).join("System32\\drivers\\etc\\hosts"),
        }
    }
    pub fn config(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }
    pub fn state(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }
    pub fn log(&self) -> PathBuf {
        self.data_dir.join("service.log")
    }
    pub fn requests(&self) -> PathBuf {
        self.data_dir.join("requests")
    }
    pub fn responses(&self) -> PathBuf {
        self.data_dir.join("responses")
    }
    pub fn installed_exe(&self) -> PathBuf {
        self.install_dir.join("animeblock.exe")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub block_start: String,
    pub block_end: String,
    /// 時差 (分)。インストール時の値で固定し、OS のタイムゾーン変更では動かない。
    pub utc_offset_minutes: i32,
    /// ユーザーが追加した時間帯ブロックのサイト
    #[serde(default)]
    pub custom_sites: Vec<String>,
    /// ユーザーが外した時間帯ブロックの既定サイト (恒久ブロックの既定サイトはここに入れても外れない)
    #[serde(default)]
    pub disabled_defaults: Vec<String>,
    /// ユーザーが追加した恒久ブロックのサイト
    #[serde(default)]
    pub permanent_sites: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    Removed,
    /// 恒久ブロックなので外せない
    Permanent,
    NotFound,
}

impl Config {
    pub fn new(utc_offset_minutes: i32) -> Self {
        Config {
            block_start: "21:00".into(),
            block_end: "07:00".into(),
            utc_offset_minutes,
            custom_sites: Vec::new(),
            disabled_defaults: Vec::new(),
            permanent_sites: Vec::new(),
        }
    }

    pub fn window(&self) -> Result<(u16, u16), String> {
        Ok((parse_hhmm(&self.block_start)?, parse_hhmm(&self.block_end)?))
    }

    pub fn is_permanent(&self, rule: &str) -> bool {
        PERMANENT_DEFAULTS.contains(&rule) || self.permanent_sites.iter().any(|s| s == rule)
    }

    /// 恒久ブロックのサイト (既定 + 追加)
    pub fn permanent_rules(&self) -> Vec<String> {
        let mut out: Vec<String> = PERMANENT_DEFAULTS.iter().map(|s| s.to_string()).collect();
        for s in &self.permanent_sites {
            if !out.contains(s) {
                out.push(s.clone());
            }
        }
        out
    }

    /// 時間帯ブロックのサイト (既定から外したものを除く + 追加。恒久ブロックと重なるものは除く)
    pub fn timed_rules(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let defaults = TIMED_DEFAULTS
            .iter()
            .filter(|s| !self.disabled_defaults.iter().any(|d| d == *s))
            .map(|s| s.to_string());
        for s in defaults.chain(self.custom_sites.iter().cloned()) {
            if !self.is_permanent(&s) && !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    /// 今ブロックすべきサイト。恒久ブロックは常に、時間帯ブロックはブロック時間中だけ含む。
    pub fn active_rules(&self, in_block_hours: bool) -> Vec<String> {
        let mut out = self.permanent_rules();
        if in_block_hours {
            out.extend(self.timed_rules());
        }
        out
    }

    /// サイトを追加する。戻り値は実際に増えたかどうか。
    /// 時間帯ブロックのサイトを恒久として追加し直すと、恒久ブロックに格上げされる。
    pub fn add_site(&mut self, rule: &str, permanent: bool) -> bool {
        if self.is_permanent(rule) {
            return false;
        }
        if permanent {
            self.custom_sites.retain(|s| s != rule);
            self.permanent_sites.push(rule.to_string());
            return true;
        }
        if let Some(i) = self.disabled_defaults.iter().position(|d| d == rule) {
            self.disabled_defaults.remove(i);
            return true;
        }
        if TIMED_DEFAULTS.contains(&rule) || self.custom_sites.iter().any(|s| s == rule) {
            return false;
        }
        self.custom_sites.push(rule.to_string());
        true
    }

    /// 時間帯ブロックのサイトを外す。恒久ブロックのサイトは外せない。
    pub fn remove_site(&mut self, rule: &str) -> Removal {
        if self.is_permanent(rule) {
            return Removal::Permanent;
        }
        if let Some(i) = self.custom_sites.iter().position(|s| s == rule) {
            self.custom_sites.remove(i);
            return Removal::Removed;
        }
        if TIMED_DEFAULTS.contains(&rule) && !self.disabled_defaults.iter().any(|d| d == rule) {
            self.disabled_defaults.push(rule.to_string());
            return Removal::Removed;
        }
        Removal::NotFound
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub blocked: bool,
    pub ntp_synced: bool,
    /// 信頼できる現在時刻 (UTC, Unix ミリ秒)
    pub trusted_now_ms: i64,
    /// これまでに観測した最も新しい時刻。時計を巻き戻されても、これより前には戻らない。
    pub last_seen_ms: i64,
    /// ブラウザポリシーのレジストリキーごとに、このアプリが書いた値
    #[serde(default)]
    pub policy_ours: BTreeMap<String, Vec<String>>,
}

pub fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<T> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// 一時ファイルに書いてから置き換える (書きかけのファイルを残さない)。
pub fn save_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_sites_can_be_added_and_removed() {
        let mut c = Config::new(540);
        let base = c.timed_rules().len();

        assert!(c.add_site("video.example.com", false));
        assert!(!c.add_site("video.example.com", false));
        assert!(!c.add_site("youtube.com", false));
        assert_eq!(c.timed_rules().len(), base + 1);

        assert_eq!(c.remove_site("youtube.com"), Removal::Removed);
        assert!(!c.timed_rules().contains(&"youtube.com".to_string()));
        assert_eq!(c.remove_site("youtube.com"), Removal::NotFound);
        assert!(c.add_site("youtube.com", false));
        assert!(c.timed_rules().contains(&"youtube.com".to_string()));

        assert_eq!(c.remove_site("video.example.com"), Removal::Removed);
        assert_eq!(c.remove_site("unknown.example.com"), Removal::NotFound);
        assert_eq!(c.timed_rules().len(), base);
    }

    #[test]
    fn permanent_sites_cannot_be_removed() {
        let mut c = Config::new(540);
        assert_eq!(c.remove_site("primevideo.com"), Removal::Permanent);
        assert!(c.add_site("anime.example.com", true));
        assert!(!c.add_site("anime.example.com", true));
        assert!(!c.add_site("anime.example.com", false));
        assert_eq!(c.remove_site("anime.example.com"), Removal::Permanent);
        // 古い設定で既定サイトを外していても、恒久ブロックの既定サイトは残る
        c.disabled_defaults.push("netflix.com".into());
        assert!(c.permanent_rules().contains(&"netflix.com".to_string()));
    }

    #[test]
    fn timed_site_can_be_promoted_to_permanent() {
        let mut c = Config::new(540);
        assert!(c.add_site("video.example.com", false));
        assert!(c.add_site("video.example.com", true));
        assert!(!c.timed_rules().contains(&"video.example.com".to_string()));
        assert_eq!(c.remove_site("video.example.com"), Removal::Permanent);
        // youtube.com を恒久に格上げすると、時間帯ブロック側からは消える
        assert!(c.add_site("youtube.com", true));
        assert!(!c.timed_rules().contains(&"youtube.com".to_string()));
    }

    #[test]
    fn active_rules_follow_block_hours() {
        let c = Config::new(540);
        let day = c.active_rules(false);
        let night = c.active_rules(true);
        assert!(day.contains(&"primevideo.com".to_string()));
        assert!(!day.contains(&"youtube.com".to_string()));
        assert!(night.contains(&"primevideo.com".to_string()));
        assert!(night.contains(&"youtube.com".to_string()));
    }
}
