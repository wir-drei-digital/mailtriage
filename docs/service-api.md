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
- `doctor.items[].check`: `provider`, `key`, `mail`, and `filing` when filing is
  on; `state` (not ready) when `doctor` itself failed, for example when the
  state database cannot be opened. `error` and `fix` appear only when `ready`
  is false. A `key` fix for a command key names the current store:
  `mailtriage setup --update --account NAME --key-store pass` (`command` for a
  command that is no store's read command).
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
changed`, `setup aborted: input ended`); 3 Himalaya, key tool or service
manager failure, unwritable config; 5 config exists without `--update`, a
binding-changing update, unmarked service file.

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
  loaded, running, pid, last_exit_status, unit_path, log_paths, last_pass}`.
  `last_pass` is `{finished_at, partial, exit_code, mode}` or `null`.
- `Context::detect()` fails with exit 2 on platforms other than macOS and
  Linux and exit 3 on Linux without `systemctl`; `status` then reports
  `manager:"none"`. Unknown account: exit 2. Unmarked file at the unit path:
  exit 5. A failed `launchctl`/`systemctl` call: exit 3.

## Heartbeat (schema v5)

Migration 5 adds `pass_heartbeats(account TEXT PRIMARY KEY, finished_at TEXT
NOT NULL, partial INTEGER NOT NULL, exit_code INTEGER NOT NULL, mode TEXT NOT
NULL)`. `Service::sync` upserts one row per pass that took the account lock and
names a configured account: `exit_code` 0, 4 for partial, else the error's code
(the `ServiceError` code, 5 for an engine configuration change, else 3);
`partial` is false for a failed pass; `mode` is the pass's filing mode (`off`
without an engine). A failed heartbeat write never changes the pass result.
`Store::heartbeat(account)` returns the row as `last_pass`.

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

Schema v6 (migration 6): `placements.refile_once`, `placements.filed_home_folder`/`filed_home_epoch`/`filed_home_uid` (the occurrence a COPYUID-proven mailtriage move produced; kept only while it is the known home), `filing_intents.consumes_refile`, `folders.drain_until_uid` (NULL: frozen; 0: retained in the last pass; N: draining until UID N). The newer-schema guard is 6.

Also: `filing status` gains `refile_marked` and `refile_candidates`; `categories apply` gains `hint` (null with filing `off`); `filing plan` refile moves carry `"reason":"refile"`; events `refile_marked`, `refile_cleared {reason}`, `refile_cancelled {intent_id, reason}`, and `moved` with `"reason":"refile"`.
