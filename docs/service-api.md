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
`--json` supported globally after subcommand too. Use explicit default config
path ./mailtriage.json, overridable global --config. Clear field accepts category
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
