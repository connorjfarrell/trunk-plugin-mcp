//! mcp — a Trunk Recorder Pro plugin that lets Claude (or any MCP client) search and read the transcripts of recorded
//! calls, and follow an incident across talkgroups.
//!
//! It indexes the recordings folder (each call's JSON and the `.transcript.json` a transcription plugin writes beside it) in an
//! SQLite file, and serves MCP over HTTP: `POST http://host:port/mcp`, optionally with a bearer token.

mod config;
mod http;
mod index;
mod mcp;
mod net;
mod queries;
mod timeparse;
mod tools;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use trunk_recorder_plugin::{ConcludedCall, Host, Manifest, Plugin, Setup, State, topic};

use config::{Config, SystemConfig};
use index::Index;
use queries::Policy;
use tools::{Ctx, Settings};

/// How often the index looks at the newest days again: a transcript arrives some seconds after its call.
const EVERY: Duration = Duration::from_secs(20);

struct Mcp {
    host: Host,
    ctx: Arc<Ctx>,
    server: Option<http::Server>,
    nudge: Option<Sender<()>>,
    stop: Arc<AtomicBool>,
    skip: Arc<RwLock<HashSet<String>>>,
    indexer: Option<JoinHandle<()>>,
    listening: (String, u16),
}

fn settings_of(setup: &Setup<Config, SystemConfig>) -> (Settings, HashSet<String>) {
    let c = &setup.config;
    let overrides: HashMap<String, bool> = setup
        .systems
        .iter()
        .filter(|s| s.settings.include_encrypted != c.include_encrypted)
        .map(|s| (s.short_name.clone(), s.settings.include_encrypted))
        .collect();
    let skip = setup.systems.iter().filter(|s| s.config.as_ref().is_some_and(|x| x.skip)).map(|s| s.short_name.clone()).collect();
    let settings = Settings {
        policy: Policy { allow_encrypted: c.include_encrypted, overrides },
        units: load_units(&c.unit_names_file),
        audio_base: c.audio_base.trim().to_string(),
        days: c.days,
        audio_max: u64::from(c.max_audio_mb) << 20,
        token: c.token.trim().to_string(),
        allowed: net::parse_list(&c.allowed_networks).unwrap_or_default(),
    };
    (settings, skip)
}

/// `number,name` lines (numbers only: a name pattern in the recorder's file is left out).
fn load_units(path: &str) -> HashMap<i64, String> {
    let mut out = HashMap::new();
    if path.trim().is_empty() {
        return out;
    }
    for l in std::fs::read_to_string(path.trim()).unwrap_or_default().lines() {
        if let Some((n, name)) = l.split_once(',')
            && let Ok(n) = n.trim().parse::<i64>()
        {
            out.insert(n, name.trim().trim_matches('"').to_string());
        }
    }
    out
}

impl Plugin for Mcp {
    type Config = Config;
    type SystemConfig = SystemConfig;

    fn manifest() -> Manifest {
        Manifest { name: "Claude (MCP)".into(), subscribe: vec![topic::CALL_CONCLUDED.into()], live_settings: true, ..trunk_recorder_plugin::manifest!() }
    }

    fn start(host: Host, setup: Setup<Config, SystemConfig>) -> Result<Self, String> {
        setup.config.check()?;
        be_gentle();
        let root: &Path = &setup.capture_dir;
        let index = Arc::new(Index::open(root, &setup.data_dir.join("index.db"))?);
        let (settings, skip) = settings_of(&setup);
        let ctx = Arc::new(Ctx { index: index.clone(), settings: RwLock::new(settings) });
        let server = http::Server::start(setup.config.bind.trim(), setup.config.port, ctx.clone())?;
        let stop = Arc::new(AtomicBool::new(false));
        let skip = Arc::new(RwLock::new(skip));
        let (tx, rx) = mpsc::channel::<()>();
        let indexer = {
            let (ctx, stop, skip, host, addr) = (ctx.clone(), stop.clone(), skip.clone(), host.clone(), server.addr.clone());
            std::thread::Builder::new()
                .name("mcp-index".into())
                .spawn(move || {
                    loop {
                        let days = ctx.settings.read().unwrap().days;
                        ctx.index.pass(days, &|s| skip.read().unwrap().contains(s), &|| stop.load(Ordering::Relaxed));
                        report(&host, &ctx, &addr);
                        match rx.recv_timeout(EVERY) {
                            Ok(()) => while rx.try_recv().is_ok() {},
                            Err(RecvTimeoutError::Timeout) => {}
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                })
                .map_err(|e| e.to_string())?
        };
        host.info(format!("serving MCP at http://{}/mcp; indexing {}", server.addr, root.display()));
        report(&host, &ctx, &server.addr);
        let listening = (setup.config.bind.trim().to_string(), setup.config.port);
        Ok(Mcp { host, ctx, server: Some(server), nudge: Some(tx), stop, skip, indexer: Some(indexer), listening })
    }

    fn call_concluded(&mut self, _call: ConcludedCall) {
        if let Some(tx) = &self.nudge {
            let _ = tx.send(());
        }
    }

    fn settings(&mut self, setup: Setup<Config, SystemConfig>) -> Result<(), String> {
        setup.config.check()?;
        if (setup.config.bind.trim(), setup.config.port) != (self.listening.0.as_str(), self.listening.1) {
            return Err("the port and address take effect when recording next starts".into());
        }
        let (settings, skip) = settings_of(&setup);
        *self.ctx.settings.write().unwrap() = settings;
        *self.skip.write().unwrap() = skip;
        self.host.info("settings changed");
        if let Some(tx) = &self.nudge {
            let _ = tx.send(());
        }
        Ok(())
    }

    fn shutdown(&mut self, _grace: Duration) {
        self.stop.store(true, Ordering::Relaxed);
        self.nudge = None;
        if let Some(s) = self.server.take() {
            s.stop();
        }
        if let Some(t) = self.indexer.take() {
            let _ = t.join();
        }
    }
}

/// The first pass reads every call in the window: do it at a lower priority than the recorder (threads started after this inherit it).
fn be_gentle() {
    let pid = std::process::id().to_string();
    for (cmd, args) in [("ionice", vec!["-c", "2", "-n", "7", "-p", &pid]), ("renice", vec!["-n", "10", "-p", &pid])] {
        let _ = std::process::Command::new(cmd).args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    }
}

fn report(host: &Host, ctx: &Ctx, addr: &str) {
    let st = ctx.index.status();
    let n: i64 = ctx.index.reader().ok().and_then(|c| c.query_row("SELECT COUNT(*) FROM calls", [], |r| r.get(0)).ok()).unwrap_or(0);
    let enc = if ctx.settings.read().unwrap().policy.allow_encrypted { " · encrypted calls included" } else { "" };
    if !st.last_error.is_empty() {
        host.status(State::Warning, format!("The index has a problem: {}", st.last_error));
    } else if st.building && !st.ready {
        host.status(
            State::Ok,
            format!("Reading the recordings the first time: {} of {} days · {n} calls · http://{addr}/mcp{enc}", st.dirs_done, st.dirs_total),
        );
    } else {
        host.status(State::Ok, format!("{n} calls indexed · http://{addr}/mcp{enc}"));
    }
}

fn main() {
    trunk_recorder_plugin::run::<Mcp>();
}

#[cfg(test)]
mod tests;
