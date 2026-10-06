#![cfg(unix)]
//! Refreshing release information against a loopback server: pagination,
//! the reservation, scheduling, retry hints, redirects and limits.
mod update_support;
use chrono::{DateTime, Duration, Utc};
use mailtriage::update::{
    cache::Cache,
    check::{self, Reservation},
    github::{Endpoint, Net},
    version,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use update_support::{Reply, Server, LIST};

fn setup() -> (Server, Net, Cache, tempfile::TempDir) {
    let server = Server::start();
    let net = Net::new(Endpoint::with_override(Some(&server.base))).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().join("cache"));
    (server, net, cache, dir)
}

fn at(text: &Option<String>) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text.as_deref().unwrap())
        .unwrap()
        .with_timezone(&Utc)
}

fn rc(n: usize) -> Value {
    json!({"tag_name": format!("v9.0.0-rc.{n}"), "draft": false, "prerelease": true, "assets": []})
}

#[test]
fn the_highest_stable_release_may_sit_on_page_two() {
    let (server, net, cache, _dir) = setup();
    let page2 = format!("{LIST}&page=2");
    let first: Vec<Value> = (0..30).map(rc).collect();
    server.reply(
        LIST,
        Reply::ok(Value::from(first).to_string())
            .header("Link", &format!("<{}>; rel=\"next\"", server.url(&page2))),
    );
    let second = vec![
        server.release_json("0.9.9", &[]),
        server.release_json("0.10.0", &[]),
        json!({"tag_name": "v1.0.0", "draft": true, "prerelease": false, "assets": []}),
    ];
    server.reply(&page2, Reply::ok(Value::from(second).to_string()));
    let checked = check::refresh(&net, &cache, Reservation::Required).unwrap();
    assert_eq!(checked.release.unwrap().version, "0.10.0");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let headers = &requests[0].headers;
    assert_eq!(
        headers["user-agent"],
        format!("mailtriage/{}", version::RUNNING)
    );
    assert_eq!(headers["accept"], "application/vnd.github+json");
    assert_eq!(headers["x-github-api-version"], "2022-11-28");
    assert_eq!(cache.read().release.unwrap().version, "0.10.0");
}

#[test]
fn a_failing_page_fails_the_whole_check() {
    let (server, net, cache, _dir) = setup();
    let page2 = format!("{LIST}&page=2");
    server.reply(
        LIST,
        Reply::ok(Value::from(vec![server.release_json("0.3.0", &[])]).to_string())
            .header("Link", &format!("<{}>; rel=\"next\"", server.url(&page2))),
    );
    server.reply(&page2, Reply::status(500));
    let e = check::refresh(&net, &cache, Reservation::Required).unwrap_err();
    assert_eq!(
        e.to_string(),
        "cannot read the release list: GitHub answered 500"
    );
    let file = cache.read();
    assert_eq!(file.release, None);
    assert_eq!(file.check_failures, 1);
    assert_eq!(
        file.last_check_error.unwrap().message,
        "cannot read the release list: GitHub answered 500"
    );
}

#[test]
fn more_than_twenty_pages_or_a_page_outside_github_fail() {
    let (server, net, cache, _dir) = setup();
    for page in 0..=20 {
        let target = if page == 0 {
            LIST.to_owned()
        } else {
            format!("{LIST}&page={page}")
        };
        let next = server.url(&format!("{LIST}&page={}", page + 1));
        server.reply(
            &target,
            Reply::ok("[]").header("Link", &format!("<{next}>; rel=\"next\"")),
        );
    }
    let e = check::refresh(&net, &cache, Reservation::Required).unwrap_err();
    assert_eq!(e.to_string(), "the release list has more than 20 pages");
    assert_eq!(server.count(LIST), 20);

    server.reply(
        LIST,
        Reply::ok("[]").header("Link", "<https://evil.example/releases>; rel=\"next\""),
    );
    let e = check::refresh(&net, &cache, Reservation::Required).unwrap_err();
    assert_eq!(e.to_string(), "a release list page is outside GitHub");
}

#[test]
fn the_reservation_is_written_before_the_request() {
    let (server, net, cache, dir) = setup();
    server.list(&[]);
    let seen = Arc::new(Mutex::new(None));
    let (file, store) = (dir.path().join("cache/update.json"), Arc::clone(&seen));
    server.on_request(LIST, move || {
        let text = std::fs::read_to_string(&file).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        *store.lock().unwrap() = value["next_check_at"].as_str().map(str::to_owned);
    });
    let before = Utc::now();
    let checked = check::refresh(&net, &cache, Reservation::Required).unwrap();
    assert_eq!(checked.release, None);
    let reserved = at(&seen.lock().unwrap().clone());
    assert!(reserved >= before + Duration::minutes(59), "{reserved}");
    assert!(reserved <= Utc::now() + Duration::hours(1));
}

#[test]
fn success_schedules_a_day_plus_jitter() {
    let (server, net, cache, _dir) = setup();
    server.list(&[server.release_json("0.3.0", &[])]);
    let before = Utc::now();
    check::refresh(&net, &cache, Reservation::Required).unwrap();
    let file = cache.read();
    let next = at(&file.next_check_at);
    assert!(next >= before + Duration::hours(24), "{next}");
    assert!(next <= Utc::now() + Duration::hours(25), "{next}");
    assert_eq!(file.check_failures, 0);
    assert_eq!(file.last_check_error, None);
    assert!(at(&file.checked_at) >= before - Duration::seconds(1));
}

#[test]
fn rate_limits_and_retry_after_set_the_next_check() {
    let (server, net, cache, _dir) = setup();
    let reset = Utc::now() + Duration::hours(3);
    server.reply(
        LIST,
        Reply::status(403)
            .header("X-RateLimit-Remaining", "0")
            .header("X-RateLimit-Reset", &reset.timestamp().to_string()),
    );
    let e = check::refresh(&net, &cache, Reservation::Required).unwrap_err();
    assert_eq!(
        e.to_string(),
        "cannot read the release list: GitHub's rate limit was reached"
    );
    assert_eq!(
        at(&cache.read().next_check_at).timestamp(),
        reset.timestamp()
    );

    server.reply(LIST, Reply::status(429).header("Retry-After", "18000"));
    let before = Utc::now();
    check::refresh(&net, &cache, Reservation::Required).unwrap_err();
    let file = cache.read();
    assert_eq!(file.check_failures, 2);
    let next = at(&file.next_check_at);
    assert!(
        next >= before + Duration::hours(5) - Duration::seconds(1),
        "{next}"
    );
}

#[test]
fn without_a_writable_cache_watch_makes_no_request() {
    let server = Server::start();
    let net = Net::new(Endpoint::with_override(Some(&server.base))).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file"), "").unwrap();
    let cache = Cache::new(dir.path().join("file/cache"));
    server.list(&[]);
    let e = check::refresh(&net, &cache, Reservation::Required).unwrap_err();
    assert!(
        e.to_string().starts_with("cannot write the update cache"),
        "{e}"
    );
    assert_eq!(server.requests().len(), 0);
    let checked = check::refresh(&net, &cache, Reservation::BestEffort).unwrap();
    assert_eq!(server.requests().len(), 1);
    assert_eq!(checked.warnings.len(), 2, "{:?}", checked.warnings);
}

#[test]
fn ten_redirects_are_followed_and_the_eleventh_fails() {
    let server = Server::start();
    let net = Net::new(Endpoint::with_override(Some(&server.base))).unwrap();
    for hop in 0..11 {
        server.reply(
            &format!("/hop/{hop}"),
            Reply::redirect(&server.url(&format!("/hop/{}", hop + 1))),
        );
    }
    server.reply("/hop/10", Reply::ok("ten"));
    assert_eq!(net.download(&server.url("/hop/0"), 3, 100).unwrap(), b"ten");
    server.reply("/hop/10", Reply::redirect(&server.url("/hop/11")));
    server.reply("/hop/11", Reply::ok("eleven"));
    let e = net.download(&server.url("/hop/0"), 6, 100).unwrap_err();
    assert_eq!(e.to_string(), "more than 10 redirects");
}

#[test]
fn downloads_stay_inside_the_rules_and_the_size_limit() {
    let server = Server::start();
    let net = Net::new(Endpoint::with_override(Some(&server.base))).unwrap();
    server.reply("/away", Reply::redirect("https://evil.example/x"));
    let e = net.download(&server.url("/away"), 1, 100).unwrap_err();
    assert_eq!(e.to_string(), "a redirect left GitHub");
    server.reply(
        "/plain",
        Reply::redirect("http://objects.githubusercontent.com/x"),
    );
    assert!(net.download(&server.url("/plain"), 1, 100).is_err());
    assert_eq!(
        net.download(&server.url("/big"), 101, 100)
            .unwrap_err()
            .to_string(),
        "it is larger than 100 bytes"
    );
    server.reply("/big", Reply::ok(vec![b'x'; 101]));
    assert!(net.download(&server.url("/big"), 1, 100).is_err());
    assert!(net
        .download("https://evil.example/x", 1, 100)
        .unwrap_err()
        .to_string()
        .contains("outside GitHub"));
    // Only the loopback origin of the override is allowed, not other ports.
    assert!(net.download("http://127.0.0.1:9/x", 1, 100).is_err());
}
