//! The HTTP server MCP clients connect to: `POST /mcp` (streamable HTTP, answering each request with JSON), and `GET /health`.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use tiny_http::{Header, Method, Request, Response, Server as Tiny};

use crate::mcp;
use crate::tools::Ctx;

const MAX_BODY: u64 = 1 << 20;
const THREADS: usize = 4;

pub struct Server {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    /// Where it listens ("127.0.0.1:8770").
    pub addr: String,
}

pub fn is_local(bind: &str) -> bool {
    matches!(bind.trim(), "127.0.0.1" | "localhost" | "::1" | "[::1]") || bind.trim().starts_with("127.")
}

impl Server {
    pub fn start(bind: &str, port: u16, ctx: Arc<Ctx>) -> Result<Server, String> {
        let tiny = Tiny::http(format!("{bind}:{port}")).map_err(|e| format!("can't listen on {bind}:{port}: {e}"))?;
        let addr = tiny.server_addr().to_ip().map_or_else(|| format!("{bind}:{port}"), |a| a.to_string());
        let tiny = Arc::new(tiny);
        let stop = Arc::new(AtomicBool::new(false));
        let threads = (0..THREADS)
            .map(|_| {
                let (tiny, stop, ctx) = (tiny.clone(), stop.clone(), ctx.clone());
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match tiny.recv_timeout(Duration::from_millis(300)) {
                            Ok(Some(req)) => serve(&ctx, req),
                            Ok(None) => {}
                            Err(_) => break,
                        }
                    }
                })
            })
            .collect();
        Ok(Server { stop, threads, addr })
    }

    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads {
            let _ = t.join();
        }
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

fn get(req: &Request, name: &str) -> Option<String> {
    req.headers().iter().find(|h| h.field.to_string().eq_ignore_ascii_case(name)).map(|h| h.value.as_str().to_string())
}

fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn host_of(v: &str) -> &str {
    let v = v.trim();
    let v = v.split_once("://").map_or(v, |(_, r)| r);
    v.split('/').next().unwrap_or("")
}

fn serve(ctx: &Ctx, mut req: Request) {
    let (token, allowed) = {
        let s = ctx.settings.read().unwrap();
        (s.token.clone(), s.allowed.clone())
    };
    // Who may talk to it at all: a network the user named.
    if !req.remote_addr().is_some_and(|a| allowed.iter().any(|n| n.contains(a.ip()))) {
        let _ = req.respond(
            Response::from_string(r#"{"error":"not from this network"}"#).with_status_code(403).with_header(header("Content-Type", "application/json")),
        );
        return;
    }
    let path = req.url().split('?').next().unwrap_or("").to_string();
    let reply = |req: Request, status: u16, body: &str, extra: Vec<Header>| {
        let mut r = Response::from_string(body).with_status_code(status).with_header(header("Content-Type", "application/json"));
        for h in extra {
            r = r.with_header(h);
        }
        let _ = req.respond(r);
    };
    match (req.method().clone(), path.as_str()) {
        (Method::Get, "/" | "/health") => {
            let _ = req.respond(Response::from_string("ok").with_header(header("Content-Type", "text/plain")));
        }
        (m, "/mcp") => {
            // A page in a browser can't make a request to this unless it says where it comes from: keep other sites out.
            let (origin, host) = (get(&req, "Origin"), get(&req, "Host"));
            if let (Some(origin), Some(host)) = (&origin, &host)
                && !host_of(origin).eq_ignore_ascii_case(host_of(host))
            {
                return reply(req, 403, r#"{"error":"another site"}"#, vec![]);
            }
            if !token.is_empty() {
                let auth = get(&req, "Authorization").unwrap_or_default();
                let given = auth.strip_prefix("Bearer ").or_else(|| auth.strip_prefix("bearer ")).unwrap_or("");
                if !same_secret(given.trim(), &token) {
                    return reply(req, 401, r#"{"error":"an access token is needed"}"#, vec![header("WWW-Authenticate", "Bearer")]);
                }
            }
            if m != Method::Post {
                return reply(req, 405, r#"{"error":"POST JSON-RPC to /mcp"}"#, vec![header("Allow", "POST")]);
            }
            let mut body = vec![];
            if req.as_reader().take(MAX_BODY).read_to_end(&mut body).is_err() {
                return reply(req, 400, r#"{"error":"couldn't read the request"}"#, vec![]);
            }
            match mcp::handle(ctx, &body) {
                (status, Some(out)) => reply(req, status, &out, vec![]),
                (status, None) => {
                    let _ = req.respond(Response::empty(status));
                }
            }
        }
        _ => reply(req, 404, r#"{"error":"not here"}"#, vec![]),
    }
}
