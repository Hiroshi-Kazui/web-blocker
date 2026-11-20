//! 改ざんされにくい現在時刻。
//!
//! - ネットの時刻サーバー (NTP) と同期できたら、以後は OS の時計ではなく
//!   「同期時刻 + 経過時間 (単調増加タイマー)」を使う。OS の時計を進めても効かない。
//! - 同期できないときは OS の時計を使うが、過去に観測した最新時刻より前には戻さない。

use std::net::UdpSocket;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const NTP_SERVERS: &[&str] = &["time.windows.com", "ntp.nict.jp", "time.google.com", "pool.ntp.org"];
const NTP_UNIX_OFFSET_SECS: i64 = 2_208_988_800;
/// 2024-01-01。これより前を返すサーバー応答は捨てる。
const MIN_VALID_MS: i64 = 1_704_067_200_000;

pub fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// NTP 応答パケットから送信時刻 (Unix ミリ秒) を取り出す。
pub fn parse_ntp_response(buf: &[u8]) -> Option<i64> {
    if buf.len() < 48 {
        return None;
    }
    let secs = u32::from_be_bytes(buf[40..44].try_into().ok()?) as i64;
    let frac = u32::from_be_bytes(buf[44..48].try_into().ok()?) as i64;
    let ms = (secs - NTP_UNIX_OFFSET_SECS) * 1000 + ((frac * 1000) >> 32);
    (ms >= MIN_VALID_MS).then_some(ms)
}

fn query_ntp(server: &str) -> Option<i64> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    socket.connect((server, 123)).ok()?;
    let mut req = [0u8; 48];
    req[0] = 0x1B; // LI=0, VN=3, Mode=3 (client)
    let sent = Instant::now();
    socket.send(&req).ok()?;
    let mut buf = [0u8; 48];
    let n = socket.recv(&mut buf).ok()?;
    let rtt_half = sent.elapsed().as_millis() as i64 / 2;
    parse_ntp_response(&buf[..n]).map(|ms| ms + rtt_half)
}

pub struct TrustedClock {
    /// (基準時刻, その時点の単調タイマー)
    anchor: Option<(i64, Instant)>,
    synced: bool,
    last_attempt: Option<Instant>,
    floor_ms: i64,
    ntp_enabled: bool,
}

impl TrustedClock {
    pub fn new(floor_ms: i64, ntp_enabled: bool) -> Self {
        TrustedClock { anchor: None, synced: false, last_attempt: None, floor_ms, ntp_enabled }
    }

    /// シミュレーション用: 指定時刻に固定する
    pub fn fixed(now_ms: i64) -> Self {
        TrustedClock { anchor: Some((now_ms, Instant::now())), synced: false, last_attempt: None, floor_ms: 0, ntp_enabled: false }
    }

    pub fn is_synced(&self) -> bool {
        self.synced
    }

    /// 必要なら NTP と同期する。未同期の間は 1 分ごと、同期後は 15 分ごとに再同期。
    pub fn tick(&mut self) {
        if !self.ntp_enabled {
            return;
        }
        let interval = if self.synced { Duration::from_secs(15 * 60) } else { Duration::from_secs(60) };
        if self.last_attempt.is_some_and(|t| t.elapsed() < interval) {
            return;
        }
        self.last_attempt = Some(Instant::now());
        if let Some(ms) = NTP_SERVERS.iter().find_map(|s| query_ntp(s)) {
            self.anchor = Some((ms, Instant::now()));
            self.synced = true;
        } else if self.anchor.is_none() {
            // 一度も同期できていない: OS 時計 (ただし最新観測時刻以上) を基準にし、以後は単調タイマーで進める
            self.anchor = Some((system_now_ms().max(self.floor_ms), Instant::now()));
        }
    }

    pub fn now_ms(&mut self) -> i64 {
        let now = match self.anchor {
            Some((base, at)) => base + at.elapsed().as_millis() as i64,
            None => system_now_ms(),
        };
        let now = now.max(self.floor_ms);
        self.floor_ms = now;
        now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_transmit_timestamp() {
        let mut buf = [0u8; 48];
        // 2026-01-01T12:00:00Z
        let secs = (1_767_268_800i64 + NTP_UNIX_OFFSET_SECS) as u32;
        buf[40..44].copy_from_slice(&secs.to_be_bytes());
        buf[44..48].copy_from_slice(&(1u32 << 31).to_be_bytes()); // 0.5 秒
        assert_eq!(parse_ntp_response(&buf), Some(1_767_268_800_500));
    }

    #[test]
    fn rejects_short_or_bogus_packets() {
        assert_eq!(parse_ntp_response(&[0u8; 10]), None);
        assert_eq!(parse_ntp_response(&[0u8; 48]), None);
    }

    #[test]
    fn never_goes_before_floor() {
        let future = system_now_ms() + 86_400_000;
        let mut c = TrustedClock::new(future, false);
        assert!(c.now_ms() >= future);
    }
}
