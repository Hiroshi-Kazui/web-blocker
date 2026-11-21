//! CLI → サービスへの要求の受け渡し。
//! 要求フォルダは管理者だけが書け、設定ファイルはサービス (SYSTEM) だけが書ける。
//! そのため設定変更は必ずサービスを経由し、サービスがブロック時間中の変更を拒否できる。

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::clock::system_now_ms;
use crate::config::{Paths, load_json, save_json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Op {
    Add {
        sites: Vec<String>,
        /// 恒久ブロックとして追加する
        #[serde(default)]
        permanent: bool,
    },
    Remove { sites: Vec<String> },
    SetHours { start: String, end: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub op: Op,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub ok: bool,
    pub message: String,
}

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(40);

/// 要求を置き、サービスの応答を待つ。
pub fn send(paths: &Paths, op: Op) -> Result<Response, String> {
    let id = format!("{}-{}", system_now_ms(), std::process::id());
    let req_path = paths.requests().join(format!("{id}.json"));
    save_json(&req_path, &Request { id: id.clone(), op }).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            "権限がありません。管理者としてターミナルを開いて実行してください。".to_string()
        } else {
            format!("要求を書き込めませんでした ({}): {e}", req_path.display())
        }
    })?;

    let resp_path = paths.responses().join(format!("{id}.json"));
    let started = Instant::now();
    while started.elapsed() < RESPONSE_TIMEOUT {
        if let Ok(resp) = load_json::<Response>(&resp_path) {
            let _ = fs::remove_file(&resp_path);
            return Ok(resp);
        }
        thread::sleep(Duration::from_millis(200));
    }
    let _ = fs::remove_file(&req_path);
    Err("サービスから応答がありません。`animeblock status` でサービスが動いているか確認してください。".into())
}

/// サービス側: 溜まっている要求を古い順に取り出す (取り出したファイルは消す)。
pub fn take_requests(paths: &Paths) -> Vec<Request> {
    let Ok(entries) = fs::read_dir(paths.requests()) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        if let Ok(req) = load_json::<Request>(&f) {
            out.push(req);
        }
        let _ = fs::remove_file(&f);
    }
    out
}

pub fn respond(paths: &Paths, resp: &Response) {
    let _ = save_json(&paths.responses().join(format!("{}.json", resp.id)), resp);
}
