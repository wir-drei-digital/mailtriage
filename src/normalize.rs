use crate::domain::{Address, NormalizedMessage};
use anyhow::{bail, Context, Result};
use mail_parser::{Address as MailAddress, MessageParser};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn truncate_body(body: String, max_chars: usize, warnings: &mut Vec<String>) -> (String, bool) {
    if body.chars().count() <= max_chars {
        return (body, false);
    }
    warnings.push("body_truncated".into());
    (body.chars().take(max_chars).collect(), true)
}

fn addresses(address: Option<&MailAddress<'_>>) -> Vec<Address> {
    address
        .into_iter()
        .flat_map(|a| a.iter())
        .filter_map(|a| {
            a.address().map(|email| Address {
                email: email.trim().to_string(),
                name: a.name().map(str::to_string),
            })
        })
        .filter(|a| !a.email.is_empty())
        .collect()
}

pub fn rfc822(bytes: &[u8], max_body_chars: usize) -> Result<NormalizedMessage> {
    if bytes.is_empty() {
        bail!("empty RFC822 message");
    }
    let message = MessageParser::default()
        .parse(bytes)
        .context("could not parse RFC822 message")?;
    if message.is_empty() {
        bail!("RFC822 message has no valid headers");
    }
    let mut warnings = Vec::new();
    let mut bodies = Vec::new();
    for index in 0..message.text_body_count() {
        if let Some(text) = message.body_text(index) {
            bodies.push(text.into_owned());
        }
    }
    if bodies.is_empty() {
        for index in 0..message.html_body_count() {
            if let Some(html) = message.body_html(index) {
                match html2text::from_read(html.as_bytes(), 120) {
                    Ok(text) => bodies.push(text),
                    Err(_) => warnings.push("html_body_unreadable".into()),
                }
            }
        }
    }
    let body = bodies.join("\n\n").trim().to_owned();
    let missing_body = body.is_empty();
    if missing_body {
        warnings.push("body_missing".into());
    }
    let (body, truncated) = truncate_body(body, max_body_chars, &mut warnings);
    Ok(NormalizedMessage {
        raw_sha256: hash(bytes),
        from: addresses(message.from()),
        to: addresses(message.to()),
        cc: addresses(message.cc()),
        subject: message.subject().unwrap_or("").trim().to_owned(),
        sent_at: message.date().map(|d| d.to_rfc3339()),
        body,
        incomplete: missing_body || truncated,
        warnings,
    })
}

fn string_field(value: &Value, field: &str) -> Result<Option<String>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        _ => bail!("JSON field {field} must be a string"),
    }
}

fn json_address(value: &Value) -> Result<Address> {
    match value {
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                bail!("empty JSON address");
            }
            if let Some((name, rest)) = s.rsplit_once('<') {
                if let Some(email) = rest.strip_suffix('>') {
                    return Ok(Address {
                        email: email.trim().into(),
                        name: Some(name.trim().trim_matches('"').into()),
                    });
                }
            }
            Ok(Address {
                email: s.into(),
                name: None,
            })
        }
        Value::Object(_) => {
            let email = string_field(value, "email")?.context("JSON address missing email")?;
            if email.trim().is_empty() {
                bail!("empty JSON address email");
            }
            Ok(Address {
                email: email.trim().into(),
                name: string_field(value, "name")?,
            })
        }
        _ => bail!("JSON address must be a string or object"),
    }
}

fn json_addresses(value: &Value, field: &str) -> Result<Vec<Address>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => values.iter().map(json_address).collect(),
        Some(one) => Ok(vec![json_address(one)?]),
    }
}

pub fn json(bytes: &[u8], max_body_chars: usize) -> Result<NormalizedMessage> {
    let value: Value = serde_json::from_slice(bytes).context("parse message JSON")?;
    if !value.is_object() {
        bail!("message JSON must be an object");
    }
    let mut warnings = Vec::new();
    let body = string_field(&value, "body")?
        .or(string_field(&value, "text_body")?)
        .unwrap_or_default();
    let missing_body = body.trim().is_empty();
    if missing_body {
        warnings.push("body_missing".into());
    }
    let (body, truncated) = truncate_body(body.trim().to_owned(), max_body_chars, &mut warnings);
    let supplied_incomplete = match value.get("incomplete") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        _ => bail!("JSON field incomplete must be a boolean"),
    };
    if supplied_incomplete {
        warnings.push("source_incomplete".into());
    }
    Ok(NormalizedMessage {
        raw_sha256: hash(bytes),
        from: json_addresses(&value, "from")?,
        to: json_addresses(&value, "to")?,
        cc: json_addresses(&value, "cc")?,
        subject: string_field(&value, "subject")?.unwrap_or_default(),
        sent_at: string_field(&value, "sent_at")?.or(string_field(&value, "date")?),
        body,
        incomplete: missing_body || truncated || supplied_incomplete,
        warnings,
    })
}
