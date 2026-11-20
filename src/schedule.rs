//! ブロック時間帯の判定。時刻はすべて「0:00 からの経過分」(0..1440) で扱う。

pub const MINUTES_PER_DAY: i64 = 1440;

/// "HH:MM" を経過分に変換する。
pub fn parse_hhmm(s: &str) -> Result<u16, String> {
    let (h, m) = s
        .trim()
        .split_once(':')
        .ok_or_else(|| format!("時刻は HH:MM 形式で指定してください: {s}"))?;
    let h: u16 = h.parse().map_err(|_| format!("時が不正です: {s}"))?;
    let m: u16 = m.parse().map_err(|_| format!("分が不正です: {s}"))?;
    if h > 23 || m > 59 {
        return Err(format!("時刻が範囲外です: {s}"));
    }
    Ok(h * 60 + m)
}

pub fn format_hhmm(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// 開始 <= 現在 < 終了 ならブロック中。開始 > 終了 のときは日付をまたぐ (例: 21:00-07:00)。
/// 開始 == 終了 は「ブロックしない」と扱う。
pub fn is_blocked(now: u16, start: u16, end: u16) -> bool {
    if start == end {
        return false;
    }
    if start < end {
        start <= now && now < end
    } else {
        now >= start || now < end
    }
}

/// UTC のミリ秒と UTC からのオフセット(分)から、その地域の経過分を求める。
pub fn minute_of_day(utc_ms: i64, utc_offset_minutes: i32) -> u16 {
    let local_minutes = utc_ms.div_euclid(60_000) + utc_offset_minutes as i64;
    local_minutes.rem_euclid(MINUTES_PER_DAY) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overnight_window() {
        let (s, e) = (parse_hhmm("21:00").unwrap(), parse_hhmm("07:00").unwrap());
        assert!(!is_blocked(parse_hhmm("20:59").unwrap(), s, e));
        assert!(is_blocked(parse_hhmm("21:00").unwrap(), s, e));
        assert!(is_blocked(parse_hhmm("23:59").unwrap(), s, e));
        assert!(is_blocked(parse_hhmm("00:00").unwrap(), s, e));
        assert!(is_blocked(parse_hhmm("06:59").unwrap(), s, e));
        assert!(!is_blocked(parse_hhmm("07:00").unwrap(), s, e));
        assert!(!is_blocked(parse_hhmm("12:00").unwrap(), s, e));
    }

    #[test]
    fn same_day_window() {
        let (s, e) = (parse_hhmm("09:00").unwrap(), parse_hhmm("17:00").unwrap());
        assert!(!is_blocked(parse_hhmm("08:59").unwrap(), s, e));
        assert!(is_blocked(parse_hhmm("09:00").unwrap(), s, e));
        assert!(!is_blocked(parse_hhmm("17:00").unwrap(), s, e));
    }

    #[test]
    fn empty_window_never_blocks() {
        assert!(!is_blocked(600, 600, 600));
    }

    #[test]
    fn parse_rejects_bad_input() {
        assert!(parse_hhmm("24:00").is_err());
        assert!(parse_hhmm("7").is_err());
        assert!(parse_hhmm("07:60").is_err());
        assert_eq!(parse_hhmm(" 7:05 ").unwrap(), 425);
        assert_eq!(format_hhmm(425), "07:05");
    }

    #[test]
    fn minute_of_day_uses_fixed_offset() {
        // 2026-01-01T12:00:00Z は JST(+540) で 21:00
        let ms = 1_767_268_800_000i64;
        assert_eq!(minute_of_day(ms, 540), 21 * 60);
        assert_eq!(minute_of_day(ms, 0), 12 * 60);
        // 負方向へまたぐ場合
        assert_eq!(minute_of_day(0, -60), 23 * 60);
    }
}
