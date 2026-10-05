#![allow(dead_code)]
//! Shared harness for service tests that run against the in-memory `FakeEngine`.
use mailtriage::{
    config,
    domain::{AppConfig, EngineConfig, FilingMode, HimalayaConfig},
    engine::fake::FakeEngine,
    service::Service,
};
use std::{fs, path::PathBuf, rc::Rc};

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

/// Fake `launchctl`: `bootstrap` loads, `bootout` unloads (exit 3 when not
/// loaded), `print` describes a loaded job with tab-indented properties, as
/// the real tool does. Calls go to `launchctl.log` next to the script.
pub const LAUNCHCTL: &str = "#!/bin/sh\ndir=\"$(dirname \"$0\")\"\necho \"$*\" >> \"$dir/launchctl.log\"\ncase \"$1\" in\n  bootstrap) touch \"$dir/loaded\" ;;\n  bootout) [ -f \"$dir/loaded\" ] || exit 3; rm -f \"$dir/loaded\" ;;\n  print) [ -f \"$dir/loaded\" ] || exit 113; printf '%s = {\\n\\tstate = running\\n\\tpid = 4242\\n\\tlast exit code = 0\\n\\tendpoints = {\\n\\t\\tstate = active\\n\\t}\\n}\\n' \"$2\" ;;\n  *) exit 64 ;;\nesac\n";

/// Fake `systemctl --user`: `enable`, `restart`, `disable --now`, `show`.
pub const SYSTEMCTL: &str = "#!/bin/sh\ndir=\"$(dirname \"$0\")\"\necho \"$*\" >> \"$dir/systemctl.log\"\ncase \"$2\" in\n  daemon-reload) ;;\n  enable) touch \"$dir/enabled\" ;;\n  restart) touch \"$dir/active\" ;;\n  disable) rm -f \"$dir/enabled\" \"$dir/active\" ;;\n  show) if [ -f \"$dir/active\" ]; then printf 'LoadState=loaded\\nActiveState=active\\nSubState=running\\nMainPID=4343\\nExecMainStatus=0\\n'; else printf 'LoadState=not-found\\nActiveState=inactive\\nSubState=dead\\nMainPID=0\\nExecMainStatus=0\\n'; fi ;;\n  *) exit 64 ;;\nesac\n";

/// Writes an executable script.
#[cfg(unix)]
pub fn write_tool(dir: &std::path::Path, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}
