//! 設定 (config.json) と実行状態 (state.json) と、各ファイルの置き場所。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::schedule::parse_hhmm;
use crate::sites::DEFAULT_SITES;

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
    /// ユーザーが追加したサイト
    #[serde(default)]
    pub custom_sites: Vec<String>,
    /// ユーザーが外した既定サイト
    #[serde(default)]
    pub disabled_defaults: Vec<String>,
}

impl Config {
    pub fn new(utc_offset_minutes: i32) -> Self {
        Config {
            block_start: "21:00".into(),
            block_end: "07:00".into(),
            utc_offset_minutes,
            custom_sites: Vec::new(),
            disabled_defaults: Vec::new(),
        }
    }

    pub fn window(&self) -> Result<(u16, u16), String> {
        Ok((parse_hhmm(&self.block_start)?, parse_hhmm(&self.block_end)?))
    }

    /// 既定サイト (外したものを除く) + 追加サイト
    pub fn effective_sites(&self) -> Vec<String> {
        let mut out: Vec<String> = DEFAULT_SITES
            .iter()
            .filter(|s| !self.disabled_defaults.iter().any(|d| d == *s))
            .map(|s| s.to_string())
            .collect();
        for s in &self.custom_sites {
            if !out.contains(s) {
                out.push(s.clone());
            }
        }
        out
    }

    /// サイトを追加する。戻り値は実際に増えたかどうか。
    pub fn add_site(&mut self, rule: &str) -> bool {
        if let Some(i) = self.disabled_defaults.iter().position(|d| d == rule) {
            self.disabled_defaults.remove(i);
            return true;
        }
        if DEFAULT_SITES.contains(&rule) || self.custom_sites.iter().any(|s| s == rule) {
            return false;
        }
        self.custom_sites.push(rule.to_string());
        true
    }

    /// サイトを外す。戻り値は実際に減ったかどうか。
    pub fn remove_site(&mut self, rule: &str) -> bool {
        if let Some(i) = self.custom_sites.iter().position(|s| s == rule) {
            self.custom_sites.remove(i);
            return true;
        }
        if DEFAULT_SITES.contains(&rule) && !self.disabled_defaults.iter().any(|d| d == rule) {
            self.disabled_defaults.push(rule.to_string());
            return true;
        }
        false
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
    fn add_and_remove_custom_and_default_sites() {
        let mut c = Config::new(540);
        let base = c.effective_sites().len();

        assert!(c.add_site("anime.example.com"));
        assert!(!c.add_site("anime.example.com"));
        assert!(!c.add_site("youtube.com"));
        assert_eq!(c.effective_sites().len(), base + 1);

        assert!(c.remove_site("youtube.com"));
        assert!(!c.effective_sites().contains(&"youtube.com".to_string()));
        assert!(!c.remove_site("youtube.com"));
        assert!(c.add_site("youtube.com"));
        assert!(c.effective_sites().contains(&"youtube.com".to_string()));

        assert!(c.remove_site("anime.example.com"));
        assert!(!c.remove_site("unknown.example.com"));
        assert_eq!(c.effective_sites().len(), base);
    }
}
