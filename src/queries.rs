//! Questions to ask the index. Each returns calls; `tools` words them.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use regex::Regex;
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};

/// Which encrypted calls may be shown: the plugin's setting, and any system that says otherwise.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    pub allow_encrypted: bool,
    /// By system short name.
    pub overrides: HashMap<String, bool>,
}

impl Policy {
    fn clause(&self, p: &str) -> (String, Vec<Value>) {
        let listed: Vec<String> = self.overrides.iter().filter(|(_, allow)| **allow != self.allow_encrypted).map(|(s, _)| s.clone()).collect();
        let marks = vec!["?"; listed.len()].join(",");
        let args: Vec<Value> = listed.into_iter().map(Value::Text).collect();
        match (self.allow_encrypted, args.is_empty()) {
            (true, true) => ("1=1".into(), vec![]),
            (true, false) => (format!("({p}encrypted = 0 OR {p}system NOT IN ({marks}))"), args),
            (false, true) => (format!("{p}encrypted = 0"), vec![]),
            (false, false) => (format!("({p}encrypted = 0 OR {p}system IN ({marks}))"), args),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    /// A word of the system's or site's name.
    pub system: Option<String>,
    /// A number, or a word of its name, description or agency.
    pub talkgroup: Option<String>,
    /// A word of the agency.
    pub group: Option<String>,
    pub unit: Option<i64>,
    pub emergency_only: bool,
    pub transcribed_only: bool,
    pub policy: Policy,
}

impl Filter {
    fn sql(&self) -> (String, Vec<Value>) {
        let mut w: Vec<String> = vec!["1=1".into()];
        let mut a: Vec<Value> = vec![];
        let like = |s: &str| Value::Text(format!("%{s}%"));
        if let Some(v) = self.since_ms {
            w.push("c.start_ms >= ?".into());
            a.push(Value::Integer(v));
        }
        if let Some(v) = self.until_ms {
            w.push("c.start_ms <= ?".into());
            a.push(Value::Integer(v));
        }
        if let Some(s) = &self.system {
            w.push("(c.system LIKE ? OR c.site LIKE ?)".into());
            a.extend([like(s), like(s)]);
        }
        if let Some(t) = &self.talkgroup {
            if let Ok(n) = t.trim().parse::<i64>() {
                w.push("c.tg = ?".into());
                a.push(Value::Integer(n));
            } else {
                w.push("(c.tg_tag LIKE ? OR c.tg_desc LIKE ? OR c.tg_group LIKE ?)".into());
                a.extend([like(t), like(t), like(t)]);
            }
        }
        if let Some(g) = &self.group {
            w.push("(c.tg_group LIKE ? OR c.tg_group_tag LIKE ?)".into());
            a.extend([like(g), like(g)]);
        }
        if let Some(u) = self.unit {
            w.push("EXISTS (SELECT 1 FROM call_units u WHERE u.call_id=c.id AND u.unit=?)".into());
            a.push(Value::Integer(u));
        }
        if self.emergency_only {
            w.push("c.emergency = 1".into());
        }
        if self.transcribed_only {
            w.push("c.has_text = 1 AND c.text <> ''".into());
        }
        let (clause, args) = self.policy.clause("c.");
        w.push(clause);
        a.extend(args);
        (w.join(" AND "), a)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Call {
    pub id: i64,
    pub key: String,
    pub system: String,
    pub site: String,
    pub tg: i64,
    pub tag: String,
    pub group: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub last_unit: Option<i64>,
    pub units: String,
    pub emergency: bool,
    pub encrypted: bool,
    pub has_audio: bool,
    pub text: String,
    pub has_text: bool,
    pub server: String,
    /// Why it is in an incident's list.
    pub link: String,
}

const COLS: &str = "c.id, c.key, c.system, c.site, c.tg, c.tg_tag, c.tg_group, c.start_ms, c.end_ms, c.last_unit, c.units, c.emergency, c.encrypted, c.has_audio, c.text, c.has_text, c.server";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Call> {
    let s = |i: usize| r.get::<_, Option<String>>(i).map(Option::unwrap_or_default);
    Ok(Call {
        id: r.get(0)?,
        key: r.get(1)?,
        system: s(2)?,
        site: s(3)?,
        tg: r.get(4)?,
        tag: s(5)?,
        group: s(6)?,
        start_ms: r.get(7)?,
        end_ms: r.get::<_, Option<i64>>(8)?.unwrap_or_default(),
        last_unit: r.get(9)?,
        units: s(10)?,
        emergency: r.get::<_, i64>(11)? != 0,
        encrypted: r.get::<_, i64>(12)? != 0,
        has_audio: r.get::<_, Option<i64>>(13)?.unwrap_or(0) != 0,
        text: s(14)?,
        has_text: r.get::<_, i64>(15)? != 0,
        server: s(16)?,
        link: String::new(),
    })
}

fn run(conn: &Connection, sql: &str, args: Vec<Value>) -> Result<Vec<Call>, String> {
    let mut st = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = st.query_map(params_from_iter(args), row).map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

static OPS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\b(AND|OR|NOT|NEAR)\b|["*()]"#).unwrap());
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\w'-]+").unwrap());

fn quoted_words(text: &str) -> String {
    WORD.find_iter(text).map(|m| format!("\"{}\"", m.as_str().replace('"', ""))).collect::<Vec<_>>().join(" ")
}

/// Words as an FTS5 expression: every word must be there, unless the text already has AND / OR / NOT / quotes.
pub fn fts_query(text: &str) -> String {
    let text = text.trim();
    if OPS.is_match(text) { text.to_string() } else { quoted_words(text) }
}

pub fn search(conn: &Connection, text: &str, f: &Filter, limit: usize, newest_first: bool) -> Result<Vec<Call>, String> {
    let q = fts_query(text);
    if q.is_empty() {
        return Err("nothing to search for".into());
    }
    let (w, a) = f.sql();
    let order = if newest_first { "DESC" } else { "ASC" };
    let sql = format!(
        "SELECT {COLS} FROM calls_fts JOIN calls c ON c.id = calls_fts.rowid WHERE calls_fts MATCH ? AND {w} ORDER BY c.start_ms {order} LIMIT {limit}"
    );
    let with = |q: &str| {
        let mut args = vec![Value::Text(format!("text : ({q})"))];
        args.extend(a.clone());
        args
    };
    // Bad FTS syntax typed by hand: search its words instead.
    run(conn, &sql, with(&q)).or_else(|_| run(conn, &sql, with(&quoted_words(text))))
}

pub fn traffic(conn: &Connection, f: &Filter, limit: usize, newest_first: bool) -> Result<Vec<Call>, String> {
    let (w, a) = f.sql();
    let order = if newest_first { "DESC" } else { "ASC" };
    let mut rows = run(conn, &format!("SELECT {COLS} FROM calls c WHERE {w} ORDER BY c.start_ms {order} LIMIT {limit}"), a)?;
    if newest_first {
        rows.reverse();
    }
    Ok(rows)
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Call>, String> {
    Ok(run(conn, &format!("SELECT {COLS} FROM calls c WHERE c.id = ?"), vec![Value::Integer(id)])?.pop())
}

pub struct TalkgroupActivity {
    pub tg: i64,
    pub tag: String,
    pub group: String,
    pub n: i64,
    pub transcribed: i64,
    pub last_ms: i64,
    pub emergencies: i64,
}

pub fn activity(conn: &Connection, f: &Filter, top: usize) -> Result<(i64, Vec<TalkgroupActivity>), String> {
    let (w, a) = f.sql();
    let total: i64 =
        conn.query_row(&format!("SELECT COUNT(*) FROM calls c WHERE {w}"), params_from_iter(a.clone()), |r| r.get(0)).map_err(|e| e.to_string())?;
    let sql = format!(
        "SELECT c.tg, MAX(c.tg_tag), MAX(c.tg_group), COUNT(*), SUM(c.has_text), MAX(c.start_ms), SUM(c.emergency)
         FROM calls c WHERE {w} GROUP BY c.tg, COALESCE(c.tg_tag,'') ORDER BY COUNT(*) DESC LIMIT {top}"
    );
    let mut st = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = st
        .query_map(params_from_iter(a), |r| {
            Ok(TalkgroupActivity {
                tg: r.get(0)?,
                tag: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                group: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                n: r.get(3)?,
                transcribed: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                last_ms: r.get(5)?,
                emergencies: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
            })
        })
        .map_err(|e| e.to_string())?;
    Ok((total, rows.flatten().collect()))
}

static APPARATUS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(medic|engine|ladder|truck|squad|rescue|battalion|brush|tower|quint|ambulance|chief|unit|car|district|sergeant|supervisor)\s+(\d{1,4}[a-z]?|one|two|three|four|five|six|seven|eight|nine|ten)\b").unwrap()
});
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{3,6}\b").unwrap());

/// What a dispatch names that the units will say back: "medic 6", "engine 6", an address number.
pub fn mention_terms(texts: &[&str], limit: usize) -> Vec<String> {
    let mut seen: Vec<String> = vec![];
    let mut add = |t: String| {
        if !seen.contains(&t) {
            seen.push(t);
        }
    };
    for t in texts {
        for m in APPARATUS.captures_iter(t) {
            add(format!("{} {}", &m[1], &m[2]).to_lowercase());
        }
    }
    if let Some(t) = texts.first() {
        for m in NUMBER.find_iter(t) {
            add(m.as_str().to_string());
        }
    }
    seen.truncate(limit);
    seen
}

fn words_of(text: &str) -> String {
    let mut s = String::from(" ");
    for c in text.to_lowercase().chars() {
        s.push(if c.is_ascii_alphanumeric() { c } else { ' ' });
    }
    s.push(' ');
    s
}

/// The calls around one call: its own talkgroup, then other talkgroups linked to it two ways.
///
/// A talkgroup is told by its number and name together, so the two sites' copies of it are one. The links: the radios that
/// answered on its talkgroup in the next four minutes and then spoke elsewhere; and what the dispatch names (the units it
/// sends, the numbers in it) said back on other talkgroups. Each row says which.
pub fn incident(conn: &Connection, seed: &Call, before_ms: i64, after_ms: i64, limit: usize, policy: &Policy) -> Result<Vec<Call>, String> {
    let f = Filter { since_ms: Some(seed.start_ms - before_ms), until_ms: Some(seed.start_ms + after_ms), policy: policy.clone(), ..Default::default() };
    let (w, a) = f.sql();
    let mut same = run(
        conn,
        &format!("SELECT {COLS} FROM calls c WHERE {w} AND c.tg = ? AND COALESCE(c.tg_tag,'') = ? ORDER BY c.start_ms"),
        [a.clone(), vec![Value::Integer(seed.tg), Value::Text(seed.tag.clone())]].concat(),
    )?;
    let seed_units: Vec<i64> = seed.units.split_whitespace().filter_map(|u| u.parse().ok()).collect();
    let mut responders: Vec<i64> = vec![];
    for r in &same {
        if r.start_ms >= seed.start_ms - 15_000 && r.start_ms <= seed.start_ms + 240_000 {
            for u in r.units.split_whitespace().filter_map(|u| u.parse::<i64>().ok()) {
                if !seed_units.contains(&u) && !responders.contains(&u) {
                    responders.push(u);
                }
            }
        }
    }
    let mut other: BTreeMap<i64, Call> = BTreeMap::new();
    let not_same = "NOT (c.tg = ? AND COALESCE(c.tg_tag,'') = ?)";
    let after_seed = Value::Integer(seed.start_ms - 15_000);
    if !responders.is_empty() {
        let marks = vec!["?"; responders.len()].join(",");
        let mut args = a.clone();
        args.extend(responders.iter().map(|u| Value::Integer(*u)));
        args.extend([Value::Integer(seed.tg), Value::Text(seed.tag.clone()), after_seed.clone()]);
        let sql = format!(
            "SELECT DISTINCT {COLS} FROM calls c JOIN call_units u ON u.call_id = c.id WHERE {w} AND u.unit IN ({marks}) AND {not_same} AND c.start_ms >= ? ORDER BY c.start_ms"
        );
        for mut r in run(conn, &sql, args)? {
            let who: Vec<String> = r.units.split_whitespace().filter(|u| u.parse::<i64>().is_ok_and(|n| responders.contains(&n))).map(String::from).collect();
            r.link = format!("answering radio: {}", who.join(", "));
            other.insert(r.id, r);
        }
    }
    let near: Vec<&str> = same.iter().filter(|r| (r.start_ms - seed.start_ms).abs() <= 45_000 && !r.text.is_empty()).map(|r| r.text.as_str()).collect();
    let mut texts: Vec<&str> = vec![seed.text.as_str()];
    texts.extend(near);
    let terms = mention_terms(&texts, 8);
    if !terms.is_empty() {
        let q = terms.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" OR ");
        let mut args = vec![Value::Text(format!("text : ({q})"))];
        args.extend(a.clone());
        args.extend([Value::Integer(seed.tg), Value::Text(seed.tag.clone()), after_seed]);
        let sql = format!(
            "SELECT {COLS} FROM calls_fts JOIN calls c ON c.id = calls_fts.rowid WHERE calls_fts MATCH ? AND {w} AND {not_same} AND c.start_ms >= ? ORDER BY c.start_ms LIMIT 200"
        );
        for mut r in run(conn, &sql, args).unwrap_or_default() {
            let low = words_of(&r.text);
            let named: Vec<String> = terms.iter().filter(|t| low.contains(&format!(" {t} "))).map(|t| format!("\"{t}\"")).collect();
            let note = format!("names {}", named.join(", "));
            match other.get_mut(&r.id) {
                Some(have) => {
                    have.link.push_str("; ");
                    have.link.push_str(&note);
                }
                None => {
                    r.link = note;
                    other.insert(r.id, r);
                }
            }
        }
    }
    for r in &mut same {
        r.link = "same talkgroup".into();
    }
    same.extend(other.into_values());
    same.sort_by_key(|r| r.start_ms);
    same.truncate(limit);
    Ok(same)
}
