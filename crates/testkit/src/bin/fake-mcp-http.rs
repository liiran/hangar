//! A Streamable-HTTP MCP server for tests, with per-session state.
//!
//! `fake-mcp-http --port N` serves `POST/GET/DELETE /mcp`. Every HTTP session
//! (`mcp-session-id`) has its own counter, the way Playwright MCP gives every
//! session its own browser context. Tools: `pid`, `counter`, `slow` (answers
//! as an SSE stream with progress), `crash`, `stats`.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct State {
    sessions: HashMap<String, u64>,
    deleted: Vec<String>,
    init_calls: u64,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port: u16 = args.iter().position(|a| a == "--port").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()).expect("--port N");
    // `--fail-once <file>`: ends at once the first time, like a server that found its port taken.
    if let Some(marker) = args.iter().position(|a| a == "--fail-once").and_then(|i| args.get(i + 1))
        && !std::path::Path::new(marker).exists()
    {
        std::fs::write(marker, "").unwrap();
        eprintln!("address already in use");
        std::process::exit(1);
    }
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind");
    let state = Arc::new(Mutex::new(State::default()));
    let next = Arc::new(AtomicU64::new(1));
    for stream in listener.incoming().flatten() {
        let state = state.clone();
        let next = next.clone();
        std::thread::spawn(move || {
            let _ = serve(stream, &state, &next);
        });
    }
}

struct Request {
    method: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &TcpStream) -> std::io::Result<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let method = line.split_whitespace().next().unwrap_or_default().to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h)?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    Ok(Request { method, headers, body })
}

fn respond(mut s: &TcpStream, status: &str, headers: &[(&str, String)], body: &str) -> std::io::Result<()> {
    let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(body);
    s.write_all(out.as_bytes())
}

fn json_reply(s: &TcpStream, v: &Value, extra: &[(&str, String)]) -> std::io::Result<()> {
    let mut headers = vec![("Content-Type", "application/json".to_string())];
    headers.extend(extra.iter().cloned());
    respond(s, "200 OK", &headers, &v.to_string())
}

fn text(id: &Value, t: String) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": t}]}})
}

fn serve(stream: TcpStream, state: &Mutex<State>, next: &AtomicU64) -> std::io::Result<()> {
    let req = read_request(&stream)?;
    let sid = req.headers.get("mcp-session-id").cloned();
    match req.method.as_str() {
        "GET" => return respond(&stream, "405 Method Not Allowed", &[], ""),
        "DELETE" => {
            let mut st = state.lock().unwrap();
            if let Some(sid) = sid {
                st.sessions.remove(&sid);
                st.deleted.push(sid);
            }
            return respond(&stream, "200 OK", &[], "");
        }
        "POST" => {}
        _ => return respond(&stream, "405 Method Not Allowed", &[], ""),
    }
    let msg: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
    let id = msg.get("id").cloned().filter(|v| !v.is_null());
    if method == "initialize" {
        let new_sid = format!("sess-{}", next.fetch_add(1, Ordering::SeqCst));
        {
            let mut st = state.lock().unwrap();
            st.sessions.insert(new_sid.clone(), 0);
            st.init_calls += 1;
        }
        let result = json!({"jsonrpc": "2.0", "id": id, "result": {
            "protocolVersion": msg.pointer("/params/protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
            "capabilities": {"tools": {"listChanged": true}},
            "serverInfo": {"name": "fake-mcp-http", "version": "1.0.0"}
        }});
        return json_reply(&stream, &result, &[("mcp-session-id", new_sid)]);
    }
    let known = sid.as_ref().is_some_and(|s| state.lock().unwrap().sessions.contains_key(s));
    if !known {
        return respond(&stream, "404 Not Found", &[], "");
    }
    let sid = sid.unwrap();
    let Some(id) = id else {
        return respond(&stream, "202 Accepted", &[], ""); // notification or response
    };
    match method.as_str() {
        "tools/list" => {
            let t = |n: &str| json!({"name": n, "inputSchema": {"type": "object"}});
            json_reply(&stream, &json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [t("pid"), t("counter"), t("slow"), t("crash"), t("stats")]}}), &[])
        }
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or_default();
            match name {
                "pid" => json_reply(&stream, &text(&id, std::process::id().to_string()), &[]),
                "counter" => {
                    let n = {
                        let mut st = state.lock().unwrap();
                        let c = st.sessions.get_mut(&sid).unwrap();
                        *c += 1;
                        *c
                    };
                    json_reply(&stream, &text(&id, n.to_string()), &[])
                }
                "slow" => {
                    // Stream progress and the result as server-sent events.
                    let token = msg.pointer("/params/_meta/progressToken").cloned();
                    let mut s = &stream;
                    s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")?;
                    for i in 1..=3 {
                        std::thread::sleep(Duration::from_millis(60));
                        if let Some(t) = &token {
                            let n =
                                json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progressToken": t, "progress": i, "message": sid}});
                            s.write_all(format!("event: message\ndata: {n}\n\n").as_bytes())?;
                        }
                    }
                    s.write_all(format!("event: message\ndata: {}\n\n", text(&id, format!("done {sid}"))).as_bytes())?;
                    Ok(())
                }
                "crash" => std::process::exit(3),
                "stats" => {
                    let st = state.lock().unwrap();
                    let v = json!({"pid": std::process::id(), "sessions": st.sessions.len(), "deleted": st.deleted, "init_calls": st.init_calls});
                    drop(st);
                    json_reply(&stream, &text(&id, v.to_string()), &[])
                }
                _ => json_reply(&stream, &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "unknown tool"}}), &[]),
            }
        }
        "ping" => json_reply(&stream, &json!({"jsonrpc": "2.0", "id": id, "result": {}}), &[]),
        _ => json_reply(&stream, &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}}), &[]),
    }
}
