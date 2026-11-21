//! ブロックの本体。サービスのメインループから一定間隔で `tick` を呼ぶ。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::clock::TrustedClock;
use crate::config::{Config, Paths, Removal, State, save_json};
use crate::harden::{self, CREATE_NO_WINDOW};
use crate::ipc::{self, Op, Response};
use crate::schedule::{format_hhmm, is_blocked, minute_of_day, parse_hhmm};
use crate::sites::{hostnames, normalize_rule};
use crate::{hosts, policy};

const FILE_SHARE_READ: u32 = 0x1;
const HARDEN_INTERVAL: Duration = Duration::from_secs(60);
const LOG_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// ブラウザポリシーを書く
    pub policies: bool,
    /// サービス・フォルダの守りを再適用する
    pub harden: bool,
    /// CLI からの要求を処理し、状態ファイルを書く
    pub ipc: bool,
    /// ブロック中は hosts ファイルを書き換え不可の状態で開いておく
    pub lock_hosts: bool,
}

pub struct Engine {
    paths: Paths,
    opts: Options,
    pub config: Config,
    pub state: State,
    clock: TrustedClock,
    hosts_lock: Option<File>,
    last_harden: Option<Instant>,
    /// ブロック時間中か (恒久ブロックは時間に関係なく常に効いている)
    pub blocked: bool,
}

/// hosts は文字コードが不定 (Shift_JIS のコメントなど) なので、1 バイト = 1 文字として
/// 読み書きし、管理区間以外のバイト列をそのまま保つ。
fn bytes_to_latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}
fn latin1_to_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

pub fn flush_dns() {
    let _ = Command::new("ipconfig").arg("/flushdns").creation_flags(CREATE_NO_WINDOW).output();
}

impl Engine {
    pub fn new(paths: Paths, opts: Options, config: Config, state: State, clock: TrustedClock) -> Self {
        Engine { paths, opts, config, state, clock, hosts_lock: None, last_harden: None, blocked: false }
    }

    pub fn log(&self, msg: &str) {
        if !self.opts.ipc {
            println!("{msg}");
            return;
        }
        let path = self.paths.log();
        if fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX_BYTES) {
            let _ = fs::rename(&path, path.with_extension("log.old"));
        }
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "[{}] {msg}", crate::clock::system_now_ms());
        }
    }

    /// 設定の時刻が壊れていても安全側 (21:00-07:00) で判定する。
    fn window(&self) -> (u16, u16) {
        self.config.window().unwrap_or((21 * 60, 7 * 60))
    }

    pub fn tick(&mut self) {
        self.clock.tick();
        let now = self.clock.now_ms();
        let (start, end) = self.window();
        self.blocked = is_blocked(minute_of_day(now, self.config.utc_offset_minutes), start, end);

        if self.opts.ipc {
            for req in ipc::take_requests(&self.paths) {
                let (ok, message) = self.handle(req.op);
                ipc::respond(&self.paths, &Response { id: req.id, ok, message });
            }
        }

        let rules = self.config.active_rules(self.blocked);
        if let Err(e) = self.apply_hosts(&rules) {
            self.log(&format!("hosts の更新に失敗: {e}"));
        }
        if self.opts.policies {
            match policy::apply(&rules, &mut self.state.policy_ours) {
                Ok(true) => self.log(&format!("ブラウザポリシーを更新 (ブロック={})", self.blocked)),
                Ok(false) => {}
                Err(e) => self.log(&format!("ブラウザポリシーの更新に失敗: {e}")),
            }
        }
        if self.opts.harden && self.last_harden.is_none_or(|t| t.elapsed() >= HARDEN_INTERVAL) {
            self.last_harden = Some(Instant::now());
            if let Err(e) = harden::lock_down(&self.paths) {
                self.log(&format!("保護設定の再適用に失敗: {e}"));
            }
        }

        self.state.blocked = self.blocked;
        self.state.ntp_synced = self.clock.is_synced();
        self.state.trusted_now_ms = now;
        self.state.last_seen_ms = now;
        if self.opts.ipc {
            let _ = save_json(&self.paths.state(), &self.state);
        }
    }

    fn apply_hosts(&mut self, rules: &[String]) -> std::io::Result<()> {
        let names = hostnames(rules);
        let current = bytes_to_latin1(&fs::read(&self.paths.hosts)?);
        let desired = hosts::render(&current, &names);
        if desired != current {
            self.hosts_lock = None;
            fs::write(&self.paths.hosts, latin1_to_bytes(&desired))?;
            flush_dns();
            self.log(&format!("hosts を更新 (ブロック対象 {} 件)", names.len()));
        }
        if !names.is_empty() && self.opts.lock_hosts {
            if self.hosts_lock.is_none() {
                // 読み取り専用で開き、他プロセスには読み取りだけを許す = 書き換え・削除・改名ができなくなる
                self.hosts_lock = Some(OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&self.paths.hosts)?);
            }
        } else {
            self.hosts_lock = None;
        }
        Ok(())
    }

    fn save_config(&self) -> Result<(), String> {
        save_json(&self.paths.config(), &self.config).map_err(|e| format!("設定を保存できませんでした: {e}"))
    }

    fn blocked_message(&self, what: &str) -> String {
        let (s, e) = self.window();
        format!("ブロック時間中 ({}〜{}) は{what}できません。", format_hhmm(s), format_hhmm(e))
    }

    fn handle(&mut self, op: Op) -> (bool, String) {
        match op {
            Op::Add { sites, permanent } => {
                let mut added = Vec::new();
                let mut existing = Vec::new();
                let mut invalid = Vec::new();
                for s in &sites {
                    match normalize_rule(s) {
                        Ok(r) if self.config.add_site(&r, permanent) => added.push(r),
                        Ok(r) => existing.push(r),
                        Err(e) => invalid.push(e),
                    }
                }
                if !added.is_empty()
                    && let Err(e) = self.save_config()
                {
                    return (false, e);
                }
                let mut msg = Vec::new();
                if !added.is_empty() {
                    let kind = if permanent { "恒久ブロック" } else { "時間帯ブロック" };
                    msg.push(format!("{kind}に追加しました: {}", added.join(", ")));
                }
                if !existing.is_empty() {
                    msg.push(format!("登録済みです: {}", existing.join(", ")));
                }
                msg.extend(invalid.iter().cloned());
                self.log(&format!("サイト追加 (恒久={permanent}): {added:?}"));
                (invalid.is_empty(), msg.join("\n"))
            }
            Op::Remove { sites } => {
                if self.blocked {
                    return (false, self.blocked_message("サイトを外すことは") + "追加だけは可能です。");
                }
                let mut removed = Vec::new();
                let mut permanent = Vec::new();
                let mut missing = Vec::new();
                for s in &sites {
                    match normalize_rule(s) {
                        Ok(r) => match self.config.remove_site(&r) {
                            Removal::Removed => removed.push(r),
                            Removal::Permanent => permanent.push(r),
                            Removal::NotFound => missing.push(r),
                        },
                        Err(e) => missing.push(e),
                    }
                }
                if !removed.is_empty()
                    && let Err(e) = self.save_config()
                {
                    return (false, e);
                }
                let mut msg = Vec::new();
                if !removed.is_empty() {
                    msg.push(format!("外しました: {}", removed.join(", ")));
                }
                if !permanent.is_empty() {
                    msg.push(format!("恒久ブロックのため外せません: {}", permanent.join(", ")));
                }
                if !missing.is_empty() {
                    msg.push(format!("登録されていません: {}", missing.join(", ")));
                }
                self.log(&format!("サイト削除: {removed:?}"));
                (missing.is_empty() && permanent.is_empty(), msg.join("\n"))
            }
            Op::SetHours { start, end } => {
                if self.blocked {
                    return (false, self.blocked_message("ブロック時間の変更は"));
                }
                let (s, e) = match (parse_hhmm(&start), parse_hhmm(&end)) {
                    (Ok(s), Ok(e)) => (s, e),
                    (Err(x), _) | (_, Err(x)) => return (false, x),
                };
                if s == e {
                    return (false, "開始と終了が同じ時刻です。".into());
                }
                self.config.block_start = format_hhmm(s);
                self.config.block_end = format_hhmm(e);
                if let Err(x) = self.save_config() {
                    return (false, x);
                }
                self.log(&format!("ブロック時間を変更: {}-{}", format_hhmm(s), format_hhmm(e)));
                (true, format!("ブロック時間を {}〜{} に変更しました。", format_hhmm(s), format_hhmm(e)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_roundtrip_preserves_any_bytes() {
        let bytes: Vec<u8> = (0u8..=255).collect();
        assert_eq!(latin1_to_bytes(&bytes_to_latin1(&bytes)), bytes);
    }
}
