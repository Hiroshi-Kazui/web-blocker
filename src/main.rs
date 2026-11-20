//! animeblock: 指定時間帯に動画配信サイトをブロックする Windows 常駐サービスと、その操作用コマンド。

mod clock;
mod config;
mod engine;
mod harden;
mod hosts;
mod install;
mod ipc;
mod policy;
mod schedule;
mod service;
mod sites;

use std::path::PathBuf;
use std::process::ExitCode;

use config::{Config, Paths, State, load_json};
use ipc::Op;
use schedule::{format_hhmm, minute_of_day, parse_hhmm};

const HELP: &str = "\
animeblock - 動画配信サイト夜間ブロッカー

使い方 (status / list 以外は管理者として実行):
  animeblock install                 サービスを登録して開始する
  animeblock status                  現在の状態を表示する
  animeblock list                    ブロック対象サイトを表示する
  animeblock add <サイト>...         ブロック対象を追加する (いつでも可)
  animeblock remove <サイト>...      ブロック対象から外す (ブロック時間外のみ)
  animeblock set-hours <開始> <終了>  ブロック時間を変更する 例: set-hours 21:00 07:00 (時間外のみ)
  animeblock uninstall               アンインストールする (ブロック時間外のみ)
  animeblock simulate --hosts <ファイル> --at <HH:MM>
                                     指定ファイルを hosts に見立てて 1 回だけ処理する (動作確認用)

サイトは example.com / https://www.example.com/ / example.com/path の形で指定できます。
ドメイン指定はサブドメインも含めてブラウザでブロックされます。";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = Paths::system();
    let result = match args.first().map(String::as_str) {
        Some("service") => service::run_dispatcher().map_err(|e| format!("サービスとして起動できません: {e}")),
        Some("install") => cmd_install(&paths),
        Some("uninstall") => install::uninstall(&paths).map(|_| println!("アンインストールしました。")),
        Some("status") => cmd_status(&paths),
        Some("list") => cmd_list(&paths),
        Some("add") => cmd_request(&paths, &args[1..], |sites| Op::Add { sites }),
        Some("remove") => cmd_request(&paths, &args[1..], |sites| Op::Remove { sites }),
        Some("set-hours") => cmd_set_hours(&paths, &args[1..]),
        Some("simulate") => cmd_simulate(&args[1..]),
        _ => {
            println!("{HELP}");
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("エラー: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_install(paths: &Paths) -> Result<(), String> {
    install::install(paths)?;
    let config: Config = load_json(&paths.config()).map_err(|e| e.to_string())?;
    println!("インストールしました。毎日 {}〜{} はブロックされます。", config.block_start, config.block_end);
    println!("以後は次のコマンドで操作できます:");
    println!("  \"{}\" status", paths.installed_exe().display());
    Ok(())
}

fn cmd_status(paths: &Paths) -> Result<(), String> {
    let Some(state) = install::service_state() else {
        println!("インストールされていません。");
        return Ok(());
    };
    println!("サービス: {state:?}");
    let config: Config = load_json(&paths.config()).map_err(|e| format!("設定を読めません: {e}"))?;
    println!(
        "ブロック時間: {}〜{} (UTC{:+}:{:02} 固定)",
        config.block_start,
        config.block_end,
        config.utc_offset_minutes / 60,
        (config.utc_offset_minutes % 60).abs()
    );
    println!("対象サイト: {} 件", config.effective_sites().len());
    match load_json::<State>(&paths.state()) {
        Ok(s) => {
            println!("現在時刻 (サービス判定): {}", format_hhmm(minute_of_day(s.trusted_now_ms, config.utc_offset_minutes)));
            println!("時刻サーバーと同期: {}", if s.ntp_synced { "済み" } else { "未同期 (OS の時計を使用)" });
            println!("状態: {}", if s.blocked { "ブロック中" } else { "ブロック時間外" });
        }
        Err(_) => println!("状態: 取得できません (サービス起動直後の可能性があります)"),
    }
    Ok(())
}

fn cmd_list(paths: &Paths) -> Result<(), String> {
    let config: Config = load_json(&paths.config()).unwrap_or_else(|_| Config::new(install::local_utc_offset_minutes()));
    println!("[既定サイト]");
    for s in sites::DEFAULT_SITES {
        let mark = if config.disabled_defaults.iter().any(|d| d == s) { "  (外しています)" } else { "" };
        println!("  {s}{mark}");
    }
    println!("[追加したサイト]");
    if config.custom_sites.is_empty() {
        println!("  (なし)");
    }
    for s in &config.custom_sites {
        println!("  {s}");
    }
    Ok(())
}

fn print_response(resp: ipc::Response) -> Result<(), String> {
    if resp.ok {
        println!("{}", resp.message);
        Ok(())
    } else {
        Err(resp.message)
    }
}

fn cmd_request(paths: &Paths, sites: &[String], make: impl FnOnce(Vec<String>) -> Op) -> Result<(), String> {
    if sites.is_empty() {
        return Err("サイトを 1 つ以上指定してください。".into());
    }
    if !install::is_installed() {
        return Err("インストールされていません。先に `animeblock install` を実行してください。".into());
    }
    print_response(ipc::send(paths, make(sites.to_vec()))?)
}

fn cmd_set_hours(paths: &Paths, args: &[String]) -> Result<(), String> {
    let [start, end] = args else {
        return Err("使い方: animeblock set-hours 21:00 07:00".into());
    };
    parse_hhmm(start)?;
    parse_hhmm(end)?;
    print_response(ipc::send(paths, Op::SetHours { start: start.clone(), end: end.clone() })?)
}

fn cmd_simulate(args: &[String]) -> Result<(), String> {
    let mut hosts: Option<PathBuf> = None;
    let mut at: Option<u16> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--hosts" => hosts = it.next().map(PathBuf::from),
            "--at" => at = Some(parse_hhmm(it.next().ok_or("--at の値がありません")?)?),
            other => return Err(format!("不明な引数: {other}")),
        }
    }
    let hosts = hosts.ok_or("--hosts <ファイル> を指定してください (本物の hosts は指定しないでください)")?;
    let at = at.ok_or("--at <HH:MM> を指定してください")?;

    let mut paths = Paths::system();
    paths.hosts = hosts;
    let config = Config::new(install::local_utc_offset_minutes());
    // 今日の at 時刻に相当する UTC ミリ秒
    let now = clock::system_now_ms();
    let cur = minute_of_day(now, config.utc_offset_minutes) as i64;
    let target_ms = now + (at as i64 - cur) * 60_000;

    let opts = engine::Options { policies: false, harden: false, ipc: false, lock_hosts: false };
    let mut e = engine::Engine::new(paths, opts, config, State::default(), clock::TrustedClock::fixed(target_ms));
    e.tick();
    println!("{} の判定: {}", format_hhmm(at), if e.blocked { "ブロック中" } else { "ブロック時間外" });
    Ok(())
}
