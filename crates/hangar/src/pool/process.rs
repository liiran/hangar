//! Starting a server process and draining its output. Shared by stdio and
//! multi-session HTTP upstreams.

use crate::launch::Launch;
use crate::procs::{self, Job};
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::mpsc;

/// Last lines a server wrote to stderr, for `hangar status` and errors.
pub type Ring = Arc<Mutex<VecDeque<String>>>;

const RING_LINES: usize = 200;

pub fn push(ring: &Ring, line: String) {
    let mut r = ring.lock().unwrap();
    if r.len() >= RING_LINES {
        r.pop_front();
    }
    r.push_back(line);
}

pub fn tail(ring: &Ring, n: usize) -> Vec<String> {
    let r = ring.lock().unwrap();
    r.iter().skip(r.len().saturating_sub(n)).cloned().collect()
}

pub struct Started {
    pub child: Child,
    pub pid: u32,
    /// Kill-on-close job holding the server and all its descendants.
    pub job: Option<Job>,
    /// Lines for the server's stdin; dropping it closes stdin.
    pub stdin: Option<mpsc::UnboundedSender<String>>,
    pub stdout: Option<ChildStdout>,
}

pub struct Options<'a> {
    pub launch: &'a Launch,
    pub extra_args: &'a [String],
    pub cwd: &'a Path,
    /// The client's environment; empty = inherit ours.
    pub env: &'a BTreeMap<String, String>,
    /// Variables declared in the client config, applied on top.
    pub declared: &'a BTreeMap<String, String>,
    pub stdin: bool,
    pub stdout: bool,
}

/// Starts the server. stderr (and stdout when not requested) is drained into `ring`.
pub fn start(o: Options<'_>, ring: &Ring) -> std::io::Result<Started> {
    let mut cmd = Command::new(&o.launch.program);
    cmd.args(&o.launch.args).args(o.extra_args);
    if o.cwd.is_dir() {
        cmd.current_dir(o.cwd);
    }
    if !o.env.is_empty() {
        cmd.env_clear().envs(o.env);
    }
    cmd.envs(o.declared);
    let piped = || std::process::Stdio::piped();
    cmd.stdin(if o.stdin { piped() } else { std::process::Stdio::null() }).stdout(piped()).stderr(piped()).kill_on_drop(true);
    procs::hide_window(&mut cmd);
    let mut child = cmd.spawn()?;
    let pid = child.id().unwrap_or(0);
    let job = procs::contain(&child);
    if let Some(err) = child.stderr.take() {
        drain(err, ring.clone(), "");
    }
    let stdout = child.stdout.take();
    let stdout = if o.stdout {
        stdout
    } else {
        if let Some(out) = stdout {
            drain(out, ring.clone(), "");
        }
        None
    };
    let stdin = child.stdin.take().map(|mut pipe| {
        // A writer task: the actor never blocks on a server that stops reading.
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if pipe.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
            let _ = pipe.shutdown().await;
        });
        tx
    });
    Ok(Started { child, pid, job, stdin, stdout })
}

/// Copies lines into the ring. Invalid UTF-8 is replaced, never fatal, so
/// the pipe keeps draining and the server never blocks on a full pipe.
pub fn drain<R: AsyncRead + Unpin + Send + 'static>(stream: R, ring: Ring, prefix: &'static str) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                    push(&ring, format!("{prefix}{line}"));
                }
            }
        }
    });
}

/// Stops a server: close stdin, give it `grace` to exit, then kill its tree.
pub async fn stop(mut s: Started, grace: std::time::Duration) {
    drop(s.stdin.take());
    if tokio::time::timeout(grace, s.child.wait()).await.is_err() {
        let _ = s.child.start_kill();
    }
    if let Some(job) = s.job.take() {
        job.terminate();
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), s.child.wait()).await;
}
