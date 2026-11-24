//! The tools an MCP client is offered, and their answers in words.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::Local;
use serde_json::{Value, json};

use crate::index::Index;
use crate::queries::{self, Call, Filter, Policy};
use crate::timeparse::{fmt_clock, fmt_time, parse_time};

pub const INSTRUCTIONS: &str = "Tools over a radio scanner's recordings: every call (a transmission on a talkgroup) with who spoke, when, and a machine transcript of what was said. \
Transcripts are automatic and imperfect: names, street names and numbers are often wrong, and a missing transcript means it was not (yet) transcribed, not that nothing was said. Times are local.\n\n\
To work out what an event was (\"I just heard sirens\"): call recent_activity first to see what is busy, then search_traffic for the likely words (medical, fire, structure, accident, a street name) \
over the last half hour, then incident_context on the call that dispatched it to see the units' traffic afterwards. Say what the transcripts support and what they do not; \
quote the transcript text, with the time and talkgroup, for what you claim.";

#[derive(Clone, Default)]
pub struct Settings {
    pub policy: Policy,
    /// Radios' names, by number.
    pub units: HashMap<i64, String>,
    /// Where the recordings are served (Playback's address), for listen links.
    pub audio_base: String,
    pub days: u32,
    /// The largest recording `get_audio` hands over in a reply, in bytes.
    pub audio_max: u64,
    /// The access token a client must send (empty: none).
    pub token: String,
    /// The networks a client may connect from.
    pub allowed: Vec<crate::net::Cidr>,
}

pub struct Ctx {
    pub index: Arc<Index>,
    pub settings: RwLock<Settings>,
}

impl Ctx {
    fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }
}

/// The tools, as `tools/list` answers.
pub fn list() -> Value {
    let time = |d: &str| json!({ "type": "string", "description": d });
    let since = time("From when: '30m', '2h', '10:30', 'yesterday 22:30' or '2026-11-05 10:30' (local time)");
    json!([
        {
            "name": "recent_activity",
            "description": "What is on the radio now: the busiest talkgroups of the last `minutes`, any emergency-flagged calls, and the newest transcripts. Start here for 'what is going on'. `group` limits it to an agency (a word of its name: 'Metro Fire'), `system` to one system or site.",
            "inputSchema": { "type": "object", "properties": {
                "minutes": { "type": "integer", "description": "How far back (default 15)" },
                "system": { "type": "string" }, "group": { "type": "string" },
                "sample": { "type": "integer", "description": "How many newest transcripts to list (default 12)" }
            } },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "search_traffic",
            "description": "Search what was said. `text` is words that must all appear in a transcript ('cardiac arrest'); use OR for alternatives ('cardiac OR arrest OR cpr OR unresponsive'), quotes for a phrase, a trailing * for a prefix ('seiz*'). Narrow by time, `talkgroup` (a number, or a word of its name), `group` (an agency), `system`, or a radio's `unit` number. Each line starts with the call's [id], which incident_context and get_call take.",
            "inputSchema": { "type": "object", "properties": {
                "text": { "type": "string" }, "since": since.clone(), "until": time("Until when (default now)"),
                "talkgroup": { "type": "string" }, "group": { "type": "string" }, "system": { "type": "string" }, "unit": { "type": "string" },
                "emergency_only": { "type": "boolean" }, "limit": { "type": "integer", "description": "Default 30, at most 200" },
                "newest_first": { "type": "boolean", "description": "Default true" }
            }, "required": ["text"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_traffic",
            "description": "The traffic, in time order, of a window: everything on a talkgroup or agency, or everything a radio said. Use it to read a conversation: `since`/`until` bracket the minutes, `talkgroup` is a number or a word of its name.",
            "inputSchema": { "type": "object", "properties": {
                "since": since, "until": time("Until when (default now)"),
                "talkgroup": { "type": "string" }, "group": { "type": "string" }, "system": { "type": "string" }, "unit": { "type": "string" },
                "limit": { "type": "integer", "description": "Default 120, at most 400" },
                "only_transcribed": { "type": "boolean", "description": "Leave out calls with no transcript (default true)" }
            }, "required": ["since"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "incident_context",
            "description": "The traffic around one call (the one that dispatched the event): its own talkgroup before and after, and the other talkgroups linked to it two ways: radios that answered it on the dispatch talkgroup and then spoke elsewhere, and calls elsewhere that name what the dispatch named ('Medic 6', 'Engine 6', an address number). Each line says why it is there. These are good guesses, not proof: check the times and words.",
            "inputSchema": { "type": "object", "properties": {
                "call_id": { "type": "integer" },
                "minutes_before": { "type": "number", "description": "Default 3" }, "minutes_after": { "type": "number", "description": "Default 20" },
                "limit": { "type": "integer", "description": "Default 80" }
            }, "required": ["call_id"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "list_talkgroups",
            "description": "Talkgroups that were active since `since`, with their number, agency and call counts: to find the number for a name, or to see which agencies are busy. `query` is a word of a talkgroup's name, description or agency.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string" }, "since": { "type": "string", "description": "Default 24h" }, "limit": { "type": "integer" } } },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_call",
            "description": "One call in full: its time, talkgroup, the radios that spoke, the whole transcript, and where its audio file is.",
            "inputSchema": { "type": "object", "properties": { "call_id": { "type": "integer" } }, "required": ["call_id"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_audio",
            "description": "A call's recording itself, as an audio file the client receives (WAV, or whatever the recorder saved). Take the id from any other tool's [id]. Recordings larger than the plugin's limit are refused: the same file can be downloaded from `GET /audio/<id>` on this server with the same token.",
            "inputSchema": { "type": "object", "properties": { "call_id": { "type": "integer" } }, "required": ["call_id"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "index_status",
            "description": "How much of the recordings the index has read (the newest days are read first), and what it is set to include.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        }
    ])
}

/// A tool's answer as MCP content: text, and for `get_audio` the recording too.
pub fn call_content(ctx: &Ctx, name: &str, args: &Value) -> Result<Vec<Value>, String> {
    if name == "get_audio" {
        return get_audio(ctx, args);
    }
    call(ctx, name, args).map(|t| vec![json!({ "type": "text", "text": t })])
}

pub fn call(ctx: &Ctx, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "recent_activity" => recent_activity(ctx, args),
        "search_traffic" => search_traffic(ctx, args),
        "get_traffic" => get_traffic(ctx, args),
        "incident_context" => incident_context(ctx, args),
        "list_talkgroups" => list_talkgroups(ctx, args),
        "get_call" => get_call(ctx, args),
        "index_status" => index_status(ctx),
        _ => Err(format!("no tool named {name}")),
    }
}

// ── Arguments ───────────────────────────────────────────────────────────────

fn text<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a[k].as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn int(a: &Value, k: &str, default: i64, max: i64) -> i64 {
    a[k].as_i64().or_else(|| a[k].as_f64().map(|f| f as i64)).or_else(|| a[k].as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(default).clamp(1, max)
}

fn flag(a: &Value, k: &str, default: bool) -> bool {
    a[k].as_bool().unwrap_or(default)
}

fn time_arg(a: &Value, k: &str) -> Result<Option<i64>, String> {
    text(a, k).map(|s| parse_time(s, Local::now())).transpose()
}

fn filter(ctx: &Ctx, a: &Value, since_default: Option<&str>, transcribed_only: bool, emergency_only: bool) -> Result<Filter, String> {
    let since = match text(a, "since").or(since_default) {
        Some(s) => Some(parse_time(s, Local::now())?),
        None => None,
    };
    Ok(Filter {
        since_ms: since,
        until_ms: time_arg(a, "until")?,
        system: text(a, "system").map(String::from),
        talkgroup: text(a, "talkgroup").map(String::from).or_else(|| a["talkgroup"].as_i64().map(|n| n.to_string())),
        group: text(a, "group").map(String::from),
        unit: text(a, "unit").map(|u| u.parse::<i64>().map_err(|_| format!("a unit is a radio number, not {u:?}"))).transpose()?.or_else(|| a["unit"].as_i64()),
        emergency_only,
        transcribed_only,
        policy: ctx.settings().policy,
    })
}

// ── Wording ─────────────────────────────────────────────────────────────────

fn unit_label(s: &Settings, u: Option<i64>) -> String {
    match u {
        None => "?".into(),
        Some(u) => s.units.get(&u).map_or_else(|| u.to_string(), |n| format!("{u} ({n})")),
    }
}

fn line(s: &Settings, r: &Call, full: bool, extra: &str) -> String {
    let secs = ((r.end_ms.max(r.start_ms) - r.start_ms) as f64 / 1000.0).round() as i64;
    let tag = if r.tag.is_empty() { format!("TG {}", r.tg) } else { r.tag.clone() };
    let mut head = format!(
        "[{}] {} {} (tg {}, {}) unit {} {}:{:02}{}{}",
        r.id,
        fmt_clock(r.start_ms),
        tag,
        r.tg,
        if r.group.is_empty() { &r.system } else { &r.group },
        unit_label(s, r.last_unit),
        secs / 60,
        secs % 60,
        if r.emergency { " EMERGENCY" } else { "" },
        if r.encrypted { " [encrypted]" } else { "" }
    );
    if !extra.is_empty() {
        head.push_str(&format!(" <{extra}>"));
    }
    if !r.has_text {
        return format!("{head} — (no transcript)");
    }
    if r.text.is_empty() {
        return format!("{head} — (nothing recognised)");
    }
    let mut t = r.text.clone();
    if !full && t.chars().count() > 500 {
        t = t.chars().take(500).collect::<String>() + "…";
    }
    format!("{head} — \"{t}\"")
}

/// One line for a transmission both of two sites (or two systems) recorded: the same talkgroup, a moment apart, the same words.
pub fn dedupe(mut rows: Vec<Call>) -> Vec<Call> {
    rows.sort_by_key(|r| r.start_ms);
    let mut out: Vec<Call> = vec![];
    let mut recent: HashMap<i64, Vec<usize>> = HashMap::new();
    for r in rows {
        let twins = recent.entry(r.tg).or_default();
        let twin = twins.iter().any(|&i| {
            let t: &Call = &out[i];
            (t.start_ms - r.start_ms).abs() < 2500
                && ((!t.text.is_empty() && t.text == r.text)
                    || (t.text.is_empty() && r.text.is_empty() && ((t.end_ms - t.start_ms) - (r.end_ms - r.start_ms)).abs() < 700))
        });
        if twin {
            continue;
        }
        twins.push(out.len());
        if twins.len() > 6 {
            twins.remove(0);
        }
        out.push(r);
    }
    out
}

fn note(ix: &Index) -> String {
    let s = ix.status();
    if !s.last_error.is_empty() {
        format!("\n(index problem: {})", s.last_error)
    } else if s.building && !s.ready && s.dirs_total > 0 {
        format!("\n(the index is still catching up on older days: {} of {} day folders checked, newest first)", s.dirs_done, s.dirs_total)
    } else {
        String::new()
    }
}

// ── The tools ───────────────────────────────────────────────────────────────

fn recent_activity(ctx: &Ctx, a: &Value) -> Result<String, String> {
    let minutes = int(a, "minutes", 15, 24 * 60);
    let sample = int(a, "sample", 12, 50) as usize;
    let since = format!("{minutes}m");
    let base = Value::Object(a.as_object().cloned().unwrap_or_default().into_iter().chain([("since".to_string(), json!(since))]).collect());
    let conn = ctx.index.reader()?;
    let s = ctx.settings();
    let (total, tgs) = queries::activity(&conn, &filter(ctx, &base, None, false, false)?, 25)?;
    let emerg = queries::traffic(&conn, &filter(ctx, &base, None, false, true)?, 20, true)?;
    let latest = queries::traffic(&conn, &filter(ctx, &base, None, true, false)?, sample, true)?;
    let mut out = vec![format!(
        "Last {minutes} min (to {}): {total} calls on {}{} talkgroups.",
        fmt_clock(Local::now().timestamp_millis()),
        tgs.len(),
        if tgs.len() >= 25 { "+" } else { "" }
    )];
    if !tgs.is_empty() {
        out.push("Busiest talkgroups (calls, transcribed, last heard):".into());
        for t in &tgs {
            let e = if t.emergencies > 0 { format!(", {} EMERGENCY", t.emergencies) } else { String::new() };
            out.push(format!(
                "  {} (tg {}, {}): {}, {}, {}{e}",
                if t.tag.is_empty() { format!("TG {}", t.tg) } else { t.tag.clone() },
                t.tg,
                if t.group.is_empty() { "?" } else { &t.group },
                t.n,
                t.transcribed,
                fmt_clock(t.last_ms)
            ));
        }
    }
    if !emerg.is_empty() {
        out.push("Emergency-flagged calls:".into());
        out.extend(dedupe(emerg).iter().map(|r| format!("  {}", line(&s, r, false, ""))));
    }
    if !latest.is_empty() {
        out.push("Newest transcripts:".into());
        out.extend(dedupe(latest).iter().map(|r| format!("  {}", line(&s, r, false, ""))));
    }
    Ok(out.join("\n") + &note(&ctx.index))
}

fn search_traffic(ctx: &Ctx, a: &Value) -> Result<String, String> {
    let words = text(a, "text").ok_or("`text` is what to search for")?;
    let limit = int(a, "limit", 30, 200) as usize;
    let newest = flag(a, "newest_first", true);
    let f = filter(ctx, a, Some("24h"), true, flag(a, "emergency_only", false))?;
    let conn = ctx.index.reader()?;
    let mut rows = dedupe(queries::search(&conn, words, &f, limit * 2, newest)?); // oldest first
    if newest {
        rows = rows.split_off(rows.len().saturating_sub(limit));
        rows.reverse();
    } else {
        rows.truncate(limit);
    }
    let since = text(a, "since").unwrap_or("24h");
    if rows.is_empty() {
        return Ok(format!("Nothing said matching {words:?} since {since}.{}", note(&ctx.index)));
    }
    let s = ctx.settings();
    let lines: Vec<String> = rows.iter().map(|r| line(&s, r, false, "")).collect();
    Ok(format!("{} calls matching {words:?} (each with its call id):\n{}{}", lines.len(), lines.join("\n"), note(&ctx.index)))
}

fn get_traffic(ctx: &Ctx, a: &Value) -> Result<String, String> {
    text(a, "since").ok_or("`since` says where the window starts")?;
    let limit = int(a, "limit", 120, 400) as usize;
    let f = filter(ctx, a, None, flag(a, "only_transcribed", true), false)?;
    let conn = ctx.index.reader()?;
    let mut rows = dedupe(queries::traffic(&conn, &f, limit * 2, false)?);
    rows.truncate(limit);
    if rows.is_empty() {
        return Ok(format!("No calls in that window.{}", note(&ctx.index)));
    }
    let s = ctx.settings();
    Ok(rows.iter().map(|r| line(&s, r, false, "")).collect::<Vec<_>>().join("\n") + &note(&ctx.index))
}

fn incident_context(ctx: &Ctx, a: &Value) -> Result<String, String> {
    let id = a["call_id"].as_i64().ok_or("`call_id` is the [id] of a call")?;
    let before = (a["minutes_before"].as_f64().unwrap_or(3.0).clamp(0.0, 120.0) * 60_000.0) as i64;
    let after = (a["minutes_after"].as_f64().unwrap_or(20.0).clamp(0.0, 240.0) * 60_000.0) as i64;
    let limit = int(a, "limit", 80, 300) as usize;
    let conn = ctx.index.reader()?;
    let s = ctx.settings();
    let Some(seed) = queries::get(&conn, id)? else { return Ok(format!("No call with id {id}.")) };
    if seed.encrypted && !allowed(&s.policy, &seed.system) {
        return Ok(format!("No call with id {id}."));
    }
    let mut rows = dedupe(queries::incident(&conn, &seed, before, after, limit * 2, &s.policy)?);
    rows.truncate(limit);
    let mut out = vec![format!(
        "Around [{}] {} on {}: {} calls, its own talkgroup, and those linked by answering radios or names.",
        seed.id,
        fmt_time(seed.start_ms, true),
        if seed.tag.is_empty() { seed.tg.to_string() } else { seed.tag.clone() },
        rows.len()
    )];
    for r in &rows {
        out.push(if r.id == seed.id { format!("> {}", line(&s, r, false, "")) } else { format!("  {}", line(&s, r, false, &r.link)) });
    }
    Ok(out.join("\n") + &note(&ctx.index))
}

fn allowed(p: &Policy, system: &str) -> bool {
    p.overrides.get(system).copied().unwrap_or(p.allow_encrypted)
}

fn list_talkgroups(ctx: &Ctx, a: &Value) -> Result<String, String> {
    let limit = int(a, "limit", 40, 200) as usize;
    let mut f = filter(ctx, a, Some("24h"), false, false)?;
    f.talkgroup = text(a, "query").map(String::from);
    let conn = ctx.index.reader()?;
    let (total, tgs) = queries::activity(&conn, &f, limit)?;
    if tgs.is_empty() {
        return Ok(format!("No talkgroups matched.{}", note(&ctx.index)));
    }
    let since = text(a, "since").unwrap_or("24h");
    let mut out = vec![format!("{total} calls since {since}.")];
    for t in &tgs {
        out.push(format!(
            "  tg {}  {}  ({})  {} calls, last {}",
            t.tg,
            if t.tag.is_empty() { "?" } else { &t.tag },
            if t.group.is_empty() { "?" } else { &t.group },
            t.n,
            fmt_time(t.last_ms, false)
        ));
    }
    Ok(out.join("\n") + &note(&ctx.index))
}

fn get_call(ctx: &Ctx, a: &Value) -> Result<String, String> {
    let id = a["call_id"].as_i64().ok_or("`call_id` is the [id] of a call")?;
    let conn = ctx.index.reader()?;
    let s = ctx.settings();
    let Some(r) = queries::get(&conn, id)?.filter(|r| !r.encrypted || allowed(&s.policy, &r.system)) else { return Ok(format!("No call with id {id}.")) };
    let radios: Vec<String> = r.units.split_whitespace().filter_map(|u| u.parse().ok()).map(|u: i64| unit_label(&s, Some(u))).collect();
    let mut out = vec![
        line(&s, &r, true, ""),
        format!("started {}, system {}, site {}", fmt_time(r.start_ms, true), r.system, r.site),
        format!("radios: {}", if radios.is_empty() { "unknown".into() } else { radios.join(", ") }),
        format!("transcribed by: {}", if r.server.is_empty() { "not transcribed" } else { &r.server }),
    ];
    if let Some((path, _)) = r.has_audio.then(|| find_audio(&ctx.index.root, &r.key)).flatten() {
        out.push(format!("file: {}", path.display()));
        if !s.audio_base.is_empty() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("wav");
            out.push(format!("listen: {}/calls/{}.{ext}", s.audio_base.trim_end_matches('/'), r.key));
        }
        out.push(format!("audio: get_audio tool, or GET /audio/{} on this server", r.id));
    } else {
        out.push("no audio file kept".into());
    }
    Ok(out.join("\n"))
}

fn index_status(ctx: &Ctx) -> Result<String, String> {
    let conn = ctx.index.reader()?;
    let (n, lo, hi, tx): (i64, Option<i64>, Option<i64>, Option<i64>) = conn
        .query_row("SELECT COUNT(*), MIN(start_ms), MAX(start_ms), SUM(has_text) FROM calls", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|e| e.to_string())?;
    let st = ctx.index.status();
    let s = ctx.settings();
    let enc = if s.policy.allow_encrypted { "included" } else { "left out (turn on 'Include encrypted calls' in the plugin's settings)" };
    Ok(format!(
        "{n} calls indexed, {} with transcripts, from {} to {}.\nWindow: the last {} days. {}\nEncrypted calls: {enc}.\nRecordings: {}{}",
        tx.unwrap_or(0),
        lo.map_or("-".into(), |t| fmt_time(t, true)),
        hi.map_or("-".into(), |t| fmt_time(t, true)),
        s.days,
        if st.building && !st.ready { format!("Reading the first time: {} of {} day folders.", st.dirs_done, st.dirs_total) } else { "Up to date.".into() },
        ctx.index.root.display(),
        if st.last_error.is_empty() { String::new() } else { format!("\nProblem: {}", st.last_error) }
    ))
}

// ── Recordings ──────────────────────────────────────────────────────────────

const AUDIO_TYPES: [(&str, &str); 3] = [("wav", "audio/wav"), ("m4a", "audio/mp4"), ("mp3", "audio/mpeg")];

fn find_audio(root: &std::path::Path, key: &str) -> Option<(std::path::PathBuf, &'static str)> {
    AUDIO_TYPES.iter().map(|(ext, mime)| (root.join(format!("{key}.{ext}")), *mime)).find(|(p, _)| p.is_file())
}

pub struct Audio {
    pub path: std::path::PathBuf,
    pub mime: &'static str,
    pub bytes: u64,
    pub call: Call,
}

/// The recording of a call, if there is one and the encrypted-calls setting lets it out.
pub fn audio_file(ctx: &Ctx, id: i64) -> Result<Audio, String> {
    let conn = ctx.index.reader()?;
    let s = ctx.settings();
    let call = queries::get(&conn, id)?.filter(|r| !r.encrypted || allowed(&s.policy, &r.system)).ok_or(format!("No call with id {id}."))?;
    let (path, mime) = find_audio(&ctx.index.root, &call.key).ok_or(format!("Call {id} has no audio file kept."))?;
    let bytes = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    Ok(Audio { path, mime, bytes, call })
}

fn get_audio(ctx: &Ctx, a: &Value) -> Result<Vec<Value>, String> {
    use base64::Engine;
    let id = a["call_id"].as_i64().ok_or("`call_id` is the [id] of a call")?;
    let au = audio_file(ctx, id)?;
    let s = ctx.settings();
    if au.bytes > s.audio_max {
        return Err(format!(
            "The recording is {} KB, over the {} KB limit for a reply: download it from GET /audio/{id} on this server instead.",
            au.bytes / 1024,
            s.audio_max / 1024
        ));
    }
    let data = std::fs::read(&au.path).map_err(|e| format!("can't read the recording: {e}"))?;
    Ok(vec![
        json!({ "type": "text", "text": line(&s, &au.call, true, "") }),
        json!({ "type": "audio", "data": base64::engine::general_purpose::STANDARD.encode(data), "mimeType": au.mime }),
    ])
}
