#![allow(dead_code)]
//! Shared harness for service tests that run against the in-memory `FakeEngine`.
use mailtriage::{
    config,
    domain::{AppConfig, EngineConfig, FilingMode, HimalayaConfig},
    engine::fake::FakeEngine,
    service::Service,
    system_service::Unit,
};
use std::{
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

pub struct Harness {
    pub dir: tempfile::TempDir,
    pub path: PathBuf,
    pub fake: FakeEngine,
}

impl Harness {
    /// `work` account with a Himalaya engine config (replaced by the fake at
    /// open), every category filed to its own name except `correspondence`,
    /// which targets `INBOX`. Review mode stays on. The fake enforces scope,
    /// so a pass that forgets `set_watch_scope` fails as it would in Himalaya.
    pub fn new(mode: FilingMode) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let toml = dir.path().join("h.toml");
        fs::write(&toml, "[accounts.work]\nimap.server='imaps://fake.test'\n").unwrap();
        let mut c = config::default_config();
        let account = c.accounts.get_mut("work").unwrap();
        account.engine = Some(EngineConfig::Himalaya(HimalayaConfig {
            binary: "himalaya".into(),
            config: toml,
            account: "work".into(),
            mailboxes: vec!["INBOX".into()],
            expected_version: "2.1.0".into(),
            timeout_seconds: 5,
            max_output_bytes: 1_000_000,
        }));
        account.filing.mode = mode;
        for category in &mut account.categories {
            category.folder = Some(if category.id == "correspondence" {
                "INBOX".into()
            } else {
                category.name.clone()
            });
        }
        config::save(&path, &c).unwrap();
        let fake = FakeEngine::new();
        fake.enforce_scope(&["INBOX"]);
        Self { dir, path, fake }
    }

    pub fn service(&self) -> Service {
        Service::open_with_engine(&self.path, Rc::new(self.fake.clone())).unwrap()
    }

    pub fn set_mode(&self, mode: FilingMode) {
        self.edit(|c| c.accounts.get_mut("work").unwrap().filing.mode = mode);
    }

    pub fn edit(&self, f: impl FnOnce(&mut AppConfig)) {
        let mut c = config::load(&self.path).unwrap();
        f(&mut c);
        config::save(&self.path, &c).unwrap();
    }

    pub fn sync(&self) -> serde_json::Value {
        self.service().sync("work", 100).unwrap()
    }
}

pub fn mail(message_id: &str, subject: &str, body: &str) -> Vec<u8> {
    format!("Message-ID: <{message_id}@test>\r\nFrom: Alex <alex@example.com>\r\nTo: work@example.com\r\nSubject: {subject}\r\nContent-Type: text/plain\r\n\r\n{body}\r\n").into_bytes()
}

/// Fake `launchctl`, with its state in files next to the script:
/// - `bootstrap` loads the plist and records its `ProgramArguments` in `args`;
///   the job runs unless `start-fails` exists, else it is loaded but idle;
/// - `bootout` unloads (exit 3 when not loaded); `kickstart` runs a loaded job;
/// - `print` describes a loaded job with tab-indented properties and its
///   `arguments` block, as the real tool does (exit 113 when not loaded,
///   1 with `print-fails`);
/// - `enable`, `disable` and `print-disabled` keep the disabled labels in
///   `disabled` (`print-disabled` exits 1 with `print-disabled-fails`).
///
/// Calls go to `launchctl.log` next to the script.
pub const LAUNCHCTL: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
echo "$*" >> "$dir/launchctl.log"
label="${2##*/}"
case "$1" in
  bootstrap)
    sed -n '/<key>ProgramArguments<\/key>/,/<\/array>/s/^ *<string>\(.*\)<\/string>$/\1/p' "$3" | sed 's/&lt;/</g; s/&gt;/>/g; s/&amp;/\&/g' > "$dir/args"
    touch "$dir/loaded"
    if [ -f "$dir/start-fails" ]; then touch "$dir/idle"; else rm -f "$dir/idle"; fi ;;
  bootout) [ -f "$dir/loaded" ] || exit 3; rm -f "$dir/loaded" "$dir/args" "$dir/idle" ;;
  kickstart) [ -f "$dir/loaded" ] || exit 113; if [ ! -f "$dir/start-fails" ]; then rm -f "$dir/idle"; fi ;;
  print)
    [ -f "$dir/print-fails" ] && exit 1
    [ -f "$dir/loaded" ] || exit 113
    printf '%s = {\n' "$2"
    if [ -f "$dir/idle" ]; then printf '\tstate = waiting\n'; else printf '\tstate = running\n\tpid = 4242\n'; fi
    printf '\tlast exit code = 0\n\targuments = {\n'
    if [ -f "$dir/args" ]; then while IFS= read -r a; do printf '\t\t%s\n' "$a"; done < "$dir/args"; fi
    printf '\t}\n\tendpoints = {\n\t\tstate = active\n\t}\n}\n' ;;
  print-disabled)
    [ -f "$dir/print-disabled-fails" ] && exit 1
    printf '\tdisabled services = {\n\t\t"com.example.other" => disabled\n'
    if [ -f "$dir/disabled" ]; then while IFS= read -r l; do printf '\t\t"%s" => disabled\n' "$l"; done < "$dir/disabled"; fi
    printf '\t}\n' ;;
  enable) if [ -f "$dir/disabled" ]; then grep -vxF "$label" "$dir/disabled" > "$dir/disabled.new"; mv "$dir/disabled.new" "$dir/disabled"; fi ;;
  disable) echo "$label" >> "$dir/disabled" ;;
  *) exit 64 ;;
esac
"#;

/// Fake `systemctl --user`, with its state in files next to the script:
/// - `daemon-reload` records the `ExecStart=` of the unit file under
///   `$MT_FAKE_HOME` (default: `home` next to the script) in `exec_start`,
///   with `%%` and `$$` undone, as the loaded definition;
/// - `enable [--now]`, `restart`, `disable [--now]` change `enabled` and
///   `active` (`start-fails` keeps a start from running);
/// - `is-enabled` prints `enabled`/`disabled`, or the text of `is-enabled`;
///   `is-active` prints `active`/`inactive`;
/// - `show` prints the requested properties; `MainPID` is `mainpid`'s text
///   or 4343, `NeedDaemonReload` is `yes` with `needs-reload`, and it exits
///   1 with `show-fails`.
///
/// Calls go to `systemctl.log` next to the script.
pub const SYSTEMCTL: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
echo "$*" >> "$dir/systemctl.log"
home="${MT_FAKE_HOME:-$dir/home}"
start() { if [ -f "$dir/start-fails" ]; then rm -f "$dir/active"; else touch "$dir/active"; fi; }
case "$2" in
  daemon-reload)
    rm -f "$dir/exec_start" "$dir/needs-reload"
    for f in "$home"/.config/systemd/user/mailtriage-*.service; do
      if [ -f "$f" ]; then sed -n 's/^ExecStart=//p' "$f" | sed 's/%%/%/g; s/\$\$/$/g' > "$dir/exec_start"; fi
    done ;;
  enable) touch "$dir/enabled"; if [ "$3" = "--now" ]; then start; fi ;;
  restart) start ;;
  disable) rm -f "$dir/enabled"; if [ "$3" = "--now" ]; then rm -f "$dir/active"; fi ;;
  is-enabled)
    if [ -f "$dir/is-enabled" ]; then cat "$dir/is-enabled"; exit 1; fi
    if [ -f "$dir/enabled" ]; then echo enabled; else echo disabled; exit 1; fi ;;
  is-active) if [ -f "$dir/active" ]; then echo active; else echo inactive; exit 3; fi ;;
  show)
    [ -f "$dir/show-fails" ] && exit 1
    if [ -f "$dir/active" ] || [ -f "$dir/exec_start" ]; then echo LoadState=loaded; else echo LoadState=not-found; fi
    if [ -f "$dir/active" ]; then
      printf 'ActiveState=active\nSubState=running\nMainPID=%s\n' "$(cat "$dir/mainpid" 2>/dev/null || echo 4343)"
    else
      printf 'ActiveState=inactive\nSubState=dead\nMainPID=0\n'
    fi
    echo ExecMainStatus=0
    if [ -f "$dir/exec_start" ]; then printf 'ExecStart={ path=x ; argv[]=%s ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }\n' "$(cat "$dir/exec_start")"; fi
    if [ -f "$dir/needs-reload" ]; then echo NeedDaemonReload=yes; else echo NeedDaemonReload=no; fi ;;
  *) exit 64 ;;
esac
"#;

/// Writes an executable script.
#[cfg(unix)]
pub fn write_tool(dir: &std::path::Path, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A config file named `name` in `dir` with the default `work` account and
/// its own state directory; returns its canonical path.
pub fn config_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let mut c = config::default_config();
    c.state_dir = dir.join(format!("{name}.state"));
    config::save(&path, &c).unwrap();
    fs::canonicalize(&path).unwrap()
}

/// The `work` account's unit for `config`, with its executable and logs in
/// `dir`.
pub fn unit_for(dir: &Path, config: &Path) -> Unit {
    Unit {
        account: "work".into(),
        exe: dir.join("mailtriage"),
        config: config.to_path_buf(),
        interval_seconds: 60,
        limit: 100,
        log_dir: dir.join("logs"),
        path_env: None,
    }
}

/// A fake `/proc/<pid>` under `root` whose command line is `unit`'s and
/// whose start time (field 22 of `stat`) is 9000.
pub fn write_proc(root: &Path, pid: u32, unit: &Unit) {
    let entry = root.join(pid.to_string());
    fs::create_dir_all(&entry).unwrap();
    let fields: Vec<String> = (4..=21).map(|n| n.to_string()).collect();
    fs::write(
        entry.join("stat"),
        format!("{pid} (mailtriage) S {} 9000 0 0\n", fields.join(" ")),
    )
    .unwrap();
    let mut cmdline = unit.arguments().join("\0").into_bytes();
    cmdline.push(0);
    fs::write(entry.join("cmdline"), cmdline).unwrap();
}
