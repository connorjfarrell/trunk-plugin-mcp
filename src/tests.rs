use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use chrono::{Datelike, Local};
use serde_json::{Value, json};
use trunk_recorder_plugin::{Plugin, testing};

use crate::config::Config;
use crate::index::Index;
use crate::queries::{self, Filter, Policy};
use crate::tools::{Ctx, Settings, dedupe};
use crate::{http, mcp};

fn t0() -> i64 {
    Local::now().timestamp() - 600
}

#[allow(clippy::too_many_arguments)]
fn write_call(root: &Path, system: &str, tg: i64, tag: &str, group: &str, start: i64, units: &[i64], text: Option<&str>, enc: bool, name: Option<String>) {
    let d = Local::now().date_naive();
    let folder = root.join(system).join(d.year().to_string()).join(d.month().to_string()).join(d.day().to_string());
    std::fs::create_dir_all(&folder).unwrap();
    let stem = name.unwrap_or_else(|| format!("{tg}-{start}_851000000.0"));
    let rec = json!({
        "talkgroup": tg, "talkgroup_tag": tag, "talkgroup_group": group, "talkgroup_description": format!("{tag} desc"), "talkgroup_group_tag": "Fire",
        "start_time": start, "start_time_ms": start * 1000 + 123, "stop_time_ms": (start + 5) * 1000 + 123, "call_length": 5,
        "short_name": system, "site": "s_800", "emergency": 0, "encrypted": i32::from(enc), "decrypted": i32::from(enc),
        "srcList": units.iter().map(|u| json!({ "src": u })).collect::<Vec<_>>(),
    });
    std::fs::write(folder.join(format!("{stem}.json")), rec.to_string()).unwrap();
    std::fs::write(folder.join(format!("{stem}.wav")), b"RIFF").unwrap();
    if let Some(t) = text {
        std::fs::write(folder.join(format!("{stem}.transcript.json")), json!({ "text": t, "server": "test" }).to_string()).unwrap();
    }
}

fn world() -> (Arc<Ctx>, PathBuf) {
    let dir = testing::temp_dir("mcp");
    let root = dir.join("calls");
    let t = t0();
    write_call(&root, "sysA", 100, "Fire Disp", "Metro Fire", t, &[9001], Some("Medic 3 respond to 1200 Main Street for a cardiac arrest"), false, None);
    write_call(&root, "sysA", 100, "Fire Disp", "Metro Fire", t + 20, &[3003], Some("Medic 3 copy"), false, None);
    write_call(&root, "sysA", 200, "Fire Tac 1", "Metro Fire", t + 200, &[3003], Some("Medic 3 on scene, starting CPR"), false, None);
    write_call(&root, "sysA", 300, "Police Disp", "Metro PD", t + 210, &[5005], Some("Unit 12 traffic stop on Oak Avenue"), false, None);
    write_call(&root, "sysA", 300, "Police Disp", "Metro PD", t + 400, &[9001], Some("Ten four"), true, None);
    write_call(&root, "sysA", 400, "Fire Tac 2", "Metro Fire", t + 300, &[4004], Some("Medic 3 is transporting to Medical Center"), false, None);
    write_call(&root, "sysA", 400, "Fire Tac 2", "Metro Fire", t + 320, &[4004], Some("Engine 9 is back in service"), false, None);
    write_call(&root, "sysA", 100, "Fire Disp", "Metro Fire", t + 600, &[9001], None, false, None);
    write_call(
        &root,
        "sysB",
        100,
        "Fire Disp",
        "Metro Fire",
        t,
        &[9001],
        Some("Medic 3 respond to 1200 Main Street for a cardiac arrest"),
        false,
        Some(format!("100-{t}_852000000.0")),
    );
    let ix = Arc::new(Index::open(&root, &dir.join("data/index.db")).unwrap());
    ix.pass(3, &|_| false, &|| false);
    let ctx = Arc::new(Ctx { index: ix, settings: RwLock::new(Settings { days: 3, audio_max: 1 << 20, ..Default::default() }) });
    (ctx, root)
}

fn set_policy(ctx: &Ctx, allow: bool, overrides: &[(&str, bool)]) {
    ctx.settings.write().unwrap().policy =
        Policy { allow_encrypted: allow, overrides: overrides.iter().map(|(s, a)| (s.to_string(), *a)).collect::<HashMap<_, _>>() };
}

fn call_tool(ctx: &Ctx, name: &str, args: Value) -> String {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
    let (status, out) = mcp::handle(ctx, body.to_string().as_bytes());
    assert_eq!(status, 200);
    let v: Value = serde_json::from_str(&out.unwrap()).unwrap();
    assert_eq!(v["result"]["isError"], false, "{v}");
    v["result"]["content"][0]["text"].as_str().unwrap().to_string()
}

#[test]
fn the_index_reads_calls_and_their_transcripts() {
    let (ctx, _) = world();
    assert_eq!(ctx.index.status().last_error, "");
    let c = ctx.index.reader().unwrap();
    assert_eq!(c.query_row("SELECT COUNT(*) FROM calls", [], |r| r.get::<_, i64>(0)).unwrap(), 9);
    assert_eq!(c.query_row("SELECT SUM(has_text) FROM calls", [], |r| r.get::<_, i64>(0)).unwrap(), 8);
}

#[test]
fn search_by_words_alternatives_phrases_and_prefixes_and_encrypted_left_out() {
    let (ctx, _) = world();
    let c = ctx.index.reader().unwrap();
    let f = Filter { since_ms: Some(0), ..Default::default() };
    let n = |q: &str, f: &Filter| queries::search(&c, q, f, 10, true).unwrap().len();
    assert_eq!(n("cardiac arrest", &f), 2, "the twin is a second row; dedupe is the tools'");
    assert_eq!(n("cardiac AND cpr", &f), 0);
    assert_eq!(n("cpr OR traffic", &f), 2);
    assert_eq!(n("transport*", &f), 1);
    assert_eq!(n("\"traffic stop\"", &f), 1);
    assert_eq!(n("ten four", &f), 0, "encrypted: left out");
    assert_eq!(n("ten four", &Filter { policy: Policy { allow_encrypted: true, ..Default::default() }, ..f.clone() }), 1);
    assert_eq!(
        n("ten four", &Filter { policy: Policy { allow_encrypted: false, overrides: [("sysA".to_string(), true)].into() }, ..f.clone() }),
        1,
        "a system can allow it"
    );
    assert_eq!(n("bad \"syntax", &f), 0, "doesn't fail");
}

#[test]
fn twin_copies_are_one_line() {
    let (ctx, _) = world();
    let c = ctx.index.reader().unwrap();
    let rows = queries::search(&c, "cardiac", &Filter { since_ms: Some(0), ..Default::default() }, 10, true).unwrap();
    assert_eq!((rows.len(), dedupe(rows).len()), (2, 1));
}

#[test]
fn an_incident_follows_answering_radios_and_names() {
    let (ctx, _) = world();
    let c = ctx.index.reader().unwrap();
    let seed = queries::search(&c, "respond", &Filter { since_ms: Some(0), ..Default::default() }, 10, false).unwrap().remove(0);
    let rows = queries::incident(&c, &seed, 180_000, 1_200_000, 50, &Policy::default()).unwrap();
    let rows = dedupe(rows);
    let text = |r: &queries::Call| r.text.clone();
    let tac = rows.iter().find(|r| text(r).contains("starting CPR")).expect("the tac channel, by the radio that answered");
    assert!(tac.link.starts_with("answering radio") && tac.link.contains("3003"), "{}", tac.link);
    assert!(
        rows.iter().any(|r| text(r).contains("transporting") && r.link.contains("names \"medic 3\"")),
        "named by the dispatch, by a radio that never answered"
    );
    assert!(!rows.iter().any(|r| text(r).contains("traffic stop")), "an unrelated call");
    assert!(!rows.iter().any(|r| text(r).contains("Engine 9")), "a different unit");
}

#[test]
fn a_transcript_that_comes_later_is_picked_up() {
    let (ctx, root) = world();
    let t = t0() + 600;
    let d = Local::now().date_naive();
    let folder = root.join("sysA").join(d.year().to_string()).join(d.month().to_string()).join(d.day().to_string());
    std::fs::write(folder.join(format!("100-{t}_851000000.0.transcript.json")), json!({ "text": "Returned to quarters", "server": "late" }).to_string())
        .unwrap();
    ctx.index.pass(3, &|_| false, &|| false);
    let c = ctx.index.reader().unwrap();
    assert_eq!(queries::search(&c, "returned to quarters", &Filter { since_ms: Some(0), ..Default::default() }, 5, true).unwrap().len(), 1);
    assert_eq!(c.query_row("SELECT COUNT(*) FROM calls", [], |r| r.get::<_, i64>(0)).unwrap(), 9, "no call twice");
}

#[test]
fn a_skipped_system_is_not_indexed() {
    let dir = testing::temp_dir("mcp-skip");
    write_call(&dir.join("calls"), "keep", 1, "A", "G", t0(), &[1], Some("hello"), false, None);
    write_call(&dir.join("calls"), "left", 1, "A", "G", t0(), &[1], Some("hello"), false, None);
    let ix = Index::open(&dir.join("calls"), &dir.join("i.db")).unwrap();
    ix.pass(3, &|s| s == "left", &|| false);
    let n: i64 = ix.reader().unwrap().query_row("SELECT COUNT(*) FROM calls", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[test]
fn the_tools_answer_in_words() {
    let (ctx, _) = world();
    ctx.settings.write().unwrap().units.insert(9001, "Dispatch 1".into());
    let out = call_tool(&ctx, "recent_activity", json!({ "minutes": 30 }));
    assert!(out.contains("Fire Disp") && out.contains("Dispatch 1") && !out.to_lowercase().contains("ten four"), "{out}");
    let out = call_tool(&ctx, "search_traffic", json!({ "text": "cardiac arrest", "since": "1h" }));
    assert_eq!(out.matches("1200 Main Street").count(), 1, "{out}");
    let id: i64 = out.split('[').nth(1).unwrap().split(']').next().unwrap().parse().unwrap();
    let out = call_tool(&ctx, "incident_context", json!({ "call_id": id }));
    assert!(out.contains("starting CPR") && !out.contains("traffic stop"), "{out}");
    let out = call_tool(&ctx, "get_call", json!({ "call_id": id }));
    assert!(out.contains("Dispatch 1") && out.contains(".wav"), "{out}");
    assert!(call_tool(&ctx, "index_status", json!({})).contains("9 calls indexed"));
    assert!(call_tool(&ctx, "get_traffic", json!({ "since": "1m", "talkgroup": "999" })).contains("No calls"));
    assert!(call_tool(&ctx, "list_talkgroups", json!({ "query": "Fire" })).contains("tg 100"));
    let newest = call_tool(&ctx, "search_traffic", json!({ "text": "medic", "limit": 2 }));
    assert_eq!(newest.lines().count(), 3, "a header and two calls: {newest}");
    assert!(newest.lines().nth(1).unwrap().contains("transporting"), "the newest first: {newest}");
}

#[test]
fn encrypted_calls_follow_the_setting() {
    let (ctx, _) = world();
    assert!(!call_tool(&ctx, "search_traffic", json!({ "text": "ten four" })).contains("Ten four"));
    set_policy(&ctx, true, &[]);
    assert!(call_tool(&ctx, "search_traffic", json!({ "text": "ten four" })).contains("[encrypted]"));
    set_policy(&ctx, true, &[("sysA", false)]);
    assert!(!call_tool(&ctx, "search_traffic", json!({ "text": "ten four" })).contains("Ten four"), "a system can say no");
}

#[test]
fn the_protocol_handshake() {
    let (ctx, _) = world();
    let ask = |v: Value| mcp::handle(&ctx, v.to_string().as_bytes());
    let (s, out) = ask(
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } } }),
    );
    let v: Value = serde_json::from_str(&out.unwrap()).unwrap();
    assert_eq!((s, v["result"]["protocolVersion"].as_str(), v["result"]["serverInfo"]["name"].as_str()), (200, Some("2025-03-26"), Some("trunk-recorder")));
    assert_eq!(ask(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })), (202, None));
    let (_, out) = ask(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let v: Value = serde_json::from_str(&out.unwrap()).unwrap();
    let names: Vec<&str> = v["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["recent_activity", "search_traffic", "get_traffic", "incident_context", "list_talkgroups", "get_call", "get_audio", "index_status"]);
    let (_, out) = ask(json!({ "jsonrpc": "2.0", "id": 3, "method": "nonsense" }));
    assert!(out.unwrap().contains("-32601"));
    let (s, _) = mcp::handle(&ctx, b"not json");
    assert_eq!(s, 400);
    let (_, out) = ask(
        json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "search_traffic", "arguments": { "text": "x", "since": "when the sirens started" } } }),
    );
    let v: Value = serde_json::from_str(&out.unwrap()).unwrap();
    assert_eq!(v["result"]["isError"], true, "a bad time is the tool's error, for the model to read");
}

fn http_request(addr: &str, head: &str, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    write!(s, "{head}\r\nHost: {addr}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out.split(' ').nth(1).unwrap().parse().unwrap();
    (status, out.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
}

#[test]
fn the_server_wants_its_token_and_keeps_other_sites_out() {
    let (ctx, _) = world();
    ctx.settings.write().unwrap().token = "a-long-enough-secret".into();
    ctx.settings.write().unwrap().allowed = crate::net::parse_list("127.0.0.0/8").unwrap();
    let server = http::Server::start("127.0.0.1", 0, ctx.clone()).unwrap();
    let a = server.addr.clone();
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string();
    assert_eq!(http_request(&a, "POST /mcp HTTP/1.1", &body).0, 401, "no token");
    assert_eq!(http_request(&a, "POST /mcp HTTP/1.1\r\nAuthorization: Bearer wrong-secret-here!", &body).0, 401);
    let (s, out) = http_request(&a, "POST /mcp HTTP/1.1\r\nAuthorization: Bearer a-long-enough-secret\r\nContent-Type: application/json", &body);
    assert_eq!(s, 200);
    assert!(out.contains("search_traffic"));
    assert_eq!(http_request(&a, "POST /mcp HTTP/1.1\r\nAuthorization: Bearer a-long-enough-secret\r\nOrigin: http://evil.example", &body).0, 403);
    assert_eq!(http_request(&a, "GET /mcp HTTP/1.1\r\nAuthorization: Bearer a-long-enough-secret", "").0, 405);
    assert_eq!(
        http_request(&a, "POST /mcp HTTP/1.1\r\nAuthorization: Bearer a-long-enough-secret", r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).0,
        202
    );
    assert_eq!(http_request(&a, "GET /health HTTP/1.1", "").0, 200);
    assert_eq!(http_request(&a, "GET /elsewhere HTTP/1.1", "").0, 404);
    // Another network is refused, token or not; the list can change while it runs.
    ctx.settings.write().unwrap().allowed = crate::net::parse_list("192.168.0.0/16").unwrap();
    assert_eq!(http_request(&a, "POST /mcp HTTP/1.1\r\nAuthorization: Bearer a-long-enough-secret", &body).0, 403);
    assert_eq!(http_request(&a, "GET /health HTTP/1.1", "").0, 403);
    ctx.settings.write().unwrap().allowed = crate::net::parse_list("127.0.0.0/8").unwrap();
    // The token can change while it runs.
    ctx.settings.write().unwrap().token = String::new();
    assert_eq!(http_request(&a, "POST /mcp HTTP/1.1", &body).0, 200);
    server.stop();
}

#[test]
fn settings_are_checked_and_the_plugin_says_what_it_is() {
    let c = |bind: &str, token: &str| Config { bind: bind.into(), token: token.into(), ..Default::default() };
    assert!(c("127.0.0.1", "").check().is_ok());
    assert!(c("0.0.0.0", "").check().is_err(), "the network needs a token");
    assert!(c("0.0.0.0", "short").check().is_err());
    assert!(c("0.0.0.0", "a-long-enough-secret").check().is_ok());
    assert!(Config { port: 0, ..Default::default() }.check().is_err());
    assert!(Config { allowed_networks: "172.16.0.0/12, 192.168.0.0/16".into(), ..Default::default() }.check().is_ok());
    assert!(Config { allowed_networks: "banana".into(), ..Default::default() }.check().is_err());
    let m = crate::Mcp::manifest();
    assert_eq!(m.subscribe, vec!["call.concluded".to_string()]);
    assert!(m.live_settings);
    assert_eq!(m.id, "mcp");
}

fn tool_content(ctx: &Ctx, name: &str, args: Value) -> (Vec<Value>, bool) {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
    let (_, out) = mcp::handle(ctx, body.to_string().as_bytes());
    let v: Value = serde_json::from_str(&out.unwrap()).unwrap();
    (v["result"]["content"].as_array().unwrap().clone(), v["result"]["isError"].as_bool().unwrap())
}

fn id_of(ctx: &Ctx, words: &str) -> i64 {
    let out = call_tool(ctx, "search_traffic", json!({ "text": words, "since": "1h" }));
    out.split('[').nth(1).unwrap().split(']').next().unwrap().parse().unwrap()
}

#[test]
fn a_recording_comes_back_as_audio() {
    let (ctx, root) = world();
    let id = id_of(&ctx, "cardiac arrest");
    let (c, err) = tool_content(&ctx, "get_audio", json!({ "call_id": id }));
    assert!(!err && c.len() == 2, "{c:?}");
    assert!(c[0]["text"].as_str().unwrap().contains("1200 Main Street"));
    assert_eq!(
        (c[1]["type"].as_str(), c[1]["mimeType"].as_str(), c[1]["data"].as_str()),
        (Some("audio"), Some("audio/wav"), Some("UklGRg==")),
        "the file's bytes"
    );
    assert!(call_tool(&ctx, "get_call", json!({ "call_id": id })).contains(&format!("GET /audio/{id}")));
    // Too big for a reply: pointed at the download instead.
    ctx.settings.write().unwrap().audio_max = 2;
    let (c, err) = tool_content(&ctx, "get_audio", json!({ "call_id": id }));
    assert!(err && c[0]["text"].as_str().unwrap().contains("/audio/"), "{c:?}");
    ctx.settings.write().unwrap().audio_max = 1 << 20;
    // A call with its file gone, and one that does not exist.
    remove_wavs(&root);
    let (c, err) = tool_content(&ctx, "get_audio", json!({ "call_id": id }));
    assert!(err && c[0]["text"].as_str().unwrap().contains("no audio"), "{c:?}");
    assert!(tool_content(&ctx, "get_audio", json!({ "call_id": 987_654 })).1);
}

fn remove_wavs(dir: &Path) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        if e.path().is_dir() {
            remove_wavs(&e.path());
        } else if e.file_name().to_string_lossy().ends_with(".wav") {
            std::fs::remove_file(e.path()).unwrap();
        }
    }
}

#[test]
fn encrypted_recordings_follow_the_setting_too() {
    let (ctx, _) = world();
    set_policy(&ctx, true, &[]);
    let id = id_of(&ctx, "ten four");
    assert!(!tool_content(&ctx, "get_audio", json!({ "call_id": id })).1);
    set_policy(&ctx, false, &[]);
    assert!(tool_content(&ctx, "get_audio", json!({ "call_id": id })).1, "left out when encrypted calls are");
}

#[test]
fn a_recording_downloads_over_http_with_the_token() {
    let (ctx, _) = world();
    ctx.settings.write().unwrap().token = "a-long-enough-secret".into();
    ctx.settings.write().unwrap().allowed = crate::net::parse_list("127.0.0.0/8").unwrap();
    let id = id_of(&ctx, "cardiac arrest");
    let server = http::Server::start("127.0.0.1", 0, ctx.clone()).unwrap();
    let a = server.addr.clone();
    assert_eq!(http_request(&a, &format!("GET /audio/{id} HTTP/1.1"), "").0, 401);
    let (s, body) = http_request(&a, &format!("GET /audio/{id}.wav HTTP/1.1\r\nAuthorization: Bearer a-long-enough-secret"), "");
    assert_eq!((s, body.as_str()), (200, "RIFF"));
    assert_eq!(http_request(&a, "GET /audio/987654 HTTP/1.1\r\nAuthorization: Bearer a-long-enough-secret", "").0, 404);
    assert_eq!(http_request(&a, "GET /audio/nope HTTP/1.1\r\nAuthorization: Bearer a-long-enough-secret", "").0, 400);
    server.stop();
}
