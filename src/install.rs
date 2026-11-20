//! インストール / アンインストール。

use std::ffi::OsString;
use std::fs;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use windows_service::service::{ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState, ServiceType};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use crate::config::{Config, Paths, SERVICE_DISPLAY_NAME, SERVICE_NAME, save_json};
use crate::harden::{self, CREATE_NO_WINDOW, run_hidden};
use crate::ipc::{self, Op};

const DETACHED_PROCESS: u32 = 0x0000_0008;

/// OS の現在のタイムゾーンの UTC からの差 (分)。インストール時に一度だけ使う。
pub fn local_utc_offset_minutes() -> i32 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    const TIME_ZONE_ID_DAYLIGHT: u32 = 2;
    let mut tz: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    let id = unsafe { GetTimeZoneInformation(&mut tz) };
    let extra = if id == TIME_ZONE_ID_DAYLIGHT { tz.DaylightBias } else { tz.StandardBias };
    -(tz.Bias + extra)
}

fn manager(access: ServiceManagerAccess) -> Result<ServiceManager, String> {
    ServiceManager::local_computer(None::<&str>, access)
        .map_err(|_| "サービス管理に接続できません。管理者としてターミナルを開いて実行してください。".to_string())
}

pub fn is_installed() -> bool {
    manager(ServiceManagerAccess::CONNECT)
        .and_then(|m| m.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS).map_err(|e| e.to_string()))
        .is_ok()
}

pub fn service_state() -> Option<ServiceState> {
    let m = manager(ServiceManagerAccess::CONNECT).ok()?;
    let s = m.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS).ok()?;
    Some(s.query_status().ok()?.current_state)
}

pub fn install(paths: &Paths) -> Result<(), String> {
    let mgr = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
    if mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS).is_ok() {
        return Err("既にインストールされています。".into());
    }

    for dir in [&paths.data_dir, &paths.requests(), &paths.responses(), &paths.install_dir] {
        fs::create_dir_all(dir).map_err(|e| format!("{} を作成できません: {e}", dir.display()))?;
    }
    if !paths.config().exists() {
        let config = Config::new(local_utc_offset_minutes());
        save_json(&paths.config(), &config).map_err(|e| format!("設定を書けません: {e}"))?;
    }
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let target = paths.installed_exe();
    if me != target {
        fs::copy(&me, &target).map_err(|e| format!("{} へコピーできません: {e}", target.display()))?;
    }

    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: target.clone(),
        launch_arguments: vec![OsString::from("service")],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let service = mgr
        .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS)
        .map_err(|e| format!("サービスを登録できません: {e}"))?;
    let _ = service.set_description("指定した時間帯に動画配信サイトへのアクセスをブロックします。");
    service.set_failure_actions_on_non_crash_failures(true).map_err(|e| e.to_string())?;
    // 異常終了・強制終了されたら 1 秒後に再起動する
    run_hidden("sc.exe", &["failure", SERVICE_NAME, "reset=", "86400", "actions=", "restart/1000/restart/1000/restart/1000"])
        .map_err(|e| e.to_string())?;

    harden::restrict_dirs(paths).map_err(|e| format!("フォルダの権限を設定できません: {e}"))?;
    service.start::<&str>(&[]).map_err(|e| format!("サービスを開始できません: {e}"))?;
    Ok(())
}

pub fn uninstall(paths: &Paths) -> Result<(), String> {
    if !is_installed() {
        return Err("インストールされていません。".into());
    }
    let resp = ipc::send(paths, Op::Uninstall)?;
    if !resp.ok {
        return Err(resp.message);
    }
    println!("{}", resp.message);

    // サービスが自分で止まるのを待つ
    let started = Instant::now();
    while service_state() != Some(ServiceState::Stopped) {
        if started.elapsed() > Duration::from_secs(30) {
            return Err("サービスが停止しませんでした。少し待ってから再度実行してください。".into());
        }
        thread::sleep(Duration::from_millis(300));
    }
    let mgr = manager(ServiceManagerAccess::CONNECT)?;
    let service = mgr
        .open_service(SERVICE_NAME, ServiceAccess::DELETE)
        .map_err(|e| format!("サービスを開けません: {e}"))?;
    service.delete().map_err(|e| format!("サービスを削除できません: {e}"))?;

    fs::remove_dir_all(&paths.data_dir).map_err(|e| format!("{} を削除できません: {e}", paths.data_dir.display()))?;
    let me = std::env::current_exe().unwrap_or_default();
    if me.starts_with(&paths.install_dir) {
        // 実行中の自分自身は消せないので、終了後に別プロセスで消す
        let dir = paths.install_dir.to_string_lossy().to_string();
        let _ = Command::new("cmd")
            .args(["/C", &format!("ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{dir}\"")])
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn();
    } else {
        fs::remove_dir_all(&paths.install_dir).map_err(|e| format!("{} を削除できません: {e}", paths.install_dir.display()))?;
    }
    Ok(())
}
