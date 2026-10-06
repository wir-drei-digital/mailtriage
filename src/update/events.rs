//! `watch`'s update events: `{"schema_version":1,"update":{"event":…}}`,
//! printed the way `watch` prints passes: one JSON line with `--json`,
//! else one text line.
use serde_json::{json, Value};
use std::io::Write;

/// `{"schema_version":1,"update":{"event":"error","message":…}}`.
pub fn error(message: &str) -> Value {
    json!({"schema_version": 1, "update": {"event": "error", "message": message}})
}

/// Prints `event` on stdout and flushes it.
pub fn emit(json_mode: bool, event: &Value) {
    let line = if json_mode {
        event.to_string()
    } else {
        text(event)
    };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// The text line of an event.
pub fn text(event: &Value) -> String {
    let u = &event["update"];
    let s = |key: &str| u[key].as_str().unwrap_or("?").to_owned();
    match u["event"].as_str() {
        Some("restarting") => format!(
            "update: restarting onto {} (was {}, pid {})",
            s("to"),
            s("from"),
            u["pid"]
        ),
        Some("available") => {
            let mut line = format!(
                "update: mailtriage {} is available (running {}): {}",
                s("latest"),
                s("current"),
                s("release_url")
            );
            if let Some(fix) = u["install"]["fix"].as_str() {
                line.push_str(&format!("; {fix}"));
            }
            line
        }
        _ => format!("update error: {}", s("message")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_have_one_line_of_text() {
        assert_eq!(text(&error("boom")), "update error: boom");
        let restarting = json!({"schema_version":1,"update":{"event":"restarting","pid":12,"from":"0.2.0","to":"0.3.0"}});
        assert_eq!(
            text(&restarting),
            "update: restarting onto 0.3.0 (was 0.2.0, pid 12)"
        );
        let available = json!({"schema_version":1,"update":{"event":"available","current":"0.2.0","latest":"0.3.0","release_url":"https://x","install":{"reason":"managed_by_homebrew","fix":"run `brew upgrade mailtriage`"}}});
        assert_eq!(
            text(&available),
            "update: mailtriage 0.3.0 is available (running 0.2.0): https://x; run `brew upgrade mailtriage`"
        );
        assert!(!text(&available).contains('\n'));
    }
}
