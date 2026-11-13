//! The index: an SQLite file of the recordings folder's calls and their transcripts, kept up to date as the plugin runs.
//!
//! A call is `<system>/<year>/<month>/<day>/<name>.json` (its record) and, beside it, `<name>.transcript.json` once a
//! transcription plugin has heard it. Only the last `days` days are indexed, newest day first, so the newest traffic can be
//! asked about while older days are still being read. A day folder that hasn't changed since it was read isn't read again; the
//! last two days are looked at on every pass, since a transcript arrives some seconds (or, in a backfill, days) after its call.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{Local, NaiveDate};
use regex::Regex;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS calls (
  id INTEGER PRIMARY KEY,
  key TEXT UNIQUE NOT NULL,
  dir TEXT NOT NULL,
  system TEXT, site TEXT,
  tg INTEGER, tg_tag TEXT, tg_desc TEXT, tg_group TEXT, tg_group_tag TEXT,
  start_ms INTEGER NOT NULL, end_ms INTEGER,
  first_unit INTEGER, last_unit INTEGER, units TEXT,
  emergency INTEGER, encrypted INTEGER, decrypted INTEGER,
  has_audio INTEGER, text TEXT, has_text INTEGER NOT NULL DEFAULT 0, server TEXT
);
CREATE INDEX IF NOT EXISTS calls_time ON calls(start_ms);
CREATE INDEX IF NOT EXISTS calls_tg ON calls(tg, start_ms);
CREATE INDEX IF NOT EXISTS calls_dir ON calls(dir);
CREATE TABLE IF NOT EXISTS call_units (call_id INTEGER NOT NULL, unit INTEGER NOT NULL, PRIMARY KEY (call_id, unit)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS call_units_unit ON call_units(unit);
CREATE VIRTUAL TABLE IF NOT EXISTS calls_fts USING fts5(text, tag, grp, tokenize='porter unicode61');
CREATE TABLE IF NOT EXISTS dirs (dir TEXT PRIMARY KEY, mtime_ns INTEGER NOT NULL, n INTEGER NOT NULL);
";

/// How far the index has got.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub building: bool,
    /// One whole pass has been made.
    pub ready: bool,
    pub dirs_total: usize,
    pub dirs_done: usize,
    pub last_error: String,
}

pub struct Index {
    pub root: PathBuf,
    pub db: PathBuf,
    pub status: Mutex<Status>,
    last_prune: Mutex<Option<Instant>>,
}

impl Index {
    pub fn open(root: &Path, db: &Path) -> Result<Index, String> {
        if let Some(p) = db.parent() {
            fs::create_dir_all(p).map_err(|e| format!("can't make {}: {e}", p.display()))?;
        }
        let ix = Index { root: root.to_path_buf(), db: db.to_path_buf(), status: Mutex::new(Status::default()), last_prune: Mutex::new(None) };
        let c = ix.writer()?;
        c.execute_batch(SCHEMA).map_err(|e| format!("can't set up the index {}: {e}", db.display()))?;
        Ok(ix)
    }

    fn writer(&self) -> Result<Connection, String> {
        let c = Connection::open(&self.db).map_err(|e| format!("can't open the index {}: {e}", self.db.display()))?;
        c.busy_timeout(Duration::from_secs(30)).ok();
        c.pragma_update(None, "journal_mode", "WAL").ok();
        c.pragma_update(None, "synchronous", "NORMAL").ok();
        Ok(c)
    }

    /// A connection for reading, one to each question.
    pub fn reader(&self) -> Result<Connection, String> {
        let c = Connection::open_with_flags(&self.db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .map_err(|e| format!("can't read the index: {e}"))?;
        c.busy_timeout(Duration::from_secs(30)).ok();
        Ok(c)
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// Every day folder in the window, newest day first: (key, path).
    pub fn day_dirs(&self, days: u32) -> Vec<(String, PathBuf)> {
        let floor = Local::now().date_naive() - chrono::Duration::days(days as i64);
        let mut out: Vec<(NaiveDate, String)> = vec![];
        for system in num_or_name_dirs(&self.root, false) {
            for y in num_or_name_dirs(&self.root.join(&system), true) {
                for m in num_or_name_dirs(&self.root.join(&system).join(&y), true) {
                    for d in num_or_name_dirs(&self.root.join(&system).join(&y).join(&m), true) {
                        if let (Ok(yy), Ok(mm), Ok(dd)) = (y.parse(), m.parse(), d.parse())
                            && let Some(day) = NaiveDate::from_ymd_opt(yy, mm, dd)
                            && day >= floor
                        {
                            out.push((day, format!("{system}/{y}/{m}/{d}")));
                        }
                    }
                }
            }
        }
        out.sort_by(|a, b| b.cmp(a));
        out.into_iter().map(|(_, k)| (k.clone(), self.root.join(&k))).collect()
    }

    /// One pass: bring the index up to date. `skip`: systems left out.
    pub fn pass(&self, days: u32, skip: &dyn Fn(&str) -> bool, stop: &dyn Fn() -> bool) {
        {
            let mut s = self.status.lock().unwrap();
            s.building = true;
            s.last_error.clear();
        }
        let result = self.pass_inner(days, skip, stop);
        let mut s = self.status.lock().unwrap();
        s.building = false;
        match result {
            Ok(()) => s.ready = true,
            Err(e) if e.is_empty() => {} // stopped
            Err(e) => s.last_error = e,
        }
    }

    fn pass_inner(&self, days: u32, skip: &dyn Fn(&str) -> bool, stop: &dyn Fn() -> bool) -> Result<(), String> {
        let dirs = self.day_dirs(days);
        {
            let mut s = self.status.lock().unwrap();
            s.dirs_total = dirs.len();
            s.dirs_done = 0;
        }
        let mut conn = self.writer()?;
        let known: HashMap<String, i64> = {
            let mut st = conn.prepare("SELECT dir, mtime_ns FROM dirs").map_err(|e| e.to_string())?;
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).map_err(|e| e.to_string())?.flatten().collect()
        };
        let recent = Local::now().date_naive() - chrono::Duration::days(1);
        for (key, path) in &dirs {
            if stop() {
                return Err(String::new());
            }
            let system = key.split('/').next().unwrap_or("");
            if !skip(system)
                && let Ok(meta) = fs::metadata(path)
            {
                let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as i64);
                let day = dir_day(key);
                if known.get(key) != Some(&mtime) || day.is_some_and(|d| d >= recent) {
                    read_dir(&mut conn, key, path, mtime);
                }
            }
            self.status.lock().unwrap().dirs_done += 1;
        }
        self.prune(&conn, days);
        Ok(())
    }

    /// Forget calls older than the window (the recorder's retention may have deleted them), once an hour.
    fn prune(&self, conn: &Connection, days: u32) {
        let mut last = self.last_prune.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(3600)) {
            return;
        }
        *last = Some(Instant::now());
        let cutoff = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64) - (days as i64 + 1) * 86_400_000;
        for sql in [
            "DELETE FROM calls_fts WHERE rowid IN (SELECT id FROM calls WHERE start_ms < ?1)",
            "DELETE FROM call_units WHERE call_id IN (SELECT id FROM calls WHERE start_ms < ?1)",
            "DELETE FROM calls WHERE start_ms < ?1",
        ] {
            conn.execute(sql, params![cutoff]).ok();
        }
    }
}

/// Folder names under `path`: any (the systems), or only numbers (years, months, days).
fn num_or_name_dirs(path: &Path, numeric: bool) -> Vec<String> {
    let Ok(rd) = fs::read_dir(path) else { return vec![] };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && (!numeric || n.bytes().all(|b| b.is_ascii_digit()) && !n.is_empty()))
        .collect()
}

fn dir_day(key: &str) -> Option<NaiveDate> {
    let mut it = key.rsplit('/');
    let (d, m, y) = (it.next()?, it.next()?, it.next()?);
    NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
}

fn call_name() -> &'static Regex {
    static R: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| Regex::new(r"^\d+-\d+_[\d.]+$").unwrap());
    &R
}

#[derive(Default, Clone, Copy)]
struct Has {
    rec: bool,
    tx: bool,
    audio: bool,
}

fn read_dir(conn: &mut Connection, key: &str, path: &Path, mtime: i64) {
    let Ok(rd) = fs::read_dir(path) else { return };
    let mut stems: HashMap<String, Has> = HashMap::new();
    for e in rd.flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        if let Some(s) = name.strip_suffix(".transcript.json") {
            stems.entry(s.to_string()).or_default().tx = true;
        } else if let Some(s) = name.strip_suffix(".json") {
            stems.entry(s.to_string()).or_default().rec = true;
        } else if let Some((s, ext)) = name.rsplit_once('.')
            && matches!(ext, "wav" | "m4a" | "mp3")
        {
            stems.entry(s.to_string()).or_default().audio = true;
        }
    }
    let have: HashMap<String, bool> = match conn.prepare("SELECT key, has_text FROM calls WHERE dir=?1") {
        Ok(mut st) => st.query_map([key], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))).map(|rows| rows.flatten().collect()).unwrap_or_default(),
        Err(_) => return,
    };
    let todo: Vec<(&String, &Has)> =
        stems.iter().filter(|(s, h)| h.rec && call_name().is_match(s) && have.get(&format!("{key}/{s}")).is_none_or(|had| h.tx && !had)).collect();
    let Ok(tx) = conn.transaction() else { return };
    for (stem, has) in todo {
        index_call(&tx, key, path, stem, *has);
    }
    tx.execute("INSERT OR REPLACE INTO dirs(dir, mtime_ns, n) VALUES (?1,?2,?3)", params![key, mtime, stems.len() as i64]).ok();
    tx.commit().ok();
}

fn index_call(tx: &rusqlite::Transaction, rel: &str, path: &Path, stem: &str, has: Has) {
    let Ok(raw) = fs::read(path.join(format!("{stem}.json"))) else { return };
    let Ok(r) = serde_json::from_slice::<Value>(&raw) else { return };
    let Some(tg) = r["talkgroup"].as_i64() else { return };
    let start = r["start_time_ms"].as_i64().filter(|v| *v > 0).or_else(|| r["start_time"].as_i64().map(|s| s * 1000)).unwrap_or(0);
    if start == 0 {
        return;
    }
    let end = r["stop_time_ms"]
        .as_i64()
        .filter(|v| *v > 0)
        .unwrap_or(start + r["call_length_ms"].as_i64().unwrap_or_else(|| r["call_length"].as_i64().unwrap_or(0) * 1000));
    let (mut text, mut server, mut has_text) = (String::new(), None::<String>, 0i64);
    if has.tx
        && let Ok(raw) = fs::read(path.join(format!("{stem}.transcript.json")))
        && let Ok(t) = serde_json::from_slice::<Value>(&raw)
    {
        text = t["text"].as_str().unwrap_or("").trim().to_string();
        server = t["server"].as_str().map(String::from);
        has_text = 1;
    }
    let srcs: Vec<i64> = r["srcList"].as_array().map(|a| a.iter().filter_map(|s| s["src"].as_i64()).filter(|u| *u > 0).collect()).unwrap_or_default();
    let mut units = srcs.clone();
    units.sort_unstable();
    units.dedup();
    let s = |k: &str| r[k].as_str().map(String::from);
    let flag = |k: &str| i64::from(r[k].as_bool().unwrap_or(false) || r[k].as_i64().is_some_and(|v| v != 0));
    let id: rusqlite::Result<i64> = tx.query_row(
        "INSERT INTO calls(key, dir, system, site, tg, tg_tag, tg_desc, tg_group, tg_group_tag, start_ms, end_ms, first_unit, last_unit, units, emergency, encrypted, decrypted, has_audio, text, has_text, server)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)
         ON CONFLICT(key) DO UPDATE SET text=excluded.text, has_text=excluded.has_text, server=excluded.server, has_audio=excluded.has_audio, end_ms=excluded.end_ms
         RETURNING id",
        params![
            format!("{rel}/{stem}"),
            rel,
            s("short_name").unwrap_or_else(|| rel.split('/').next().unwrap_or("").to_string()),
            s("site"),
            tg,
            s("talkgroup_tag"),
            s("talkgroup_description"),
            s("talkgroup_group"),
            s("talkgroup_group_tag"),
            start,
            end,
            srcs.first(),
            srcs.last(),
            units.iter().map(i64::to_string).collect::<Vec<_>>().join(" "),
            flag("emergency"),
            flag("encrypted"),
            flag("decrypted"),
            i64::from(has.audio),
            text,
            has_text,
            server,
        ],
        |row| row.get(0),
    );
    let Ok(id) = id else { return };
    tx.execute("DELETE FROM calls_fts WHERE rowid=?1", [id]).ok();
    tx.execute(
        "INSERT INTO calls_fts(rowid, text, tag, grp) VALUES (?1,?2,?3,?4)",
        params![id, text, s("talkgroup_tag").unwrap_or_default(), s("talkgroup_group").unwrap_or_default()],
    )
    .ok();
    for u in units {
        tx.execute("INSERT OR IGNORE INTO call_units(call_id, unit) VALUES (?1,?2)", params![id, u]).ok();
    }
}
