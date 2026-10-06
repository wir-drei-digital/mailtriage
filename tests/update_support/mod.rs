#![allow(dead_code)]
//! Shared by the update tests: a loopback HTTP server that plays GitHub,
//! release fixtures, a copy of the binary under test in a temporary
//! directory, and a harness for `watch`. Nothing here touches the real
//! HOME, cache or binaries.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

/// The first page of the release list, as `Endpoint::releases_url` asks.
pub const LIST: &str = "/repos/wir-drei-digital/mailtriage/releases?per_page=30";

/// One canned answer.
#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub delay: Duration,
}

impl Reply {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: body.into(),
            delay: Duration::ZERO,
        }
    }
    pub fn status(status: u16) -> Self {
        Self {
            status,
            ..Self::ok("")
        }
    }
    pub fn redirect(location: &str) -> Self {
        Self::status(302).header("Location", location)
    }
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    pub fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// A request the server received: its target and headers (names lowercase).
#[derive(Clone, Debug)]
pub struct Request {
    pub target: String,
    pub headers: HashMap<String, String>,
}

type Hook = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct State {
    replies: HashMap<String, Reply>,
    hooks: HashMap<String, Hook>,
    log: Vec<Request>,
}

/// An HTTP/1.1 server on 127.0.0.1 with one thread per connection. Unknown
/// targets get 404.
pub struct Server {
    pub base: String,
    state: Arc<Mutex<State>>,
}

impl Server {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = Arc::clone(&shared);
                thread::spawn(move || serve(stream, &state));
            }
        });
        Self { base, state }
    }

    /// Answers `target` (path and query) with `reply`.
    pub fn reply(&self, target: &str, reply: Reply) {
        self.state
            .lock()
            .unwrap()
            .replies
            .insert(target.into(), reply);
    }

    /// Runs `hook` when `target` is requested, before answering.
    pub fn on_request(&self, target: &str, hook: impl Fn() + Send + Sync + 'static) {
        self.state
            .lock()
            .unwrap()
            .hooks
            .insert(target.into(), Arc::new(hook));
    }

    pub fn requests(&self) -> Vec<Request> {
        self.state.lock().unwrap().log.clone()
    }

    /// How many requests targeted something starting with `prefix`.
    pub fn count(&self, prefix: &str) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.target.starts_with(prefix))
            .count()
    }

    /// Waits up to 30 s until `count` requests targeted `prefix`.
    pub fn wait_for(&self, prefix: &str, count: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while self.count(prefix) < count {
            assert!(
                std::time::Instant::now() < deadline,
                "no request for {prefix}; got {:?}",
                self.requests()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn url(&self, target: &str) -> String {
        format!("{}{target}", self.base)
    }

    /// Serves `releases` as the one-page release list.
    pub fn list(&self, releases: &[Value]) {
        self.reply(LIST, Reply::ok(Value::from(releases.to_vec()).to_string()));
    }

    /// Publishes stable `version` with `archive` as this host's archive and
    /// a matching `SHA256SUMS`, as the only release. Returns the archive name.
    pub fn publish(&self, version: &str, archive: &[u8]) -> String {
        let name = format!("mailtriage-v{version}-{}.tar.gz", platform());
        let sums = format!("{}  {name}\n", sha256_hex(archive));
        self.publish_with_sums(version, archive, &sums);
        name
    }

    /// `publish` with the given `SHA256SUMS` text.
    pub fn publish_with_sums(&self, version: &str, archive: &[u8], sums: &str) {
        let name = format!("mailtriage-v{version}-{}.tar.gz", platform());
        self.reply(&download_path(version, &name), Reply::ok(archive));
        self.reply(&download_path(version, "SHA256SUMS"), Reply::ok(sums));
        self.list(&[self.release_json(
            version,
            &[(&name, archive.len()), ("SHA256SUMS", sums.len())],
        )]);
    }

    /// One entry of the release list, its assets served under `/download/`.
    pub fn release_json(&self, version: &str, assets: &[(&str, usize)]) -> Value {
        json!({
            "tag_name": format!("v{version}"),
            "draft": false,
            "prerelease": false,
            "html_url": format!("https://github.com/wir-drei-digital/mailtriage/releases/tag/v{version}"),
            "published_at": "2026-11-02T09:00:00Z",
            "assets": assets.iter().map(|(name, size)| json!({
                "name": name,
                "size": size,
                "browser_download_url": self.url(&download_path(version, name)),
            })).collect::<Vec<_>>(),
        })
    }
}

pub fn download_path(version: &str, name: &str) -> String {
    format!("/download/v{version}/{name}")
}

fn serve(stream: TcpStream, state: &Mutex<State>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let target = line.split_whitespace().nth(1).unwrap_or("").to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
        if let Some((name, value)) = header.trim_end().split_once(':') {
            headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let (reply, hook) = {
        let mut state = state.lock().unwrap();
        state.log.push(Request {
            target: target.clone(),
            headers,
        });
        (
            state
                .replies
                .get(&target)
                .cloned()
                .unwrap_or(Reply::status(404)),
            state.hooks.get(&target).cloned(),
        )
    };
    if let Some(hook) = hook {
        hook();
    }
    thread::sleep(reply.delay);
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let mut out = stream;
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(&reply.body);
    let _ = out.flush();
}

/// This host's platform name in archive names.
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos-arm64"
    } else if cfg!(target_arch = "aarch64") {
        "linux-arm64"
    } else {
        "linux-amd64"
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

use std::path::Path;

/// A gzip tar archive of regular files with mode 0755.
pub fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (name, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, name, *data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

/// A release archive as the release workflow builds it.
pub fn release_archive(binary: &[u8]) -> Vec<u8> {
    archive(&[
        ("mailtriage", binary),
        ("LICENSE", b"MIT License\n"),
        ("README.md", b"# mailtriage\n"),
    ])
}

/// The fake release binary: `--version` prints `mailtriage VERSION`; any
/// other call appends its arguments to `marker`.
pub fn fake_binary(version: &str, marker: &Path) -> Vec<u8> {
    format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'mailtriage {version}'; exit 0; fi\necho \"$@\" >> '{}'\n",
        marker.display()
    )
    .into_bytes()
}

/// Writes `data` to `path` with `mode` through a new file renamed over
/// it, so `path` gets a new inode.
pub fn replace_file(path: &Path, data: &[u8], mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let tmp = path.with_extension("replacing");
    std::fs::write(&tmp, data).unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

use std::path::PathBuf;

/// A private temporary directory with an `install/` directory (0755)
/// holding a copy of the binary under test, and `home/` and `xdg/` for
/// `HOME` and `XDG_CACHE_HOME`.
pub struct Sandbox {
    pub dir: tempfile::TempDir,
    pub bin: PathBuf,
    pub home: PathBuf,
    pub xdg: PathBuf,
}

impl Sandbox {
    pub fn new() -> Self {
        Self::at("install")
    }

    /// The copy at `<tmp>/<relative>/mailtriage`.
    pub fn at(relative: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let install = root.join(relative);
        std::fs::create_dir_all(&install).unwrap();
        let mut walk = install.clone();
        while walk != root {
            std::fs::set_permissions(&walk, std::fs::Permissions::from_mode(0o755)).unwrap();
            walk.pop();
        }
        let bin = install.join("mailtriage");
        std::fs::copy(env!("CARGO_BIN_EXE_mailtriage"), &bin).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let home = root.join("home");
        let xdg = root.join("xdg");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&xdg).unwrap();
        Self {
            dir,
            bin,
            home,
            xdg,
        }
    }

    pub fn root(&self) -> PathBuf {
        std::fs::canonicalize(self.dir.path()).unwrap()
    }

    /// The copy run with this sandbox's HOME and XDG_CACHE_HOME, the
    /// server as GitHub, and no inherited config or test hook. Neither
    /// `update` nor the tests that use this run `launchctl` or `systemctl`.
    pub fn command(&self, server: &Server) -> std::process::Command {
        let mut command = std::process::Command::new(&self.bin);
        command
            .current_dir(self.root())
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.xdg)
            .env("MAILTRIAGE_UPDATE_URL", &server.base)
            .env("PATH", "/usr/bin:/bin")
            .env_remove("MAILTRIAGE_CONFIG")
            .env_remove("MAILTRIAGE_UPDATE_TEST_HOOK")
            .env_remove("MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS");
        command
    }

    /// `update.json` in this sandbox's cache directory.
    pub fn cache_file(&self) -> PathBuf {
        cache_dir(&self.home, &self.xdg).join("update.json")
    }

    pub fn cache(&self) -> Value {
        std::fs::read(self.cache_file())
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or(Value::Null)
    }
}

/// The cache directory mailtriage uses for these HOME and XDG_CACHE_HOME.
pub fn cache_dir(home: &Path, xdg: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Caches/mailtriage")
    } else {
        xdg.join("mailtriage")
    }
}

/// Runs `command`; returns its exit code, its stdout as JSON (`Null` when
/// it is not JSON) and its stderr.
pub fn run(command: &mut std::process::Command) -> (Option<i32>, Value, String) {
    let out = command.output().unwrap();
    (
        out.status.code(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}
