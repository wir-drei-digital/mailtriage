# Service API for CLI implementation

Use `mailtriage::service::{Service,ListOptions}` from the binary (src/cli.rs can
be declared in main.rs). All service methods return anyhow::Result<Value>.
Construct once per command: `Service::open(&Path)` -> Result<Self>. State path
and relative Himalaya config path resolve against app config file's parent.

```rust
pub struct ListOptions {
  pub view: String, // attention or all, default attention
  pub category: Option<String>,
  pub urgency: Option<String>,
  pub action_required: Option<bool>,
  pub limit: usize, // 1..=500, default 50
  pub cursor: Option<String>,
}
impl Default for ListOptions { /* above */ }
impl Service {
  pub fn open(path: &Path) -> Result<Self>;
  pub fn doctor(&mut self, account:&str) -> Result<Value>;
  pub fn classify(&mut self, account:&str, bytes:&[u8], format:&str) -> Result<Value>;
  pub fn sync(&mut self, account:&str, limit:usize) -> Result<Value>;
  pub fn list(&mut self, account:&str, opts:ListOptions) -> Result<Value>;
  pub fn read(&mut self, account:&str, id:&str) -> Result<Value>;
  // JSON map fields urgency:string, category_id:string, action_required:bool.
  pub fn correct(&mut self, account:&str,id:&str,patch:Value,clear:Option<&str>) -> Result<Value>;
  pub fn review(&mut self, account:&str,id:&str,done:bool) -> Result<Value>;
  pub fn categories(&mut self,account:&str) -> Result<Value>;
  // categories array; removed IDs with live overrides must be cleared/remapped
  // explicitly by user first, otherwise error. No hidden category deletion.
  pub fn apply_categories(&mut self,account:&str,categories:Vec<Category>) -> Result<Value>;
  pub fn reclassify(&mut self,account:&str,since:Option<&str>,dry_run:bool,limit:usize) -> Result<Value>;
  pub fn export(&mut self,account:&str) -> Result<Value>;
}
```

`init` is CLI-owned: default_config + config::save, refuse overwrite existing;
choose state_dir relative .state. `categories validate` accepts JSON category
array or {categories:[...]}; can use default config work account and validate.
`watch` is CLI-owned repeated Service::open + sync, signal graceful stop, default
60s interval, bounded limit default100; sleeps in short intervals for signal.
`--json` supported globally after subcommand too. The config path follows the
resolution order below; `init` writes `--config` or ./mailtriage.json. Clear field accepts category
alias mapping to category_id. Main errors printable JSON with safe message.

Error mapping: downcast `service::ServiceError {pub code:i32,pub message:String}`
for deliberate service errors; otherwise 3 for operational errors. Invalid
argument/config errors 2, revision/cursor/identity conflict 5. Partial sync
returns successful Value with `partial:true`; CLI exits4 after printing.
Never print raw debug chain from provider/transport errors (possible secrets).

Develop CLI against these signatures. Do not edit lib.rs/service.rs/store.rs;
coordinator provides them. Own src/main.rs src/cli.rs README.md examples/, docs/
hermes.md, .github/workflows/ci.yml and tests/cli.rs. Offline tests run explicit
fake provider using temporary config. release.yml optional with artifacts.

## Config resolution

`config::resolve_path(flag, env, cwd, home)` gives the config for every command
except `init` and `setup`: `--config`, else `MAILTRIAGE_CONFIG` (ignored when
empty), else `<cwd>/mailtriage.json` if it exists, else
`~/.config/mailtriage/mailtriage.json`. `config::setup_path(flag, env, home)`
is the same without the working-directory step; `setup` uses it. Without
`HOME` the last step fails with `cannot find the config: HOME is not set; pass
--config PATH` (exit 2). `Service::open` on a missing file exits 2 with
``configuration not found; run `mailtriage setup` or pass --config``.

## `setup` result

`setup::run(&SetupArgs, &Path, &mut Prompter) -> Result<Value>`. Prompts and
progress go to the prompter's output (stderr in the CLI); the value is the only
stdout:

```json
{"schema_version":1,"setup":{
  "config": "/abs/path/mailtriage.json",
  "account": "work",
  "mailboxes": ["INBOX"],
  "provider": "openrouter",
  "model": "typesafe/jev-1.13",
  "key_source": "command",
  "key_store": "keychain",
  "filing": "dry_run",
  "updates": "auto",
  "doctor": {"ready": false, "items": [
    {"check": "provider", "ready": true},
    {"check": "key", "ready": false,
     "error": "OpenRouter API key environment variable is missing",
     "fix": "export OPENROUTER_API_KEY=<your OpenRouter key> where mailtriage runs"},
    {"check": "mail", "ready": true},
    {"check": "filing", "ready": true}]},
  "service": null}}
```

- `key_source`: `command`, `env`, or `null` for `fake`.
- `key_store`: the `--key-store` flag value chosen in this run; `null` when no
  store was chosen: the classifier was kept, `--model` or `--provider
  openrouter` changed it without a key flag (the key source is kept), or it is
  `fake`.
- `filing`: `off`, `dry_run` or `live`.
- `updates`: the config's `updates` after this run (`auto`, `notify` or `off`);
  `--updates` sets it, otherwise an existing config keeps its value.
- `doctor.items[].check`: `provider`, `key`, `mail`, and `filing` when filing is
  on; `state` (not ready) when `doctor` itself failed, for example when the
  state database cannot be opened. `error` and `fix` appear only when `ready`
  is false. A `key` fix for a command key names the current store:
  `mailtriage setup --update --account NAME --key-store pass` (`command` for a
  command that is no store's read command). A `mail` fix for an untested
  Himalaya (`transport.tested` false) is `run mailtriage himalaya install, then
  mailtriage setup --update --account NAME --himalaya-binary PATH`
  (`setup::himalaya_install_fix`), with the path `himalaya install` would
  print, or `<the path it prints>` without a data directory. Step 2's failure
  for a missing or untested Himalaya ends with the same fix, naming
  `--account` and `--himalaya-account` when known then, and `, or pass
  --himalaya-install`.
- `service`: `null` when skipped, or when the `state` item is not ready (setup
  then installs nothing), else the `service install` object below.
- Printed `mailtriage` commands (fixes, next steps, the categories hint) pass
  `--config '<written path>'` when `config::resolve_path(None,
  MAILTRIAGE_CONFIG, cwd, HOME)` would not resolve to the written config
  (compared canonically); a `./mailtriage.json` that shadows it gets one
  warning line. The step-10 retry always passes `--config`, `--interval-seconds`
  and `--limit`.

Before step 8 writes, `service::stored_binding_matches(config, &AppConfig,
account) -> Result<Option<bool>>` compares the binding stored in the state
database (resolved as `Service::open` resolves it, opened read-only, never
created) with the one the new config gives the account, computed as `ensure`
computes it. `Some(false)` exits 5 with `step 3 (account): account NAME is
bound to its previous mailbox (identity, Himalaya account or IMAP server
changed); keep them, or set this mailbox up under a new name with --account
NEW`, and nothing is written. `None` (no database or no row) and read errors
let setup go on; step 9 then reports what `doctor` finds.

Errors are `ServiceError`s. Except for the two abort messages below, the
message starts with `step N (name): ` and names the flag or command that fixes
it. Codes: 2 input, missing flag without
prompts, key flags with `--provider fake`, abort (`setup aborted; nothing was
changed` with reason `setup_aborted`, which `self install` reports as
`setup: "skipped"`; `setup aborted: input ended` without a reason); 3
Himalaya, key tool or service manager failure, unwritable config; 5 config
exists without `--update`, a
binding-changing update, unmarked service file, another command holding the
config lock (`config_busy`), or a config that changed since step 1 read it
(`config_changed`); step 8 writes under the exclusive config lock.

## `doctor` key fields

`provider` carries `kind`, `model`, `configuration_valid`, `key_source`
(`command` when `api_key_command` is set, else `env`; `null` for `fake`) and
`key_present`. When the key is missing, `key_error` holds one fixed string:
`API key command failed (exit N)` (`exit signal` when killed),
`API key command timed out`, `API key command printed no key`,
`API key command could not start`,
`OpenRouter API key environment variable is missing`, or
`OpenRouter API key environment variable is empty`. For `command`, `doctor` runs
the key command (once per `Service`; `secrets::KeyCache`).

`transport` carries `configured` and `ready`, and for a configured engine
`version` (the first line of `himalaya --version`) and `tested` (whether that is
a tested version). When the version is tested, it also carries
`alias_conflicts` (the source folders Himalaya resolves to another mailbox,
which no pass reads from). When it is not ready, `error` is
`Himalaya X is not a tested version (tested: …)`,
`Himalaya X was built without IMAP (+imap)`,
`Himalaya printed no version mailtriage knows (tested: …)`,
`cannot read the Himalaya configuration: …` (no folder is read), or
`Himalaya version/config check failed` (then without `version`, `tested` or
`alias_conflicts`).

## Classification without a key

`sync`, `classify` and `reclassify` resolve the OpenRouter key once through the
service's `KeyCache` before they lease any job, and only when a job is eligible
to lease (`sync`: `queued_outside` is not empty; `classify`:
`Store::leasable`; `reclassify`: a message is selected). With nothing to
classify the key command does not run and the result is not partial. When the
key cannot be resolved they lease nothing (no attempt is used, jobs stay
queued) and add
`"classification": {"skipped": true, "reason": KEY_ERROR}` with one of the
fixed key-error strings above; `partial` is then `true`. `sync` still runs
discovery and the filing steps, with `fetched`, `classified`, `cached` and
`failed` 0. `classify` stores the message and reports `outcome: "skipped"`;
`reclassify` requeues the matched messages and reports `reclassified: 0`. The
field is absent when classification ran or nothing was due, and for the `fake`
provider.

## `service` results

The CLI wraps each in `{"schema_version":1,"service":{...}}`.

- `system_service::install_account(&Service, config, account, interval, limit,
  &Context)`: `{action:"installed", manager:"launchd"|"systemd", account,
  unit_path, log_paths, command}`, plus `note` when the provider is `openrouter`
  without `api_key_command`: the `setup --update --config … --account …
  --key-store …` command for the platform's own store (`keychain`; `secret-service`
  or `pass`) and a warning not to put the key into the plist or unit.
  `log_paths` is `[]` for systemd. `command` is the
  `watch` argument list with absolute paths.
- `system_service::uninstall(&Context, account)`: `{action:"uninstalled" |
  "not_installed", manager, account, unit_path}`.
- `system_service::status_account(&Service, config, account,
  Option<&Context>)`: `{manager:"launchd"|"systemd"|"none", account, installed,
  loaded, running, pid, last_exit_status, unit_path, log_paths, last_pass,
  update}`. `last_pass` is `{finished_at, partial, exit_code, mode, version,
  reason}` or `null`.
  `update` is `update::report::update_block(mode, Option<(Manager, unit
  path)>, Option<Cache>)`: `{mode, executable, installed, latest, available,
  checked_at, last_error, replaceable, reason}`, plus `tray: {path, installed,
  available}` (`update::report::tray_block`) when a regular file
  `mailtriage-tray` sits next to the executable it describes. The tray's
  `installed` is the version its `installs` entry recorded for the same file
  (same identity), as `watch` reads it; otherwise `--version` runs, once per
  file identity per process (a replaced file is probed again). Nothing is
  written to the cache. `status_account` passes the cache of the
  context's home (`Cache::for_home`: `XDG_CACHE_HOME` counts only for the home
  in `HOME`), so a test `Context` never reads this user's cache; without a
  context it uses `Cache::for_user()`. `doctor` adds
  `update::report::doctor_block` (the same with `Cache::for_user()`, plus
  `ready`, and `fix` when not ready; `tray` included), and setup step 9 adds
  an `update` item when that block is not ready.
- `Context::detect()` fails with exit 2 on platforms other than macOS and
  Linux and exit 3 on Linux without `systemctl`; `status` then reports
  `manager:"none"`. Unknown account: exit 2. Unmarked file at the unit path:
  exit 5. A failed `launchctl`/`systemctl` call: exit 3.

## `update`

`update::command::run(check_only, &dyn Hooks) -> Result<Value>` is
`mailtriage update [--check]`; it opens no config. Errors are `ServiceError`s:
3 for network, release, archive, smoke-test and replaceability problems of the
CLI, 5 when the installation lock stayed held for 60 s. The JSON results are
in the [guide](guide.md#updates). The code is in `src/update/`: `github` (URL
rules, release list, downloads), `release` (candidate, archive names,
`SHA256SUMS`), `cache` (`update.json`), `schedule`, `check` (one refresh,
and `due`: when `watch` refreshes), `platform` (file identity,
replaceability, `--version` probes), `archive`, `install` (the transaction
under the installation lock), `service_files` (decoding service files) and
`command`.

The updater installs `update::COMPONENTS`: `CLI` (`mailtriage`) and `TRAY`
(`mailtriage-tray`, at `update::tray_path(cli)`, used when
`update::installed_tray(cli)` finds a regular file there). Both go through
`install::install` with their own `Job { component, path, fallback }`; the
tray's fallback is its own probed version. `run` handles the CLI first and
then the tray under the same installation lock. A failed tray part, including
a tray the replaceability check (`platform::blocker`) refuses, keeps the CLI's
result and sets top-level `"partial": true` (exit 4); a failed CLI part never
reaches the tray. Every check records `release.archives` for each
component (`null` when the release lacks it); a missing key was written by an
older version and counts as unknown (`CachedRelease::knows`), so `watch`
refreshes early once (`check::due`) and status reports the tray's `available`
as false. `Cache::record_install_failure` (backoff), `record_skip` (no
backoff) and `record_version` are the `installs` writers that `update` and
`watch` share.

## Heartbeat (schema v5)

Migration 5 adds `pass_heartbeats(account TEXT PRIMARY KEY, finished_at TEXT
NOT NULL, partial INTEGER NOT NULL, exit_code INTEGER NOT NULL, mode TEXT NOT
NULL)`. `Service::sync` upserts one row per pass that names a configured
account, including a pass that finds another worker holding the account lock
(exit 5, reason `account_busy`; the holder's own heartbeat replaces it when its
pass ends): `exit_code` 0, 4 for partial, else the error's code
(the `ServiceError` code, 5 for an engine configuration change, else 3);
`partial` is false for a failed pass; `mode` is the pass's filing mode (`off`
without an engine). A failed heartbeat write never changes the pass result.
`Store::heartbeat(account)` returns the row as `last_pass`.

## Categories digest and changes

`src/categories.rs` (`mailtriage::categories`) holds what `categories apply`
would write for an account:

```rust
pub fn filing_on(account: &AccountConfig) -> bool; // filing.mode != off
pub fn digest(filing_on: bool, categories: &[Category]) -> String;
pub fn account_digest(account: &AccountConfig) -> String;
pub fn normalized(previous: &AccountConfig, categories: Vec<Category>) -> Vec<Category>;
pub fn changes(previous: &[Category], next: &[Category]) -> Changes;

#[derive(Serialize)] pub struct Changes {
  pub added: Vec<String>, pub removed: Vec<Removed>, pub renamed: Vec<Change>,
  pub folders_changed: Vec<Change>, pub edited: Vec<String>, pub reclassifies: bool,
}
#[derive(Serialize)] pub struct Removed { pub id: String, pub folder: String }
#[derive(Serialize)] pub struct Change { pub id: String, pub from: String, pub to: String }

impl Service {
  pub fn apply_categories_expecting(&mut self, account:&str, categories:Vec<Category>,
    expected:Option<&str>) -> Result<Value>;
}
```

- `filing_on` is the configured filing mode, not the effective one of an
  account without an engine; it is the rule `normalized` uses.
- `digest` is `v1:` plus the lowercase hex SHA-256 of
  `serde_json::to_vec(&DigestInput { filing_on, categories })`: an object with
  `filing_on` first, then `categories`, each `Category` with its fields in
  declaration order (`id`, `name`, `description`, `examples`, `catch_all`,
  `folder`) and `folder` omitted when unset. Key order and `"folder": null`
  versus no `folder` in a file therefore do not change it; category order does.
  `account_digest` is `digest(filing_on(account), &account.categories)`.
- `normalized` is apply's folder normalization: with filing on, a category
  without `folder` keeps its ID's previous effective folder, and a new one uses
  its name.
- `changes` compares `previous` with an already normalized `next`. `added`,
  `renamed`, `folders_changed` and `edited` follow `next`'s order, `removed`
  `previous`'s. `folders_changed` and `removed[].folder` hold effective folder
  names in every filing mode, for display. `edited` lists categories whose
  `description`, `examples` or `catch_all` changed. `reclassifies` is
  `category_semantics(previous) != category_semantics(next)`, exactly apply's
  rule for advancing `taxonomy_revision`.
- `apply_categories(account, categories)` is
  `apply_categories_expecting(account, categories, None)`. The order is: the
  exclusive config lock, the unchanged-file guard (`config_changed`), then,
  with `expected`, `account_digest` must equal it, else exit 5
  `categories changed since export; export again` with `ErrorKind::CategoriesChanged`
  (reason `categories_changed`) and nothing written; then everything else.

JSON additions:

- `categories export` gains `digest`, the account's current digest.
- `categories validate --account` gains `digest` (the account's current one,
  which `--expect-digest` compares, not one of the file) and `changes`.
- `categories apply` returns export's shape, so it gains `digest` too.
- `categories apply --expect-digest D` passes `Some(D)`.

## `service_control`

`src/service_control.rs` (`mailtriage::service_control`) adds to
`system_service` what `service status` reports about the config a service runs,
its enablement and the fields the tray reads. It writes no service file.

```rust
pub const PROC_ROOT: &str = "/proc";

pub struct WatchArgs { pub config: PathBuf, pub interval_seconds: Option<u64> }
pub fn watch_args(args: &[String]) -> Option<WatchArgs>;

// Decoders: the argument list, `None` when it cannot be read or decoded.
// The unit file and plist decoders are `update::service_files::{plist_arguments,
// unit_arguments}`, which `inspect` uses.
pub fn launchctl_arguments(text: &str) -> Option<Vec<String>>;   // `arguments = {…}` of `launchctl print`
pub fn exec_start_arguments(value: &str) -> Option<Vec<String>>; // argv[] of `systemctl show -p ExecStart`
pub fn process_arguments(proc_root: &Path, pid: u32) -> Option<Vec<String>>; // <proc_root>/<pid>/cmdline

pub fn launchd_enablement(output: Option<&str>, label: &str) -> (Option<bool>, String);
pub fn systemd_enablement(output: Option<&str>) -> (Option<bool>, String);

pub struct Inspection {
  pub installed: bool, pub loaded: Option<bool>, pub running: bool, pub pid: Option<u32>,
  pub service_config: Option<PathBuf>, pub file_config: Option<PathBuf>,
  pub named: Vec<Option<PathBuf>>, pub needs_daemon_reload: Option<bool>,
  pub enabled: Option<bool>, pub enablement: String, pub interval_seconds: Option<u64>,
}
pub fn inspect(ctx: &Context, account: &str, proc_root: &Path) -> Inspection;
pub fn other_config(found: &Inspection, resolved: &Path) -> Option<PathBuf>;
pub fn config_matches(found: &Inspection, manager: Manager, resolved: &Path) -> Option<bool>;
pub fn status(service: &Service, config_path: &Path, account: Option<&str>,
  ctx: Option<&Context>, proc_root: &Path) -> Result<Value>;
```

- `watch_args` takes the values after `--config` and `--interval-seconds`
  wherever they stand; `None` when `--config` is missing, has no value or
  appears twice. An `--interval-seconds` that is not a number gives `None` for
  `interval_seconds` only.
- `update::service_files::{plist_arguments, unit_arguments}` invert `plist`
  and `systemd_unit` (XML entities; systemd quoting, `%%` and `$$`), so the
  paths come back exact. Anything those writers never write (an unknown
  entity, a raw `<`, a single `%` or `$`, a quote in a bare word) gives
  `None`, so `inspect` reports the file's config as unknown rather than a
  wrong one.
- `process_arguments` reads the start time (field 22 of `stat`) before and
  after `cmdline` and returns `None` when the two differ, so a PID reused in
  between is never read.
- `exec_start_arguments` accepts only the shape `service install` writes:
  11 words, `watch`, `--config`, `--account` and `--interval-seconds` at their
  places. A path that `show` printed without quotes therefore gives `None`,
  never a shorter path.
- `inspect` reads the unit file; a missing or unmarked file is not installed
  (`enabled: Some(false)`, `enablement: "not_installed"`, everything else
  empty). It then runs `launchctl print gui/<uid>/<label>` and
  `launchctl print-disabled gui/<uid>`, or `systemctl --user show <unit>
  --property=LoadState,ActiveState,SubState,MainPID,ExecStart,NeedDaemonReload`
  and `systemctl --user is-enabled <unit>`, each with a 30 s limit and 1 MiB of
  output. `interval_seconds` comes from the unit file.

`service_config` and the configs `config_matches` compares
(`Inspection::named`, in this order):

| Manager and job | `service_config` | Configs compared for `config_matches` |
| --- | --- | --- |
| launchd, `print` exits 0 | `arguments` block | it, file |
| launchd, `print` exits 113 (not loaded) | unit file | file |
| launchd, any other failure | `null` | `null`, file |
| systemd, loaded and running | `/proc/<MainPID>/cmdline`, start time unchanged | it, loaded `ExecStart`, file |
| systemd, loaded, not running | loaded `ExecStart` | it, file |
| systemd, not loaded | unit file | file |
| systemd, `show` fails | `null` | `null`, file |

- `config_matches` is `Some(true)` when not installed. A compared config that
  is not `resolved` (canonicalized when it exists) gives `Some(false)`, and
  `other_config` returns the first such config. Otherwise any `None` gives
  `None`, and so does systemd's `NeedDaemonReload` unless it is `no`; else
  `Some(true)`.
- `launchd_enablement`: the label listed as `disabled` or `true` gives
  `(Some(false), "disabled")`; `enabled`, `false` or not listed gives
  `(Some(true), "enabled")`; another value or a failed query (`None`) gives
  `(None, "unknown")`.
- `systemd_enablement` takes the first line of `is-enabled`, whatever its exit
  code: `enabled` and `enabled-runtime` give `Some(true)`, `disabled`, `masked`
  and `masked-runtime` `Some(false)`, any other state `None`, each with that
  state as `enablement`. No output, or a command that could not run, gives
  `(None, "unknown")`.

`status` is `service status`: `{schema_version:1, config, services:[…]}` with
one object per account in name order, or `{schema_version:1, config, service}`
with `account`. `config` is the canonical config path. Each object is
`system_service::status_account`'s plus `service_config`, `file_config`,
`config_matches`, `enabled`, `enablement`, `interval_seconds`, `filing_mode`
(the configured mode, `filing::mode_str`) and `identity`, and
`needs_daemon_reload` on systemd only. Without a `Context` (no supported
manager) every account reads as not installed. An unknown account exits 2. The
CLI prints the value as it is, with `PROC_ROOT`.

### `service start`, `stop` and the service lock

```rust
pub const SERVICE_LOCK_WAIT: Duration; // 30 s
pub const START_WAIT: Duration;        // 5 s
pub fn lock_dir(ctx: &Context) -> PathBuf;
pub struct ServiceLock { /* the locked file */ }
pub fn lock(ctx: &Context, account: &str, wait: Duration) -> Result<ServiceLock>;
pub fn start(ctx: &Context, service: &Service, config_path: &Path, account: &str,
  proc_root: &Path, wait: Duration) -> Result<Value>;
pub fn stop(ctx: &Context, service: &Service, config_path: &Path, account: &str,
  proc_root: &Path) -> Result<Value>;
```

- `lock` checks the account name first (exit 2, `account name "…" cannot name
  a service; use 1 to 64 letters, digits, - or _`), because it becomes part of
  a file name. It then creates `lock_dir` (mode 0700) and takes an exclusive
  lock on `service-<label>.lock` there, retrying every 100 ms for up to `wait`;
  after that it fails with exit 5, `ErrorKind::ServiceBusy` (`service_busy`):
  `another service command for account A is running; try again`. The lock is
  held until the `ServiceLock` is dropped. `lock_dir` is
  `$HOME/Library/Caches/mailtriage` for launchd and `$HOME/.cache/mailtriage`
  for systemd, from `Context::home` only (never `XDG_CACHE_HOME`).
- The CLI holds the lock with `SERVICE_LOCK_WAIT` around `service install`,
  `uninstall`, `start` and `stop`, from opening the config through the last
  manager call; `setup` holds it around step 10's install. `service status`
  takes no lock.
- `start` and `stop` first refuse what they must not act on, with no manager
  call beyond `inspect`'s queries:
  - an account that is not in the config: exit 2, `unknown account`;
  - a service that is not installed: exit 2, `service for account A is not
    installed; run mailtriage service install --account A`;
  - `config_matches` `Some(false)`: exit 5, `ErrorKind::ServiceConfigMismatch`
    (`service_config_mismatch`), `service for account A runs config X; pass
    --config X`, where X is `other_config` and the second X is shell-quoted;
  - `config_matches` `None`: exit 5, `ErrorKind::ServiceConfigUnknown`
    (`service_config_unknown`), `cannot tell which config the service for
    account A runs; try again, or reinstall it with mailtriage service install
    --account A`.
- `start`, launchd: `launchctl enable gui/<uid>/<label>`, then nothing more
  when the job was loaded and running (`already_running`), `kickstart
  gui/<uid>/<label>` when it was loaded but not running, else `bootstrap
  gui/<uid> <plist>` (`started`). systemd: `systemctl --user enable --now
  <unit>`; `already_running` when the unit was running, else `started`. After
  `started` it polls every 200 ms for up to `wait` (the CLI passes
  `START_WAIT`) until `launchctl print` shows a PID or `systemctl --user
  is-active` prints `active`; otherwise exit 3, `service did not start; see
  LOG`, where LOG is `<canonical config dir>/logs/<account>.err` on launchd and
  `journalctl --user -u mailtriage-<account>.service` on systemd.
- `stop`, launchd: `system_service::bootout` (bootout and wait until launchd
  drops the job, as `install` does), then `launchctl disable
  gui/<uid>/<label>`; `stopped` when the job was loaded, else
  `already_stopped`. systemd: `systemctl --user disable --now <unit>`;
  `stopped` when it was running, else `already_stopped`. The unit file stays.
- A failed `launchctl` or `systemctl` call is exit 3, as in `install`.
- Both return `{action:"started"|"already_running"|"stopped"|"already_stopped",
  manager:"launchd"|"systemd", account, unit_path}`; the CLI wraps it in
  `{"schema_version":1,"service":{...}}`.
- `system_service::install` now runs `launchctl enable gui/<uid>/<label>`
  between the bootout and the `bootstrap`, so a reinstall clears a `stop`.

## `filing refile` (schema v6)

```rust
// mailtriage::service::RefileOptions (= filing::refile::command::RefileOptions)
pub struct RefileOptions {
  pub category: Option<String>, // only candidates whose new category is this id
  pub folder: Option<String>,   // a folder's native name, or its configured name
  pub limit: usize,             // 1..=500, default 50; lists candidates only
}
impl Service {
  // Preview: no locks, no engine calls; may bring the generation up to date.
  pub fn filing_refile(&mut self, account: &str, opts: RefileOptions) -> Result<Value>;
  // Filing `live` only; shared configuration lock; `mailtriage.json` must be unchanged.
  pub fn filing_refile_apply(&mut self, account: &str, opts: RefileOptions) -> Result<Value>;
}
```

Preview result (filing `off`: the same shape, empty):

```json
{"schema_version":1,"account":"work","mode":"live",
 "candidates":[{"id":"msg_…","folder":"INBOX.Other","target":"INBOX.Updates","category":"updates","reason":"category_changed"}],
 "total":1,
 "folders":[{"folder":"Other","native":"INBOX.Other","retired":false,"candidates":1,"waiting":0}],
 "waiting":0,
 "skipped":{"not_filed_by_mailtriage":0,"corrected":0,"pinned":0,"blocked":0,"done":0,"open_intent":0,"explicit_target":0,"multiple_copies":0,"incomplete_input":0,"retired_frozen":0,"target_unusable":0,"target_inbox_or_source":0}}
```

`reason` is `category_changed` or `folder_retired`. Candidates and waiting messages are ordered by native folder, then UID; `folders` by native name. `--category` filters `candidates` and `skipped`, not `waiting`.

Apply result: `{"schema_version":1,"account":"work","marked":N,"waiting_marked":W}`. `marked` counts candidates newly marked, `waiting_marked` waiting messages newly marked (only with `folder` and no `category`). Repeating it marks 0.

Errors: 2 for `--apply` outside `live` (`refile --apply requires filing mode live`), an unknown category, a folder that names no category or retired folder (`unknown folder: not a category or retired folder`) or several (`folder name matches several folders; pass the native name: A, B`), a limit outside 1..=500, an unknown account; 3 when the state database is unavailable; 5 when `mailtriage.json` changed (`reason: config_changed`) or the placements kept changing (`placements changed concurrently; retry`).

Schema v6 (migration 6): `placements.refile_once`, `placements.filed_home_folder`/`filed_home_epoch`/`filed_home_uid` (the occurrence a COPYUID-proven mailtriage move produced; kept only while it is the known home), `filing_intents.consumes_refile`, `folders.drain_until_uid` (NULL: frozen; 0: retained in the last pass; N: draining until UID N). The newer-schema guard is the public `store::LATEST` (10 since schema v10, see "Reply queue").

Also: `filing status` gains `refile_marked` and `refile_candidates`; `categories apply` gains `hint` (null with filing `off`); `filing plan` refile moves carry `"reason":"refile"`; events `refile_marked`, `refile_cleared {reason}`, `refile_cancelled {intent_id, reason}`, and `moved` with `"reason":"refile"`.

## Heartbeat version (schema v7)

Migration 7 adds the nullable column `pass_heartbeats.version`.
`Store::record_heartbeat` writes the running version (`CARGO_PKG_VERSION`) on
every heartbeat, error heartbeats included, and `Store::heartbeat` returns it
as `version` (`null` for rows written before v7). The newer-schema guard is
`LATEST`, the last migration's version. Like every migration of a stable
release, it is additive, so a process of the previous release keeps
inserting heartbeats on an open connection after another process migrated.
A process of a release before v7 updates an existing heartbeat row without
touching `version`, so during a rolling update a row can keep the newer
process's version until the next pass of a v7-capable process.

## Heartbeat reason (schema v8)

Migration 8 adds two nullable columns, `pass_heartbeats.reason` and
`pass_heartbeats.reason_at`. `Store::record_heartbeat(account, partial,
exit_code, mode, reason)` writes `reason` and sets `reason_at` to the
`finished_at` it writes, in the same upsert. `Service::sync` passes
`service::error_reason(&error)`, the `reason` the CLI's error object carries
(a `ServiceError`'s kind, or `config_changed` for the engine's
`ConfigChanged`), and `None` for a pass that did not fail. So error heartbeats
carry `config_changed` (the errors `watch` skips), `account_busy` (another
worker holds the account lock), `binding_conflict` and the other reasons, and
`null` for an error without one. `Store::heartbeat` returns `reason` only when
`reason_at` equals `finished_at`, else `null`: rows written before v8 read
`null`, and so does a row that a process of a release before v8 rewrote after
the migration (it updates `finished_at` but neither `reason` nor `reason_at`),
so a reason never outlives its pass. `store::LATEST` is public: tests name the
latest schema `LATEST` and a newer one `LATEST + 1`.

## Reply queue (config schema 4)

`filing.reply_queue` (default `false`, written only when true) holds new mail
that needs action in its source folder until it is answered or marked done
(spec: `docs/superpowers/specs/2026-10-08-reply-queue-design.md`). A config
that turns it on is written as `schema_version` 4; every other config stays
3. `config::SCHEMA_VERSION` (4) is the newest schema a binary reads;
`config::written_schema` is the one a write produces.

Schema v9 (migration 9) adds the table `read_approvals(account, message_id,
requested_at, approved_at, applied_at)`, the read approval list. A claimed
reply exit enters it once (`Store::request_read_approval`);
`Store::approve_reads` sets `approved_at`, and a live pass sets `applied_at`
after adding `\Seen` (`Store::mark_read_applied`).

Schema v10 (migration 10) adds the nullable columns
`read_approvals.attempt_folder` and `attempt_epoch`: the folder and epoch of
an `add_seen` session whose outcome is not known yet. A live pass writes
them (`FilingWrite::ReadAttempt`) before the session and clears them once
its outcome is known; `filing::reply::recover_reads`, run by
`filing::recover::recover`, compares a remaining attempt with the folder's
epoch. Additive, so a process of the previous release keeps inserting rows.
`store::LATEST` is 10.

- `filing enable --reply-queue on|off` (`Service::filing_enable_with`); the
  result gains `reply_queue`.
- `filing status` gains `reply_queue`, `awaiting_reply` and
  `awaiting_reply_ids` (up to 50).
- `filing status` also gains `read_waiting` and `read_approved_pending`.
- The sync `filing` object (and the stored last pass) gains
  `awaiting_reply`, `reply_exits`, `replies_checked` and `reads_applied`,
  each only when not 0; problems `reply_check_failed:<folder>`,
  `read_failed:<folder>` and `read_incomplete:<folder>`, and
  `epoch_race:<folder>` for a `\Seen` race.
- `filing replies [--approve [--id ID]...]` (`Service::filing_replies`):
  `{"schema_version":1,"account":"work","waiting":W,"approved_pending":A,"approved":[ids],"items":[{"id","subject","from","folder","answered","requested_at","approved_at"}]}`.
  An `--id` that is not waiting is exit code 2 and approves nothing.
- `filing plan`: a reply exit's move carries `"reason":"reply_exit"`.
- `MailEngine::add_seen(folder, uids)`: one session `a1 SELECT; a2 UID
  STORE uids +FLAGS.SILENT (\Seen)`, returning a `WriteOutcome` like
  `add_flagged`. `filing::reply::apply_reads` maps it: `selected`,
  `completed` and the verified `session_epoch` set `applied_at`; `selected`
  in another epoch is an epoch race (pause `epoch_race`, event `epoch_race`
  with `{"kind":"seen","error":"epoch_race","epoch":E}` per message, rows
  kept approved); anything else leaves the rows for the next pass
  (`read_incomplete`); an error keeps the attempt for recovery, which
  reports another epoch as a suspected race (`"error":"epoch_race_suspected"`).

