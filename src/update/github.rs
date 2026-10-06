//! GitHub access: the URL rules every request and redirect passes, the
//! paginated release list, and bounded downloads. No token is sent.
use super::version;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::{
    blocking::{Client, Response},
    header::{HeaderMap, ACCEPT},
    redirect, Url,
};
use serde::Deserialize;
use std::{fmt, io::Read, time::Duration};

/// The API base URL.
pub const API_BASE: &str = "https://api.github.com";
/// The hidden variable that replaces `API_BASE` in tests; honoured only for
/// a loopback host.
pub const URL_OVERRIDE: &str = "MAILTRIAGE_UPDATE_URL";
/// The release list is read page by page; more pages fail the check.
pub const MAX_PAGES: usize = 20;
/// The largest asset (listed `size` or actual body) a download accepts.
pub const MAX_ASSET_BYTES: u64 = 200 * 1024 * 1024;
/// The largest `SHA256SUMS` accepted.
pub const MAX_SUMS_BYTES: u64 = 1024 * 1024;
const MAX_PAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_REDIRECTS: usize = 10;
const API_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Where release information comes from.
#[derive(Debug, Clone)]
pub struct Endpoint {
    api_base: Url,
    /// Set only when `MAILTRIAGE_UPDATE_URL` names a loopback host: then
    /// plain HTTP and that origin are allowed too.
    loopback: Option<Url>,
}

impl Endpoint {
    pub fn github() -> Self {
        Self {
            api_base: Url::parse(API_BASE).expect("API_BASE is a URL"),
            loopback: None,
        }
    }

    /// GitHub, or `MAILTRIAGE_UPDATE_URL` when it names 127.0.0.1, ::1 or
    /// localhost.
    pub fn from_env() -> Self {
        Self::with_override(std::env::var(URL_OVERRIDE).ok().as_deref())
    }

    pub fn with_override(value: Option<&str>) -> Self {
        match value.and_then(|v| Url::parse(v).ok()).filter(is_loopback) {
            Some(url) => Self {
                api_base: url.clone(),
                loopback: Some(url),
            },
            None => Self::github(),
        }
    }

    /// The first page of the release list.
    pub fn releases_url(&self) -> Url {
        let mut url = self.api_base.clone();
        let base = self.api_base.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{base}/repos/{}/releases", super::REPO));
        url.set_query(Some("per_page=30"));
        url
    }

    /// Whether a request, or a redirect, may go to `url`.
    pub fn allows(&self, url: &Url) -> bool {
        github_url(url)
            || self.loopback.as_ref().is_some_and(|base| {
                url.scheme() == base.scheme()
                    && url.host_str() == base.host_str()
                    && url.port_or_known_default() == base.port_or_known_default()
                    && url.username().is_empty()
                    && url.password().is_none()
            })
    }
}

fn is_loopback(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))
}

/// GitHub's own URLs: HTTPS on port 443 without credentials, to
/// `github.com` or a host ending in `.github.com` or
/// `.githubusercontent.com`.
pub fn github_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| {
            host == "github.com"
                || host.ends_with(".github.com")
                || host.ends_with(".githubusercontent.com")
        })
}

/// A failed check, with the server's hints for when to retry.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckError {
    pub message: String,
    /// From `Retry-After`.
    pub retry_after: Option<DateTime<Utc>>,
    /// From `X-RateLimit-Reset` when `X-RateLimit-Remaining` is 0.
    pub rate_reset: Option<DateTime<Utc>>,
}

impl CheckError {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_after: None,
            rate_reset: None,
        }
    }
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CheckError {}

/// One entry of the release list; unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseJson {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<AssetJson>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetJson {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

/// An HTTP client that follows only allowed redirects, at most 10.
pub struct Net {
    endpoint: Endpoint,
    client: Client,
}

impl Net {
    pub fn new(endpoint: Endpoint) -> anyhow::Result<Self> {
        let rules = endpoint.clone();
        let policy = redirect::Policy::custom(move |attempt| {
            // `previous` holds the first URL too, so the 11th redirect sees 11.
            if attempt.previous().len() > MAX_REDIRECTS {
                attempt.error("more than 10 redirects")
            } else if rules.allows(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("a redirect left GitHub")
            }
        });
        let mut builder = Client::builder()
            .redirect(policy)
            .user_agent(format!("mailtriage/{}", version::RUNNING));
        if endpoint.loopback.is_some() {
            builder = builder.no_proxy();
        }
        Ok(Self {
            endpoint,
            client: builder.build()?,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Every page of the release list, following `Link: rel="next"`. Any
    /// failing page, a page URL outside the rules, or more than 20 pages
    /// fails the whole list.
    pub fn releases(&self) -> Result<Vec<ReleaseJson>, CheckError> {
        let mut url = self.endpoint.releases_url();
        let mut all = Vec::new();
        for _ in 0..MAX_PAGES {
            let (page, next) = self.page(&url)?;
            all.extend(page);
            match next {
                Some(next) => url = next,
                None => return Ok(all),
            }
        }
        Err(CheckError::plain(format!(
            "the release list has more than {MAX_PAGES} pages"
        )))
    }

    fn page(&self, url: &Url) -> Result<(Vec<ReleaseJson>, Option<Url>), CheckError> {
        if !self.endpoint.allows(url) {
            return Err(CheckError::plain("a release list page is outside GitHub"));
        }
        let response = self
            .client
            .get(url.clone())
            .timeout(API_TIMEOUT)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .map_err(|e| {
                CheckError::plain(format!("cannot read the release list: {}", describe(&e)))
            })?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers();
            let rate_reset = rate_reset(headers);
            let what = if rate_reset.is_some() || status.as_u16() == 429 {
                "GitHub's rate limit was reached".to_owned()
            } else {
                format!("GitHub answered {}", status.as_u16())
            };
            return Err(CheckError {
                message: format!("cannot read the release list: {what}"),
                retry_after: retry_after(headers, Utc::now()),
                rate_reset,
            });
        }
        let next = response
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .and_then(next_link)
            .map(|link| Url::parse(&link))
            .transpose()
            .map_err(|_| CheckError::plain("GitHub sent an invalid next-page link"))?;
        let body = read_capped(response, MAX_PAGE_BYTES)
            .map_err(|e| CheckError::plain(format!("cannot read the release list: {e}")))?;
        let releases = serde_json::from_slice(&body)
            .map_err(|_| CheckError::plain("GitHub sent a release list mailtriage cannot read"))?;
        Ok((releases, next))
    }

    /// Downloads `url` into memory, refusing a listed or actual size over
    /// `max` bytes. 300 s for the whole download.
    pub fn download(&self, url: &str, listed_size: u64, max: u64) -> anyhow::Result<Vec<u8>> {
        if listed_size > max {
            anyhow::bail!("it is larger than {}", size_text(max));
        }
        let url = Url::parse(url).map_err(|_| anyhow::anyhow!("its URL is invalid"))?;
        if !self.endpoint.allows(&url) {
            anyhow::bail!("its URL is outside GitHub");
        }
        let response = self
            .client
            .get(url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .map_err(|e| anyhow::anyhow!("{}", describe(&e)))?;
        if !response.status().is_success() {
            anyhow::bail!("GitHub answered {}", response.status().as_u16());
        }
        read_capped(response, max).map_err(anyhow::Error::msg)
    }
}

/// The body, refused once it exceeds `max` bytes.
fn read_capped(response: Response, max: u64) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    response
        .take(max + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("the download broke off ({e})"))?;
    if body.len() as u64 > max {
        return Err(format!("it is larger than {}", size_text(max)));
    }
    Ok(body)
}

/// `200 MB`, or bytes below one megabyte.
fn size_text(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else {
        format!("{bytes} bytes")
    }
}

/// A short reason for a failed request, without URLs or headers.
fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "timed out".into();
    }
    if error.is_redirect() {
        let mut source = std::error::Error::source(error);
        while let Some(inner) = source {
            if inner.source().is_none() {
                return inner.to_string();
            }
            source = inner.source();
        }
        return "a redirect was refused".into();
    }
    "cannot connect".into()
}

/// The target of the `rel="next"` entry of a `Link` header.
pub fn next_link(value: &str) -> Option<String> {
    value.split(',').find_map(|part| {
        let (target, params) = part.trim().split_once(';')?;
        let target = target.trim().strip_prefix('<')?.strip_suffix('>')?;
        let next = params.split(';').any(|param| {
            param.split_once('=').is_some_and(|(key, value)| {
                key.trim().eq_ignore_ascii_case("rel")
                    && value
                        .trim()
                        .trim_matches('"')
                        .split_ascii_whitespace()
                        .any(|rel| rel.eq_ignore_ascii_case("next"))
            })
        });
        next.then(|| target.to_owned())
    })
}

/// `Retry-After` as seconds or an HTTP date.
pub fn retry_after(headers: &HeaderMap, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let value = headers.get("retry-after")?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<i64>() {
        return Some(now + chrono::Duration::seconds(seconds.clamp(0, 365 * 86_400)));
    }
    DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// `X-RateLimit-Reset` (epoch seconds) when `X-RateLimit-Remaining` is 0.
pub fn rate_reset(headers: &HeaderMap) -> Option<DateTime<Utc>> {
    let header = |name: &str| headers.get(name)?.to_str().ok()?.trim().parse::<i64>().ok();
    if header("x-ratelimit-remaining")? != 0 {
        return None;
    }
    Utc.timestamp_opt(header("x-ratelimit-reset")?, 0).single()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn github_hosts_over_https_are_allowed() {
        let github = Endpoint::github();
        for ok in [
            "https://api.github.com/repos/x/y/releases?per_page=30",
            "https://github.com/wir-drei-digital/mailtriage/releases/download/v1.0.0/a.tar.gz",
            "https://objects.githubusercontent.com/x",
            "https://release-assets.githubusercontent.com/x",
            "https://github.com:443/x",
        ] {
            assert!(github.allows(&url(ok)), "{ok}");
        }
        for refused in [
            "https://github.com.evil.example/x",
            "https://evilgithubusercontent.com/x",
            "https://evil.example/github.com",
            "http://github.com/x",
            "http://objects.githubusercontent.com/x",
            "https://github.com:8443/x",
            "https://user@github.com/x",
            "https://user:pw@api.github.com/x",
            "ftp://github.com/x",
            "https://140.82.112.3/x",
        ] {
            assert!(!github.allows(&url(refused)), "{refused}");
        }
    }

    #[test]
    fn the_override_is_honoured_only_for_loopback_hosts() {
        for base in [
            "http://127.0.0.1:8080",
            "http://[::1]:9",
            "http://localhost:1",
        ] {
            let endpoint = Endpoint::with_override(Some(base));
            assert!(
                endpoint.allows(&url(&format!("{base}/download/a"))),
                "{base}"
            );
            assert!(endpoint.releases_url().as_str().starts_with(base), "{base}");
            // GitHub stays allowed; another loopback port does not.
            assert!(endpoint.allows(&url("https://objects.githubusercontent.com/x")));
            assert!(!endpoint.allows(&url("http://127.0.0.1:2/x")), "{base}");
        }
        for refused in [
            "http://example.com:8080",
            "http://10.0.0.1",
            "file:///tmp/x",
            "nonsense",
        ] {
            let endpoint = Endpoint::with_override(Some(refused));
            assert_eq!(
                endpoint.releases_url().as_str(),
                "https://api.github.com/repos/wir-drei-digital/mailtriage/releases?per_page=30"
            );
            assert!(!endpoint.allows(&url("http://127.0.0.1:8080/x")));
        }
        assert_eq!(
            Endpoint::github().releases_url().as_str(),
            "https://api.github.com/repos/wir-drei-digital/mailtriage/releases?per_page=30"
        );
    }

    #[test]
    fn the_next_link_is_found_among_others() {
        let header = r#"<https://api.github.com/r?per_page=30&page=1>; rel="prev", <https://api.github.com/r?per_page=30&page=3>; rel="next", <https://api.github.com/r?per_page=30&page=9>; rel="last""#;
        assert_eq!(
            next_link(header).as_deref(),
            Some("https://api.github.com/r?per_page=30&page=3")
        );
        assert_eq!(next_link(r#"<https://x/a>; rel="last""#), None);
        assert_eq!(
            next_link("<https://x/a>; rel=next").as_deref(),
            Some("https://x/a")
        );
        assert_eq!(next_link(""), None);
    }

    #[test]
    fn retry_hints_come_from_their_headers() {
        let now = Utc.with_ymd_and_hms(2026, 11, 3, 8, 0, 0).unwrap();
        let mut headers = HeaderMap::new();
        assert_eq!(retry_after(&headers, now), None);
        headers.insert("retry-after", HeaderValue::from_static("120"));
        assert_eq!(
            retry_after(&headers, now),
            Some(now + chrono::Duration::seconds(120))
        );
        headers.insert(
            "retry-after",
            HeaderValue::from_static("Tue, 03 Nov 2026 10:00:00 GMT"),
        );
        assert_eq!(
            retry_after(&headers, now),
            Some(Utc.with_ymd_and_hms(2026, 11, 3, 10, 0, 0).unwrap())
        );
        headers.insert("x-ratelimit-reset", HeaderValue::from_static("1793700000"));
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("3"));
        assert_eq!(rate_reset(&headers), None);
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        assert_eq!(
            rate_reset(&headers),
            Utc.timestamp_opt(1_793_700_000, 0).single()
        );
    }
}
