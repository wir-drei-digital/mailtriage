#![allow(dead_code)]
//! Setup shared by the refile tests: mail that mailtriage filed and whose
//! category then changes, and readers for placements, events and calls.
use super::common::{mail, Harness};
use mailtriage::{
    domain::Category,
    filing::Placement,
    service::{RefileOptions, ServiceError},
};
use serde_json::Value;

/// The fake classifier files mail mentioning "update" into `updates`, or
/// into the catch-all `other` while `updates` is not configured.
pub fn updates_category() -> Category {
    Category {
        id: "updates".into(),
        name: "Updates".into(),
        description: "Status and service updates".into(),
        examples: vec![],
        catch_all: false,
        folder: Some("Updates".into()),
    }
}

/// Mail without a keyword; it stays in INBOX.
pub fn correspondence_category() -> Category {
    Category {
        id: "correspondence".into(),
        name: "Correspondence".into(),
        description: "Direct conversations with people".into(),
        examples: vec![],
        catch_all: false,
        folder: Some("INBOX".into()),
    }
}

/// Leaves categories out of the configuration file; call before the first pass.
pub fn without(h: &Harness, ids: &[&str]) {
    h.edit(|c| {
        c.accounts
            .get_mut("work")
            .unwrap()
            .categories
            .retain(|cat| !ids.contains(&cat.id.as_str()))
    });
}

/// `categories apply` with `category` added: open mail is classified again.
pub fn add_category(h: &Harness, category: Category) -> Value {
    let mut categories = h.service().config.accounts["work"].categories.clone();
    categories.push(category);
    h.service().apply_categories("work", categories).unwrap()
}

/// `categories apply` without category `id`.
pub fn remove_category(h: &Harness, id: &str) -> Value {
    let categories = h.service().config.accounts["work"]
        .categories
        .iter()
        .filter(|c| c.id != id)
        .cloned()
        .collect();
    h.service().apply_categories("work", categories).unwrap()
}

/// `categories apply` with `id` as the only catch-all: mail whose keyword
/// category is not configured follows it.
pub fn set_catch_all(h: &Harness, id: &str) {
    let mut categories = h.service().config.accounts["work"].categories.clone();
    for c in &mut categories {
        c.catch_all = c.id == id;
    }
    h.service().apply_categories("work", categories).unwrap();
}

/// Adds `updates` back and runs the pass that classifies open mail again.
pub fn add_updates(h: &Harness) {
    add_category(h, updates_category());
    h.sync();
}

/// Delivers `mid` into INBOX and runs the two passes that file it.
pub fn deliver_filed(h: &Harness, mid: &str, subject: &str, body: &str) -> String {
    h.fake.deliver("INBOX", &mail(mid, subject, body));
    h.sync();
    h.sync();
    id_of(h, mid)
}

/// "System update" mail (`u`) filed into Other while `updates` was left
/// out, then `updates` added back: the message now belongs in Updates.
pub fn filed_in_other_now_updates(h: &Harness) -> String {
    without(h, &["updates"]);
    h.sync();
    let id = deliver_filed(h, "u", "System update", "Version 2 is out");
    add_updates(h);
    id
}

pub fn id_of(h: &Harness, mid: &str) -> String {
    let rfc = format!("<{mid}@test>");
    h.service()
        .store
        .records("work")
        .unwrap()
        .into_iter()
        .find(|r| r.envelope["message_id"] == rfc.as_str())
        .unwrap()
        .id
}

/// Where the server holds `mid`: (folder, UID) of its first copy.
pub fn located(h: &Harness, mid: &str) -> (String, u64) {
    h.fake.locate(&format!("<{mid}@test>"))[0].clone()
}

pub fn placement(h: &Harness, id: &str) -> Placement {
    h.service().store.placement("work", id).unwrap().unwrap()
}

pub fn home(p: &Placement) -> Option<(String, u64, u64)> {
    Some((p.home_folder.clone()?, p.home_epoch?, p.home_uid?))
}

pub fn filed_home(p: &Placement) -> Option<(String, u64, u64)> {
    Some((
        p.filed_home_folder.clone()?,
        p.filed_home_epoch?,
        p.filed_home_uid?,
    ))
}

/// Sets the refile mark the way `filing refile --apply` does.
pub fn mark(h: &Harness, id: &str) {
    let mut s = h.service();
    let mut p = s.store.placement("work", id).unwrap().unwrap();
    let rev = p.desired_rev;
    p.refile_once = true;
    p.desired_rev += 1;
    assert!(s.store.save_placement(&p, Some(rev)).unwrap());
}

/// Filing events of `kind`, newest first.
pub fn events(h: &Harness, kind: &str) -> Vec<Value> {
    h.service()
        .store
        .events("work", None, 500)
        .unwrap()
        .into_iter()
        .filter(|e| e["kind"] == kind)
        .collect()
}

/// MOVE calls into `target`.
pub fn moves_to(h: &Harness, target: &str) -> usize {
    let suffix = format!(" -> {target}");
    h.fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("move ") && c.ends_with(&suffix))
        .count()
}

/// A safety pause on a folder, as an epoch race leaves it.
pub fn pause(h: &Harness, native: &str) {
    let mut s = h.service();
    let mut rec = s.store.folder_record("work", native).unwrap().unwrap();
    rec.pause_reason = Some("epoch_race".into());
    s.store.save_folder(&rec).unwrap();
}

pub fn review_state(h: &Harness, id: &str) -> String {
    h.service().read("work", id).unwrap()["item"]["review_state"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Whether the latest pass put `folder` in the engine's watch scope.
pub fn scoped(h: &Harness, folder: &str) -> bool {
    h.fake
        .calls()
        .iter()
        .rev()
        .find_map(|c| c.strip_prefix("scope ").map(str::to_string))
        .is_some_and(|scope| scope.split(',').any(|f| f == folder))
}

pub fn opts(category: Option<&str>, folder: Option<&str>) -> RefileOptions {
    RefileOptions {
        category: category.map(str::to_string),
        folder: folder.map(str::to_string),
        limit: 50,
    }
}

/// `filing refile` without `--apply`.
pub fn preview(h: &Harness, category: Option<&str>, folder: Option<&str>) -> Value {
    h.service()
        .filing_refile("work", opts(category, folder))
        .unwrap()
}

/// The exit code of a refused service call.
pub fn code(e: &anyhow::Error) -> Option<i32> {
    e.downcast_ref::<ServiceError>().map(|s| s.code)
}

/// `filing refile --apply`.
pub fn apply(h: &Harness, category: Option<&str>, folder: Option<&str>) -> Value {
    h.service()
        .filing_refile_apply("work", opts(category, folder))
        .unwrap()
}
