//! Windows サービスとしての起動処理とメインループ。

use std::ffi::OsString;
use std::sync::mpsc;
use std::time::Duration;

use windows_service::service::{ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use crate::clock::TrustedClock;
use crate::config::{Config, Paths, SERVICE_NAME, State, load_json};
use crate::engine::{Engine, Options};

const TICK: Duration = Duration::from_secs(2);

define_windows_service!(ffi_service_main, service_main);

pub fn run_dispatcher() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run() {
        let paths = Paths::system();
        let _ = std::fs::write(paths.data_dir.join("service-fatal.log"), format!("{e}"));
    }
}

fn status(state: ServiceState, accept: ServiceControlAccept) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    }
}

fn run() -> windows_service::Result<()> {
    let (tx, rx) = mpsc::channel();
    let handler = move |event| match event {
        // 停止要求はアクセス権で SYSTEM 以外から送れない。OS のシャットダウンだけを受け付ける。
        ServiceControl::Shutdown | ServiceControl::Stop => {
            let _ = tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let handle = service_control_handler::register(SERVICE_NAME, handler)?;
    handle.set_service_status(status(ServiceState::Running, ServiceControlAccept::SHUTDOWN))?;

    let paths = Paths::system();
    let config_loaded = load_json::<Config>(&paths.config());
    let config = config_loaded.as_ref().cloned().unwrap_or_else(|_| Config::new(crate::install::local_utc_offset_minutes()));
    let state: State = load_json(&paths.state()).unwrap_or_default();
    let clock = TrustedClock::new(state.last_seen_ms, true);
    let opts = Options { policies: true, harden: true, ipc: true, lock_hosts: true };
    let mut engine = Engine::new(paths, opts, config, state, clock);
    if let Err(e) = config_loaded {
        engine.log(&format!("設定を読めなかったため既定値で動作します: {e}"));
    }
    engine.log("サービス開始");

    loop {
        engine.tick();
        match rx.recv_timeout(TICK) {
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            _ => break,
        }
    }
    engine.log("サービス停止");
    handle.set_service_status(status(ServiceState::Stopped, ServiceControlAccept::empty()))?;
    Ok(())
}
