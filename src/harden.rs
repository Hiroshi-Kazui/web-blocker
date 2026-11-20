//! サービスとファイルを管理者からも守る設定、およびその解除。
//!
//! - サービスのアクセス権: 管理者は「状態確認・開始」だけ。停止・設定変更・削除は SYSTEM のみ
//! - セーフモードでもサービスが起動するよう登録する
//! - 設定フォルダ・実行ファイルフォルダは SYSTEM だけが書ける

use std::io;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use winreg::RegKey;
use winreg::enums::HKEY_LOCAL_MACHINE;

use crate::config::{Paths, SERVICE_NAME};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const SID_SYSTEM: &str = "*S-1-5-18";
const SID_ADMINS: &str = "*S-1-5-32-544";
const SID_USERS: &str = "*S-1-5-32-545";

/// SYSTEM: 全権 / Administrators: 状態確認・開始・読み取りのみ / 対話ユーザー: 状態確認のみ
const HARDENED_SDDL: &str = "D:(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;SY)(A;;CCLCSWRPLOCRRC;;;BA)(A;;CCLCSWLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)";
/// Windows がサービス作成時に付ける標準のアクセス権
const DEFAULT_SDDL: &str = "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)";

const SAFEBOOT_KEYS: &[&str] = &[
    r"SYSTEM\CurrentControlSet\Control\SafeBoot\Minimal",
    r"SYSTEM\CurrentControlSet\Control\SafeBoot\Network",
];

pub fn run_hidden(program: &str, args: &[&str]) -> io::Result<()> {
    let out = Command::new(program).args(args).creation_flags(CREATE_NO_WINDOW).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{program} {} が失敗しました: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

fn icacls(dir: &Path, args: &[&str]) -> io::Result<()> {
    let d = dir.to_string_lossy().to_string();
    let mut all = vec![d.as_str()];
    all.extend_from_slice(args);
    run_hidden("icacls", &all)
}

/// フォルダを「SYSTEM 全権、管理者・ユーザーは読み取りのみ」にする。要求・応答フォルダだけは管理者も書ける。
pub fn restrict_dirs(paths: &Paths) -> io::Result<()> {
    let system_f = format!("{SID_SYSTEM}:(OI)(CI)F");
    let admins_rx = format!("{SID_ADMINS}:(OI)(CI)RX");
    let users_rx = format!("{SID_USERS}:(OI)(CI)RX");
    let admins_m = format!("{SID_ADMINS}:(OI)(CI)M");
    for dir in [&paths.data_dir, &paths.install_dir] {
        icacls(dir, &["/inheritance:r", "/grant:r", &system_f, &admins_rx, &users_rx, "/T", "/C", "/Q"])?;
    }
    for dir in [paths.requests(), paths.responses()] {
        icacls(&dir, &["/grant", &admins_m, "/T", "/C", "/Q"])?;
    }
    Ok(())
}

/// サービス (SYSTEM) から定期的に呼ぶ: 守りの設定を再適用する。
pub fn lock_down(paths: &Paths) -> io::Result<()> {
    run_hidden("sc.exe", &["sdset", SERVICE_NAME, HARDENED_SDDL])?;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let (svc, _) = hklm.create_subkey(format!(r"SYSTEM\CurrentControlSet\Services\{SERVICE_NAME}"))?;
    svc.set_value("Start", &2u32)?; // 自動起動
    svc.set_value("FailureActionsOnNonCrashFailures", &1u32)?;
    for base in SAFEBOOT_KEYS {
        let (k, _) = hklm.create_subkey(format!(r"{base}\{SERVICE_NAME}"))?;
        k.set_value("", &"Service")?;
    }

    restrict_dirs(paths)?;
    for dir in [&paths.data_dir, &paths.install_dir] {
        icacls(dir, &["/setowner", SID_SYSTEM, "/T", "/C", "/Q"])?;
    }
    Ok(())
}

/// アンインストール時 (サービスから呼ぶ): 守りの設定を外し、管理者が削除できる状態に戻す。
pub fn relax(paths: &Paths) -> io::Result<()> {
    run_hidden("sc.exe", &["sdset", SERVICE_NAME, DEFAULT_SDDL])?;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    for base in SAFEBOOT_KEYS {
        let _ = hklm.delete_subkey(format!(r"{base}\{SERVICE_NAME}"));
    }
    let admins_f = format!("{SID_ADMINS}:(OI)(CI)F");
    for dir in [&paths.data_dir, &paths.install_dir] {
        icacls(dir, &["/setowner", SID_ADMINS, "/T", "/C", "/Q"])?;
        icacls(dir, &["/grant", &admins_f, "/T", "/C", "/Q"])?;
    }
    Ok(())
}
