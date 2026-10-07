//! The update work inside `watch`: before each pass the restart check,
//! then the update step (refresh, install or notify); while waiting
//! between passes, the restart check every 5 s. Update errors are printed
//! as events and never fail or end a pass.
use super::{
    cache::{install_key, Cache, ConfigEntry, ErrorRecord},
    check::{self, Reservation},
    events,
    github::{Endpoint, Net},
    install::{self, EnvHooks, Hooks, Job, Outcome},
    platform::{self, Blocker, FileIdentity},
    release::{self, CachedRelease},
    restart::{Image, Restarter},
    schedule, version, CLI,
};
use crate::{config, domain::UpdateMode};
use anyhow::anyhow;
use chrono::{Duration as Age, Utc};
use semver::Version;
use serde_json::{json, Map, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

/// The start of every message about a cache that cannot be written; the
/// first such message is printed, later ones are not.
const CACHE_UNWRITABLE: &str = "cannot write the update cache";
/// `auto` installs only from release information at most this old.
const MAX_RELEASE_AGE_HOURS: i64 = 48;

pub struct WatchUpdates {
    restarter: Restarter,
    emit: Rc<dyn Fn(&Value)>,
    hooks: Box<dyn Hooks>,
    /// `None` without `HOME`: no update step.
    cache: Option<Cache>,
    endpoint: Endpoint,
    /// A cache problem was printed.
    cache_reported: bool,
    /// The (config key, version) of every `available` event printed, so a
    /// cache that cannot store `notified_version` does not repeat one.
    notified: HashSet<(String, String)>,
}

impl WatchUpdates {
    /// Records the installation path and image first, as `watch` must
    /// before anything else.
    pub fn start(json_mode: bool) -> Self {
        let image = Image::record();
        let emit: Rc<dyn Fn(&Value)> = Rc::new(move |event| events::emit(json_mode, event));
        Self::with(
            image,
            emit,
            Box::new(EnvHooks),
            Cache::for_user(),
            Endpoint::from_env(),
        )
    }

    /// The update work from the given parts, for embedding: `start` passes
    /// the real ones.
    pub fn with(
        image: Result<Image, String>,
        emit: Rc<dyn Fn(&Value)>,
        hooks: Box<dyn Hooks>,
        cache: Option<Cache>,
        endpoint: Endpoint,
    ) -> Self {
        let restart_emit = Rc::clone(&emit);
        Self {
            restarter: Restarter::new(image, Box::new(move |event| restart_emit(event))),
            emit,
            hooks,
            cache,
            endpoint,
            cache_reported: false,
            notified: HashSet::new(),
        }
    }

    /// The restart check, the update step for the config at `config`, and
    /// the restart check again after an install.
    pub fn before_pass(&mut self, config: &Path, stopped: &dyn Fn() -> bool) {
        self.restarter.check(stopped, &*self.hooks);
        if self.update_step(config) {
            self.restarter.check(stopped, &*self.hooks);
        }
    }

    /// The restart check alone, for the wait between passes.
    pub fn while_waiting(&mut self, stopped: &dyn Fn() -> bool) {
        self.restarter.check(stopped, &*self.hooks);
    }

    /// Returns whether a release was installed.
    fn update_step(&mut self, config: &Path) -> bool {
        let Some(cache) = self.cache.clone() else {
            return false;
        };
        let key = config_key(config);
        let mode = match config::read_updates_mode(config) {
            Ok(mode) => {
                self.store_mode(&cache, &key, mode);
                mode
            }
            // Only this config's own stored mode, never another config's.
            Err(_) => match cache.read().configs.get(&key) {
                Some(entry) => entry.mode,
                None => return false,
            },
        };
        if mode == UpdateMode::Off {
            return false;
        }
        if schedule::due(cache.read().next_check_at.as_deref(), Utc::now()) {
            let refreshed = Net::new(self.endpoint.clone())
                .and_then(|net| check::refresh(&net, &cache, Reservation::Required));
            match refreshed {
                // A result that could not be recorded: a cache problem.
                Ok(checked) => {
                    for warning in checked.warnings {
                        self.report(warning);
                    }
                }
                Err(e) => self.report(e.to_string()),
            }
        }
        let file = cache.read();
        let Some(release) = file.release.clone() else {
            return false;
        };
        let Ok(candidate) = Version::parse(&release.version) else {
            return false;
        };
        let path = self.restarter.image().map(|image| image.path.clone());
        let installed = path
            .as_deref()
            .and_then(|p| self.installed_version(&cache, p));
        if !version::is_newer(&candidate, &installed.unwrap_or_else(version::running)) {
            return false;
        }
        let blocker = path
            .as_deref()
            .and_then(|p| platform::blocker(p, release::platform()));
        if let (UpdateMode::Auto, Some(path), None) = (mode, &path, blocker) {
            let now = Utc::now();
            let fresh = file
                .checked_at
                .as_deref()
                .and_then(schedule::parse)
                .is_some_and(|at| now - at <= Age::hours(MAX_RELEASE_AGE_HOURS));
            let attempt_due = schedule::due(
                file.installs
                    .get(&install_key(path))
                    .and_then(|e| e.next_attempt_at.as_deref()),
                now,
            );
            return fresh && attempt_due && self.install(&cache, path, &release);
        }
        // Only `auto` says why it does not install; `notify` never would.
        let not_replaceable = path
            .as_deref()
            .zip(blocker.filter(|_| mode == UpdateMode::Auto));
        self.notify(&cache, &key, mode, &release, not_replaceable);
        false
    }

    /// Stores `mode` for the config `key` when it changed.
    fn store_mode(&mut self, cache: &Cache, key: &str, mode: UpdateMode) {
        if cache.read().configs.get(key).map(|e| e.mode) == Some(mode) {
            return;
        }
        let stored = cache.update(|c| {
            c.configs
                .entry(key.to_owned())
                .and_modify(|e| e.mode = mode)
                .or_insert(ConfigEntry {
                    mode,
                    notified_version: None,
                    extra: Map::new(),
                });
        });
        if let Err(e) = stored {
            self.report(format!("{CACHE_UNWRITABLE}: {e:#}"));
        }
    }

    /// Steps 2 to 10 for the binary at `path`, trying the installation lock
    /// once. Returns whether the release was installed.
    fn install(&mut self, cache: &Cache, path: &Path, release: &CachedRelease) -> bool {
        let key = install_key(path);
        let Some(dir) = path.parent() else {
            return false;
        };
        let lock = match install::lock(dir, Duration::ZERO) {
            Ok(Some(lock)) => lock,
            // Another updater is at work: try again at the next pass.
            Ok(None) => return false,
            Err(e) => {
                self.install_failed(cache, &key, &format!("{e:#}"));
                return false;
            }
        };
        let net = match Net::new(self.endpoint.clone()) {
            Ok(net) => net,
            Err(e) => {
                self.install_failed(cache, &key, &e.to_string());
                return false;
            }
        };
        let running = version::running();
        let job = Job {
            net: &net,
            component: CLI,
            path,
            release,
            fallback: &running,
            cache: Some(cache),
            hooks: &*self.hooks,
        };
        let mut reserve = || {
            cache
                .update(|c| {
                    c.installs.entry(key.clone()).or_default().next_attempt_at =
                        Some(schedule::stamp(schedule::reservation(Utc::now())));
                })
                .map_err(|e| anyhow!("{CACHE_UNWRITABLE}: {e:#}; not installing"))
        };
        let outcome = install::install(&job, &lock, &mut reserve);
        drop(lock);
        match outcome {
            Ok(Outcome::Installed(done)) => {
                for warning in done.warnings {
                    self.report(warning);
                }
                true
            }
            Ok(Outcome::Current { .. }) => false,
            Err(e) => {
                let message = format!("{e:#}");
                // An unwritable reservation is a cache problem, printed
                // once, not an install failure with a backoff to record.
                if message.starts_with(CACHE_UNWRITABLE) {
                    self.report(message);
                } else {
                    self.install_failed(cache, &key, &message);
                }
                false
            }
        }
    }

    /// Records a failed install with its backoff and prints it.
    fn install_failed(&mut self, cache: &Cache, key: &str, message: &str) {
        let now = Utc::now();
        let recorded = cache.update(|c| {
            let entry = c.installs.entry(key.to_owned()).or_default();
            entry.failures = entry.failures.saturating_add(1);
            entry.last_error = Some(ErrorRecord {
                at: schedule::stamp(now),
                message: message.to_owned(),
            });
            entry.next_attempt_at = Some(schedule::stamp(schedule::install_backoff(
                now,
                entry.failures,
            )));
        });
        self.report(format!("installing the update failed: {message}"));
        if let Err(e) = recorded {
            self.report(format!("{CACHE_UNWRITABLE}: {e:#}"));
        }
    }

    /// The `available` event, once per release and config: not again when
    /// the cache or this process already has it as notified. With
    /// `not_replaceable` (`auto` only) it adds `install`.
    fn notify(
        &mut self,
        cache: &Cache,
        key: &str,
        mode: UpdateMode,
        release: &CachedRelease,
        not_replaceable: Option<(&Path, Blocker)>,
    ) {
        let notified = cache
            .read()
            .configs
            .get(key)
            .and_then(|e| e.notified_version.clone());
        let shown = (key.to_owned(), release.version.clone());
        if notified.as_deref() == Some(release.version.as_str()) || self.notified.contains(&shown) {
            return;
        }
        let mut event = json!({"schema_version": 1, "update": {
            "event": "available",
            "current": version::RUNNING,
            "latest": release.version,
            "release_url": release.release_url,
        }});
        if let Some((path, blocker)) = not_replaceable {
            event["update"]["install"] =
                json!({"reason": blocker.reason(), "fix": blocker.fix(path)});
        }
        (self.emit)(&event);
        self.notified.insert(shown);
        let stored = cache.update(|c| {
            c.configs
                .entry(key.to_owned())
                .or_insert(ConfigEntry {
                    mode,
                    notified_version: None,
                    extra: Map::new(),
                })
                .notified_version = Some(release.version.clone());
        });
        if let Err(e) = stored {
            self.report(format!("{CACHE_UNWRITABLE}: {e:#}"));
        }
    }

    /// The installed version at `path`: `--version` runs only when the
    /// file's identity differs from the one recorded with the cached
    /// version, and that reading is recorded.
    fn installed_version(&mut self, cache: &Cache, path: &Path) -> Option<Version> {
        let identity = FileIdentity::read(path).ok()?;
        let key = install_key(path);
        if let Some(entry) = cache.read().installs.get(&key) {
            if entry.identity == Some(identity) {
                return entry
                    .version
                    .as_deref()
                    .and_then(|v| Version::parse(v).ok());
            }
        }
        let found = platform::probe(path, CLI).ok();
        let recorded = cache.update(|c| {
            let entry = c.installs.entry(key).or_default();
            entry.version = found.as_ref().map(ToString::to_string);
            entry.identity = Some(identity);
        });
        if let Err(e) = recorded {
            self.report(format!("{CACHE_UNWRITABLE}: {e:#}"));
        }
        found
    }

    /// Prints an error event; of the cache problems only the first.
    fn report(&mut self, message: String) {
        if message.starts_with(CACHE_UNWRITABLE) {
            if self.cache_reported {
                return;
            }
            self.cache_reported = true;
        }
        (self.emit)(&events::error(&message));
    }
}

/// The cache key of a config: its canonical path, else its absolute path.
fn config_key(config: &Path) -> String {
    std::fs::canonicalize(config)
        .or_else(|_| std::path::absolute(config))
        .unwrap_or_else(|_| PathBuf::from(config))
        .to_string_lossy()
        .into_owned()
}
