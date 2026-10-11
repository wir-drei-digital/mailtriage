#![allow(dead_code)]
//! Kev without Python, uv, a network or the model: a stand-in Kev server,
//! an in-process `Stub`, and stand-in tool scripts. The stand-in `uv run`
//! execs the test binary itself, whose ignored test `kev_standin_server`
//! calls `serve_from_env`; each test file that uses `uv_script` declares
//! that test.
use serde_json::{json, Map, Value};
use std::{
    fs,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

/// A unique fixture port verified free right now. Keep managed server ports
/// below the macOS/Linux ephemeral client range, so setup's many outgoing
/// probes cannot reuse a just-released autoassigned listener port. Allocation
/// never repeats within this test binary, including concurrent fixtures.
pub fn free_port() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(16_000);
    loop {
        let port = NEXT.fetch_add(1, Ordering::Relaxed);
        assert!(port < 32_000, "fixture port range exhausted");
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}

/// Polls `done` every 100 ms until it is true or `timeout` passes.
pub fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    done()
}

/// What Kev's `GET /v1/models` lists for `--run run`.
pub fn models_for(run: &str) -> Value {
    json!({"models": [
        {"name": "kev-latest", "run": run, "device": "stand-in"},
        {"name": "jev-latest", "run": run, "device": "stand-in"}
    ]})
}

/// A valid System One answer for `request`'s questions: the first label
/// (in the sorted order the request has) at 0.9, the rest sharing 0.1,
/// four decimals, as Kev rounds.
pub fn answer(request: &Value) -> Value {
    let mut answers = Map::new();
    for (id, question) in request["questions"].as_object().into_iter().flatten() {
        if question["type"] == "noul" {
            answers.insert(id.clone(), json!({"type": "noul", "noul": 0.8123}));
            continue;
        }
        let labels: Vec<&String> = question["criteria"]
            .as_object()
            .map(|c| c.keys().collect())
            .unwrap_or_default();
        let Some(&chosen) = labels.first() else {
            continue;
        };
        let others = labels.len().saturating_sub(1).max(1) as f64;
        let rest = (0.1 / others * 10_000.0).round() / 10_000.0;
        let top = if labels.len() == 1 { 1.0 } else { 0.9 };
        let probabilities: Map<String, Value> = labels
            .iter()
            .map(|&l| (l.clone(), json!(if l == chosen { top } else { rest })))
            .collect();
        answers.insert(
            id.clone(),
            json!({"type": "choice", "choice": chosen, "confidence": 0.8667, "probabilities": probabilities}),
        );
    }
    json!({"model": request["model"], "answers": answers,
           "usage": {"input_tokens": 100, "output_tokens": 20}, "latency_ms": 5.0})
}

/// Reads one request: its head and body.
fn read_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 8192];
    let head_end = loop {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buf[..n]);
        if let Some(p) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
    };
    let head = String::from_utf8_lossy(&bytes[..head_end]).into_owned();
    let length: usize = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|v| v.trim().parse().ok())
        })
        .unwrap_or(0);
    while bytes.len() < head_end + length {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buf[..n]);
    }
    Some((head, bytes[head_end..head_end + length].to_vec()))
}

/// Answers one connection: `GET .../v1/models` with `models`,
/// `POST .../v1/systemone` with `answer` (or `systemone_status` and a body
/// that must never reach an error). Returns `"METHOD PATH auth=yes|no"`.
fn respond(stream: TcpStream, models: &Value, systemone_status: Option<u16>) -> Option<String> {
    respond_with_hook(stream, models, systemone_status, &|| {})
}

fn respond_with_hook(
    mut stream: TcpStream,
    models: &Value,
    systemone_status: Option<u16>,
    hook: &dyn Fn(),
) -> Option<String> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let (head, body) = read_request(&mut stream)?;
    let mut first = head.lines().next()?.split(' ');
    let (method, path) = (first.next()?.to_owned(), first.next()?.to_owned());
    let auth = head
        .to_ascii_lowercase()
        .contains("\r\nauthorization: bearer ");
    let (status, reply) = if method == "GET" && path.ends_with("/v1/models") {
        (200, models.clone())
    } else if method == "POST" && path.ends_with("/v1/systemone") {
        hook();
        match systemone_status {
            Some(code) => (code, json!({"detail": "stub secret diagnostic"})),
            None => (200, answer(&serde_json::from_slice(&body).ok()?)),
        }
    } else {
        (404, json!({"detail": "not found"}))
    };
    let text = reply.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} Stand-in\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    Some(format!(
        "{method} {path} auth={}",
        if auth { "yes" } else { "no" }
    ))
}

/// The stand-in Kev server, run as the test binary's ignored test
/// `kev_standin_server` by the stand-in `uv run`. It reads
/// `KEV_STANDIN_DIR` (the control directory), `KEV_STANDIN_PORT` and
/// `KEV_STANDIN_RUN`; without them (a plain `cargo test -- --ignored`) it
/// returns at once. In the control directory, `exit-now` makes it exit 3
/// before listening and `delay` (seconds) makes it wait before listening.
/// Requests go to `requests.log`. Orphaned, it ends after 120 s without a
/// request.
pub fn serve_from_env() {
    let (Some(control), Ok(port), Ok(run)) = (
        std::env::var_os("KEV_STANDIN_DIR"),
        std::env::var("KEV_STANDIN_PORT"),
        std::env::var("KEV_STANDIN_RUN"),
    ) else {
        return;
    };
    let control = PathBuf::from(control);
    if control.join("exit-now").exists() {
        std::process::exit(3);
    }
    if let Ok(delay) = fs::read_to_string(control.join("delay")) {
        thread::sleep(Duration::from_secs(delay.trim().parse().unwrap_or(0)));
    }
    let port: u16 = port.parse().expect("KEV_STANDIN_PORT is a port");
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("the stand-in Kev port is free");
    listener.set_nonblocking(true).unwrap();
    let models = models_for(&run);
    let log = control.join("requests.log");
    let mut last = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                last = Instant::now();
                if let Some(line) = respond_with_hook(stream, &models, None, &|| {
                    if control.join("kill-supervisor-on-post").exists() {
                        let owner = fs::read_to_string(control.join("supervisor.pid")).unwrap();
                        assert!(fixture_signal(owner.trim().parse().unwrap(), 9));
                    }
                }) {
                    let mut file = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log)
                        .unwrap();
                    let _ = writeln!(file, "{line}");
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if last.elapsed() > Duration::from_secs(120) {
                    std::process::exit(0);
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => std::process::exit(1),
        }
    }
}

/// The stand-in `uv`. Each call appends
/// `ARGS|cwd=PATH|HF_HOME=..|HF_HUB_OFFLINE=..|UV_CACHE_DIR=..|UV_PYTHON_INSTALL_DIR=..`
/// to `control/uv.log` (`HF_HUB_OFFLINE=unset` when it is not set). `sync`
/// makes `.venv` (fails with `control/uv-sync-fails`); `run` execs the
/// stand-in Kev server in `exe`.
pub fn uv_script(control: &Path, exe: &Path) -> String {
    format!(
        r#"#!/bin/sh
control='{control}'
printf '%s|cwd=%s|HF_HOME=%s|HF_HUB_OFFLINE=%s|UV_OFFLINE=%s|KEV_API_KEY=%s|UV_CACHE_DIR=%s|UV_PYTHON_INSTALL_DIR=%s\n' "$*" "$(pwd -P)" "${{HF_HOME-}}" "${{HF_HUB_OFFLINE-unset}}" "${{UV_OFFLINE-unset}}" "$(if [ "${{KEV_API_KEY+x}}" = x ]; then printf set; else printf unset; fi)" "${{UV_CACHE_DIR-}}" "${{UV_PYTHON_INSTALL_DIR-}}" >> "$control/uv.log"
case "$1" in
  sync)
    printf '%s\n' "$$" > "$control/sync-writer.pid"
    printf '%s\n' "$PPID" > "$control/sync-owner.pid"
    if [ -f "$control/uv-sync-fails" ]; then echo 'stand-in uv: sync failed' >&2; exit 1; fi
    if [ -f "$control/hold-sync-descendant" ]; then
      (
        exec 3>&- 4>&- 5>&- 6>&- 7>&- 8>&- 9>&-
        trap '' TERM
        touch "$control/descendant-entered"
        while [ -f "$control/hold-sync-descendant" ]; do sleep 0.1; done
        mkdir -p .venv
        touch "$control/descendant-finished"
      ) &
    fi
    touch "$control/sync-entered"
    if [ -f "$control/kill-sync-owner" ]; then kill -KILL "$PPID"; fi
    while [ -f "$control/hold-sync" ]; do sleep 0.1; done
    touch "$control/sync-leader-ended"
    if [ -f "$control/hold-sync-descendant" ]; then exit 0; fi
    mkdir -p .venv
    touch "$control/sync-finished" ;;
  run)
    mkdir -p "$HF_HOME"
    printf '%s\n' "$PPID" > "$control/supervisor.pid"
    printf '%s\n' "$$" > "$control/child.pid"
    port=; run=
    while [ $# -gt 0 ]; do
      case "$1" in
        --port) port="$2"; shift ;;
        --run) run="$2"; shift ;;
      esac
      shift
    done
    KEV_STANDIN_DIR="$control" KEV_STANDIN_PORT="$port" KEV_STANDIN_RUN="$run" exec '{exe}' kev_standin_server --exact --ignored --nocapture --test-threads=1 ;;
  *) exit 64 ;;
esac
"#,
        control = control.display(),
        exe = exe.display()
    )
}

/// An in-process Kev server on 127.0.0.1 for tests that need no managed
/// runtime. It runs until the test process ends.
pub struct Stub {
    pub port: u16,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    /// Serves `models` at `GET .../v1/models`; `POST .../v1/systemone`
    /// answers each question, or replies `systemone_status`.
    pub fn start(models: Value, systemone_status: Option<u16>) -> Stub {
        Self::start_with_hook(models, systemone_status, || {})
    }

    pub fn start_with_hook(
        models: Value,
        systemone_status: Option<u16>,
        hook: impl Fn() + Send + 'static,
    ) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Some(line) = respond_with_hook(stream, &models, systemone_status, &hook) {
                    seen.lock().unwrap().push(line);
                }
            }
        });
        Stub { port, requests }
    }

    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/v1/systemone", self.port)
    }

    /// `"METHOD PATH auth=yes|no"` per request so far.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// The stand-in `git`: calls go to `control/git.log`. `clone ... --branch
/// TAG URL DEST` makes DEST with the tag in `.standin-tag` (exit 128 with
/// `control/git-clone-fails`); `-C DIR describe --tags --exact-match`
/// prints it.
pub fn git_script(control: &Path) -> String {
    format!(
        r#"#!/bin/sh
control='{control}'
echo "$*" >> "$control/git.log"
case "$1" in
  clone)
    if [ -f "$control/git-clone-fails" ]; then exit 128; fi
    branch=; dest=; prev=
    for a in "$@"; do
      if [ "$prev" = --branch ]; then branch="$a"; fi
      prev="$a"; dest="$a"
    done
    touch "$control/clone-entered"
    while [ -f "$control/hold-clone" ]; do sleep 0.1; done
    mkdir -p "$dest" && printf '%s\n' "$branch" > "$dest/.standin-tag"
    touch "$control/clone-finished" ;;
  -C)
    if [ "$3" = describe ] && [ -f "$2/.standin-tag" ]; then cat "$2/.standin-tag"; else exit 128; fi ;;
  *) exit 64 ;;
esac
"#,
        control = control.display()
    )
}

/// The stand-in `brew`: `install uv` copies `uv.standin`, next to the
/// script, to `uv`. Calls go to `control/brew.log`.
pub fn brew_script(control: &Path) -> String {
    format!(
        r#"#!/bin/sh
here="$(dirname "$0")"
echo "$*" >> '{control}/brew.log'
[ "$1 $2" = "install uv" ] || exit 64
cp "$here/uv.standin" "$here/uv" && chmod 755 "$here/uv"
"#,
        control = control.display()
    )
}
/// Signals only fresh process identities owned by the current test fixture.
#[cfg(unix)]
pub fn fixture_signal(pid: i32, signal: i32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // SAFETY: the caller supplies the fixture identity and POSIX signal.
    unsafe { kill(pid, signal) == 0 }
}
