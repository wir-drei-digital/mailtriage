//! Strict, minimal parser for `himalaya imap raw` output. It reads tagged
//! completions, UIDVALIDITY, COPYUID, CAPABILITY, NAMESPACE and LIST lines and
//! ignores everything else.
use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    Ok,
    No,
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyUid {
    pub target_epoch: u64,
    /// `(source, target)` UIDs.
    pub pairs: Vec<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListLine {
    pub attributes: Vec<String>,
    pub delimiter: Option<char>,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct RawResponse {
    pub tagged: Vec<(String, Completion)>,
    pub uidvalidity: Option<u64>,
    pub copyuid: Option<CopyUid>,
    pub capabilities: Vec<String>,
    pub personal_namespace: Option<(String, Option<char>)>,
    pub list: Vec<ListLine>,
    pub list_errors: usize,
}

impl RawResponse {
    pub fn completion(&self, tag: &str) -> Option<Completion> {
        self.tagged.iter().find(|(t, _)| t == tag).map(|(_, c)| *c)
    }
}

const MAX_UID_SET: usize = 10_000;

pub fn parse(output: &[u8]) -> RawResponse {
    let text = String::from_utf8_lossy(output);
    let mut r = RawResponse::default();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("* ") {
            untagged(rest, &mut r);
        } else if let Some((tag, rest)) = line.split_once(' ') {
            if tag.is_empty() || tag == "+" || !tag.chars().all(|c| c.is_ascii_alphanumeric()) {
                continue;
            }
            let (status, tail) = rest.split_once(' ').unwrap_or((rest, ""));
            let completion = match status.to_ascii_uppercase().as_str() {
                "OK" => Completion::Ok,
                "NO" => Completion::No,
                "BAD" => Completion::Bad,
                _ => continue,
            };
            r.tagged.push((tag.to_string(), completion));
            response_code(tail, &mut r);
        }
    }
    r
}

fn untagged(rest: &str, r: &mut RawResponse) {
    let upper = rest.to_ascii_uppercase();
    for prefix in ["OK ", "NO ", "BAD "] {
        if upper.starts_with(prefix) {
            response_code(&rest[prefix.len()..], r);
            return;
        }
    }
    if let Some(caps) = upper.strip_prefix("CAPABILITY ") {
        r.capabilities = caps.split_ascii_whitespace().map(str::to_string).collect();
    } else if upper.starts_with("NAMESPACE ") {
        r.personal_namespace = namespace(&rest["NAMESPACE ".len()..]);
    } else if upper.starts_with("LIST ") {
        match list_line(&rest["LIST ".len()..]) {
            Some(line) => r.list.push(line),
            None => r.list_errors += 1,
        }
    }
}

fn response_code(tail: &str, r: &mut RawResponse) {
    let Some(start) = tail.find('[') else { return };
    let Some(len) = tail[start..].find(']') else {
        return;
    };
    let mut parts = tail[start + 1..start + len].split_ascii_whitespace();
    match parts.next().map(str::to_ascii_uppercase).as_deref() {
        Some("UIDVALIDITY") => r.uidvalidity = parts.next().and_then(|v| v.parse().ok()),
        Some("CAPABILITY") => {
            r.capabilities = parts.map(str::to_ascii_uppercase).collect();
        }
        Some("COPYUID") => {
            let (Some(epoch), Some(src), Some(dst)) = (parts.next(), parts.next(), parts.next())
            else {
                return;
            };
            let (Ok(epoch), Ok(src), Ok(dst)) =
                (epoch.parse::<u64>(), parse_uid_set(src), parse_uid_set(dst))
            else {
                return;
            };
            if src.len() != dst.len() {
                return;
            }
            let pairs = src.into_iter().zip(dst);
            match &mut r.copyuid {
                Some(existing) if existing.target_epoch == epoch => existing.pairs.extend(pairs),
                Some(_) => {}
                None => {
                    r.copyuid = Some(CopyUid {
                        target_epoch: epoch,
                        pairs: pairs.collect(),
                    })
                }
            }
        }
        _ => {}
    }
}

/// Parses a quoted string with `\"` and `\\` escapes; returns it and the rest.
fn quoted(s: &str) -> Option<(String, &str)> {
    let body = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        if escaped {
            out.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Some((out, &body[i + 1..]));
        } else {
            out.push(c);
        }
    }
    None
}

fn namespace(s: &str) -> Option<(String, Option<char>)> {
    let s = s.trim_start();
    let first = s.strip_prefix("((")?;
    let (prefix, rest) = quoted(first.trim_start())?;
    let rest = rest.trim_start();
    let delimiter = if rest.starts_with("NIL") {
        None
    } else {
        quoted(rest)?.0.chars().next()
    };
    Some((prefix, delimiter))
}

fn list_line(s: &str) -> Option<ListLine> {
    let s = s.trim();
    let inner = s.strip_prefix('(')?;
    let close = inner.find(')')?;
    let attributes = inner[..close]
        .split_ascii_whitespace()
        .map(str::to_string)
        .collect();
    let rest = inner[close + 1..].trim_start();
    let (delimiter, rest) = match rest.strip_prefix("NIL") {
        Some(rest) => (None, rest),
        None => {
            let (d, rest) = quoted(rest)?;
            (d.chars().next(), rest)
        }
    };
    let rest = rest.trim();
    let name = if rest.starts_with('"') {
        quoted(rest)?.0
    } else if rest.starts_with('{') || rest.is_empty() {
        return None;
    } else {
        rest.split_ascii_whitespace().next()?.to_string()
    };
    Some(ListLine {
        attributes,
        delimiter,
        name,
    })
}

/// A mailbox name as an IMAP quoted string for raw IMAP text. A backslash is
/// refused, not escaped: Himalaya's `imap raw` turns `\n` and `\r` into line
/// breaks even after an escaping backslash.
pub fn quote_mailbox(name: &str) -> Result<String> {
    if name.is_empty()
        || !name.chars().all(|c| (' '..='~').contains(&c))
        || name.contains(['&', '\\'])
        || name.starts_with('-')
    {
        bail!(
            "mailbox names must be nonempty printable ASCII without '&' or '\\' or a leading '-'"
        );
    }
    Ok(format!("\"{}\"", name.replace('"', "\\\"")))
}

pub fn uid_set(uids: &[u64]) -> String {
    uids.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

pub fn parse_uid_set(s: &str) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let (a, b) = part.split_once(':').unwrap_or((part, part));
        let (a, b): (u64, u64) = (a.parse()?, b.parse()?);
        let (lo, hi) = (a.min(b), a.max(b));
        // Saturating: a server-supplied range must not overflow the size check.
        if (hi - lo).saturating_add(out.len() as u64) >= MAX_UID_SET as u64 {
            bail!("UID set too large");
        }
        out.extend(lo..=hi);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_and_move_with_copyuid() {
        let out = b"* 3 EXISTS\r\n* OK [UIDVALIDITY 7] UIDs valid\r\na1 OK [READ-WRITE] done\r\n* OK [COPYUID 9 4,5 20:21] moved\r\n* 1 EXPUNGE\r\n* 1 EXPUNGE\r\na2 OK MOVE done\r\n";
        let r = parse(out);
        assert_eq!(r.completion("a1"), Some(Completion::Ok));
        assert_eq!(r.completion("a2"), Some(Completion::Ok));
        assert_eq!(r.uidvalidity, Some(7));
        assert_eq!(
            r.copyuid,
            Some(CopyUid {
                target_epoch: 9,
                pairs: vec![(4, 20), (5, 21)]
            })
        );
    }

    #[test]
    fn copyuid_in_tagged_response_and_partial_no() {
        let r = parse(b"* OK [UIDVALIDITY 7] x\r\na1 OK x\r\na2 NO [COPYUID 9 4 30] partial\r\n");
        assert_eq!(r.completion("a2"), Some(Completion::No));
        assert_eq!(r.copyuid.unwrap().pairs, vec![(4, 30)]);
    }

    #[test]
    fn multiple_copyuid_responses_accumulate() {
        let r = parse(b"* OK [COPYUID 9 1:2 10:11] a\r\n* OK [COPYUID 9 5 12] b\r\na2 OK\r\n");
        assert_eq!(r.copyuid.unwrap().pairs, vec![(1, 10), (2, 11), (5, 12)]);
    }

    #[test]
    fn mismatched_copyuid_sets_are_ignored() {
        assert!(parse(b"* OK [COPYUID 9 1:3 10:11] bad\r\n")
            .copyuid
            .is_none());
    }

    #[test]
    fn bad_completion_and_missing_uidvalidity() {
        let r = parse(b"a1 BAD no mailbox\r\na2 BAD no mailbox selected\r\n");
        assert_eq!(r.completion("a1"), Some(Completion::Bad));
        assert_eq!(r.uidvalidity, None);
    }

    #[test]
    fn capability_and_namespace() {
        let r = parse(b"* CAPABILITY IMAP4rev1 MOVE UIDPLUS SPECIAL-USE\r\na1 OK\r\n* NAMESPACE ((\"INBOX.\" \".\")) NIL NIL\r\na2 OK\r\n");
        assert!(r.capabilities.contains(&"MOVE".to_string()));
        assert!(r.capabilities.contains(&"SPECIAL-USE".to_string()));
        assert_eq!(
            r.personal_namespace,
            Some(("INBOX.".to_string(), Some('.')))
        );
        let r = parse(b"* NAMESPACE NIL NIL NIL\r\n");
        assert_eq!(r.personal_namespace, None);
        let r = parse(b"* NAMESPACE ((\"\" \"/\")) NIL NIL\r\n");
        assert_eq!(r.personal_namespace, Some((String::new(), Some('/'))));
    }

    #[test]
    fn list_lines_quoted_atom_nil_and_literal() {
        let r = parse(b"* LIST (\\HasNoChildren \\Sent) \"/\" \"Sent Items\"\r\n* LIST () \".\" INBOX\r\n* LIST (\\Noselect) NIL \"\"\r\n* LIST () \"/\" {5}\r\nHello\r\na1 OK\r\n");
        assert_eq!(
            r.list[0],
            ListLine {
                attributes: vec!["\\HasNoChildren".into(), "\\Sent".into()],
                delimiter: Some('/'),
                name: "Sent Items".into()
            }
        );
        assert_eq!(r.list[1].name, "INBOX");
        assert_eq!(r.list[2].delimiter, None);
        assert_eq!(r.list_errors, 1);
    }

    #[test]
    fn quoting_and_uid_sets() {
        assert_eq!(
            quote_mailbox("Bills and Receipts").unwrap(),
            "\"Bills and Receipts\""
        );
        assert!(
            quote_mailbox("A&B").is_err(),
            "& is the modified UTF-7 shift character"
        );
        assert!(quote_mailbox("Grüße").is_err());
        assert!(quote_mailbox("a\r\nb").is_err());
        assert!(quote_mailbox("").is_err());
        assert!(quote_mailbox("-x").is_err());
        assert_eq!(uid_set(&[4, 5, 9]), "4,5,9");
        assert_eq!(parse_uid_set("1:3,7").unwrap(), vec![1, 2, 3, 7]);
        assert_eq!(parse_uid_set("3:1").unwrap(), vec![1, 2, 3]);
        assert!(parse_uid_set("1:200000").is_err());
        assert!(parse_uid_set("x").is_err());
    }

    /// Final review M1: Himalaya's `imap raw` turns `\n` and `\r` into line
    /// breaks even after an escaping backslash, so no backslash may reach
    /// raw IMAP text; a `"` is still escaped.
    #[test]
    fn backslashes_are_refused_in_mailbox_names() {
        for name in ["a\\nb", "a\\rb", "x\\", "\\"] {
            assert!(quote_mailbox(name).is_err(), "accepted {name:?}");
        }
        assert_eq!(quote_mailbox("a\"b").unwrap(), "\"a\\\"b\"");
    }

    #[test]
    fn huge_uid_ranges_are_rejected_without_overflow() {
        assert!(parse_uid_set("1,0:18446744073709551615").is_err());
        assert!(
            parse(b"* OK [COPYUID 9 1,0:18446744073709551615 1,0:18446744073709551615] x\r\n")
                .copyuid
                .is_none()
        );
    }
}
