#![cfg(unix)]
use mailtriage::{
    config,
    domain::HimalayaConfig,
    service::{ListOptions, Service},
};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};
#[test]
fn full_sync_is_idempotent_reconciles_removal_and_handles_epoch_reset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let state = dir.path().join("source.json");
    let binary = dir.path().join("himalaya");
    let hconfig = dir.path().join("himalaya.toml");
    fs::write(&state, json!({"epoch":1,"missing":[]}).to_string()).unwrap();
    fs::write(&hconfig,"[accounts.work]\nimap.server='imaps://example.test'\nimap.sasl.plain.username='work@example.test'\n").unwrap();
    let script = format!(
        r#"#!/usr/bin/env python3
import sys,json
s=json.load(open({state:?})); a=sys.argv[1:]
assert '--seen' not in a
if '--version' in a: print('himalaya v2.1.0 +imap')
elif 'status' in a: print(json.dumps({{'uid_validity':s['epoch'],'uid_next':3}}))
elif 'fetch' in a:
 lo,hi=map(int,a[-1].split(':'))
 print(json.dumps({{'messages':[{{'uid':u,'envelope':{{'subject':'Please reply','from':['Alex <a@example.test>'],'date':None}}}} for u in range(lo,hi+1) if u not in s['missing']]}}))
elif 'read' in a:
 uid=int(a[-1]); assert uid not in s['missing']
 sys.stdout.buffer.write(('From: a@example.test\r\nTo: work@example.test\r\nSubject: Please reply '+str(s['epoch'])+'-'+str(uid)+'\r\nContent-Type: text/plain\r\n\r\nPlease reply to this request.').encode())
else: sys.exit(3)
"#,
        state = state.to_str().unwrap()
    );
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let mut c = config::default_config();
    c.policy.review_mode = false;
    c.accounts.get_mut("work").unwrap().himalaya = Some(HimalayaConfig {
        binary,
        config: hconfig,
        account: "work".into(),
        mailboxes: vec!["INBOX".into()],
        expected_version: "2.1.0".into(),
        timeout_seconds: 3,
        max_output_bytes: 100000,
    });
    config::save(&path, &c).unwrap();
    let mut s = Service::open(&path).unwrap();
    let first = s.sync("work", 100).unwrap();
    assert_eq!(first["discovered"], 2);
    assert_eq!(first["classified"], 2);
    assert_eq!(first["fetched"], 2);
    assert_eq!(first["pending"], 0);
    assert_eq!(first["partial"], false);
    assert_eq!(s.sync("work", 100).unwrap()["classified"], 0);
    let items = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(items["total"], 2);
    let id = items["items"][0]["id"].as_str().unwrap();
    let detail = s.read("work", id).unwrap();
    assert_eq!(detail["item"]["source_occurrences"][0]["uid_validity"], 1);
    fs::write(&state, json!({"epoch":1,"missing":[1]}).to_string()).unwrap();
    s.sync("work", 100).unwrap();
    let items = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        items["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["source_present"] == false)
            .count(),
        1
    );
    fs::write(&state, json!({"epoch":2,"missing":[]}).to_string()).unwrap();
    let reset = s.sync("work", 100).unwrap();
    assert_eq!(reset["discovered"], 2);
    assert_eq!(reset["classified"], 2);
    assert_eq!(reset["partial"], false);
}
