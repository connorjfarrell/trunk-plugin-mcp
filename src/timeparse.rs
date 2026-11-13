//! Times as a person says them: "30m", "2h ago", "yesterday 22:30", "2026-11-05 10:30", "10:30", or Unix seconds.

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use regex::Regex;
use std::sync::LazyLock;

static REL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^-?\s*(\d+(?:\.\d+)?)\s*(s|sec|secs|seconds?|m|min|mins|minutes?|h|hr|hrs|hours?|d|days?)(?:\s+ago)?$").unwrap());
static CLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,2}):(\d{2})(?::(\d{2}))?\s*(am|pm)?$").unwrap());

/// A moment, as Unix milliseconds. The error is fit to show.
pub fn parse_time(value: &str, now: DateTime<Local>) -> Result<i64, String> {
    let s = value.trim();
    let low = s.to_lowercase();
    if low.is_empty() {
        return Err("empty time".into());
    }
    if low == "now" {
        return Ok(now.timestamp_millis());
    }
    if let Some(m) = REL.captures(&low) {
        let n: f64 = m[1].parse().map_err(|_| format!("can't read the time {s:?}"))?;
        let unit = match m[2].chars().next().unwrap() {
            's' => 1.0,
            'm' => 60.0,
            'h' => 3600.0,
            _ => 86400.0,
        };
        return Ok(now.timestamp_millis() - (n * unit * 1000.0) as i64);
    }
    if low.len() >= 9 && low.chars().all(|c| c.is_ascii_digit() || c == '.') {
        let x: f64 = low.parse().map_err(|_| format!("can't read the time {s:?}"))?;
        return Ok(if x > 1e11 { x as i64 } else { (x * 1000.0) as i64 });
    }
    for (word, back) in [("today", 0), ("yesterday", 1)] {
        if let Some(rest) = low.strip_prefix(word) {
            let day = now.date_naive() - Duration::days(back);
            let t = if rest.trim().is_empty() { NaiveTime::MIN } else { clock(rest.trim())? };
            return local(day.and_time(t), s);
        }
    }
    if let Ok(t) = clock(&low) {
        return local(now.date_naive().and_time(t), s);
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Ok(d.timestamp_millis());
    }
    for f in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(s, f) {
            return local(d, s);
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return local(d.and_time(NaiveTime::MIN), s);
    }
    Err(format!("can't read the time {s:?}: use 30m, 2h, 10:30, yesterday 22:30, or 2026-11-05 10:30"))
}

fn clock(s: &str) -> Result<NaiveTime, String> {
    let m = CLOCK.captures(s).ok_or_else(|| format!("can't read the time of day {s:?}"))?;
    let (mut h, mi, sec): (u32, u32, u32) = (m[1].parse().unwrap(), m[2].parse().unwrap(), m.get(3).map_or(0, |x| x.as_str().parse().unwrap()));
    match m.get(4).map(|x| x.as_str()) {
        Some("pm") if h < 12 => h += 12,
        Some("am") if h == 12 => h = 0,
        _ => {}
    }
    NaiveTime::from_hms_opt(h, mi, sec).ok_or_else(|| format!("can't read the time of day {s:?}"))
}

fn local(d: NaiveDateTime, said: &str) -> Result<i64, String> {
    Local.from_local_datetime(&d).earliest().map(|t| t.timestamp_millis()).ok_or_else(|| format!("{said:?} isn't a time that happened here"))
}

pub fn fmt_time(ms: i64, seconds: bool) -> String {
    let d = Local.timestamp_millis_opt(ms).single().unwrap_or_else(Local::now);
    d.format(if seconds { "%Y-%m-%d %H:%M:%S" } else { "%Y-%m-%d %H:%M" }).to_string()
}

pub fn fmt_clock(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().unwrap_or_else(Local::now).format("%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 5, 12, 0, 0).unwrap()
    }

    #[test]
    fn says_when() {
        let n = now().timestamp_millis();
        assert_eq!(parse_time("30m", now()).unwrap(), n - 1_800_000);
        assert_eq!(parse_time("2 hours ago", now()).unwrap(), n - 7_200_000);
        assert_eq!(parse_time("now", now()).unwrap(), n);
        assert_eq!(parse_time("yesterday 22:30", now()).unwrap(), Local.with_ymd_and_hms(2026, 11, 4, 22, 30, 0).unwrap().timestamp_millis());
        assert_eq!(parse_time("10:30", now()).unwrap(), Local.with_ymd_and_hms(2026, 11, 5, 10, 30, 0).unwrap().timestamp_millis());
        assert_eq!(parse_time("10:30 pm", now()).unwrap(), Local.with_ymd_and_hms(2026, 11, 5, 22, 30, 0).unwrap().timestamp_millis());
        assert_eq!(parse_time("2026-11-05 10:30", now()).unwrap(), Local.with_ymd_and_hms(2026, 11, 5, 10, 30, 0).unwrap().timestamp_millis());
        assert_eq!(parse_time("1793897584", now()).unwrap(), 1_793_897_584_000);
        assert_eq!(parse_time("1793897584123", now()).unwrap(), 1_793_897_584_123);
        assert!(parse_time("when the sirens started", now()).is_err());
    }
}
