//! One Nextcloud account over WebDAV, on loopback. It sends PROPFIND and GET,
//! and PUT and MKCOL for `Tree::write` only: never DELETE, MOVE or COPY.
//!
//! After one refusal it sends nothing more: each failed login counts against
//! every app on loopback. `refused` carries that across runs through `mark`,
//! a short hash of the password, kept in the status file by the indexer.

use std::{
    io::Read,
    net::SocketAddr,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use reqwest::{Method, blocking::Client};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::folders::Error;

const REFUSED: &str = "Nextcloud refused the app password; it is not tried again until a new one is saved in the config panel.";
/// Nextcloud counts failed logins per address, and the units all come from 127.0.0.1.
const THROTTLED: &str = "Nextcloud is holding back logins from this server after failed ones; \
                         as root, occ security:bruteforce:reset 127.0.0.1 clears it.";
const PROPFIND: &str = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/><d:getlastmodified/><d:getetag/></d:prop></d:propfind>"#;
/// What Python's urllib.parse.quote(safe="") leaves as it is.
const QUOTED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// One file or folder of the account, by its path in it: "" is the account.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    pub is_dir: bool,
    pub size: i64,
    pub mtime_ns: i64,
    pub etag: String,
}

impl Entry {
    /// As the index keeps it: [path, is_dir, size, mtime_ns, etag].
    pub fn to_json(&self) -> Value {
        json!([self.path, self.is_dir, self.size, self.mtime_ns, self.etag])
    }

    pub fn from_json(v: &Value) -> Option<Entry> {
        Some(Entry {
            path: v[0].as_str()?.to_string(),
            is_dir: v[1].as_bool()?,
            size: v[2].as_i64()?,
            mtime_ns: v[3].as_i64()?,
            etag: v[4].as_str()?.to_string(),
        })
    }
}

/// A path in the account as a URL path, relative to its WebDAV root.
fn quote(path: &str) -> String {
    path.split('/')
        .filter(|p| !p.is_empty())
        .map(|p| utf8_percent_encode(p, QUOTED).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn unreachable(e: impl std::fmt::Display) -> Error {
    Error::Nextcloud {
        message: format!("Nextcloud did not answer: {e}"),
        refused: false,
    }
}

fn answered(code: u16) -> Error {
    Error::Nextcloud {
        message: format!("Nextcloud answered {code}."),
        refused: false,
    }
}

pub struct Nextcloud {
    client: Client,
    /// The WebDAV root as reached, ending in "/".
    url: String,
    /// The root's path, without its trailing "/", as hrefs start with it.
    base: String,
    user: String,
    password: String,
    pub mark: String,
    pub refused: AtomicBool,
}

impl Nextcloud {
    /// url is the account's WebDAV root as reached, such as
    /// https://127.0.0.1/remote.php/dav/files/<user>; host, when set, is the
    /// name Nextcloud answers to, sent in the URL and checked on its
    /// certificate, while the connection goes to url's address.
    pub fn new(url: &str, host: &str, user: &str, password: &str) -> Result<Self, String> {
        let parsed = reqwest::Url::parse(url).map_err(|e| format!("DOCS_NEXTCLOUD_URL: {e}"))?;
        let base = percent_decode_str(parsed.path())
            .decode_utf8_lossy()
            .trim_end_matches('/')
            .to_string();
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(60))
            .no_proxy();
        let mut reached = parsed.clone();
        if !host.is_empty() {
            let ip = parsed
                .host_str()
                .unwrap_or("127.0.0.1")
                .trim_matches(['[', ']']);
            let port = parsed.port_or_known_default().unwrap_or(443);
            let addr: SocketAddr = format!("{ip}:{port}")
                .parse()
                .or_else(|_| format!("[{ip}]:{port}").parse())
                .map_err(|e| format!("DOCS_NEXTCLOUD_URL: {e}"))?;
            builder = builder.resolve(host, addr);
            reached
                .set_host(Some(host))
                .map_err(|e| format!("DOCS_NEXTCLOUD_HOST: {e}"))?;
        }
        let client = builder.build().map_err(|e| e.to_string())?;
        let mark = hex(&Sha256::digest(password.as_bytes()))[..16].to_string();
        Ok(Nextcloud {
            client,
            url: format!("{}/", reached.as_str().trim_end_matches('/')),
            base,
            user: user.to_string(),
            password: password.to_string(),
            mark,
            refused: AtomicBool::new(false),
        })
    }

    /// The account in DOCS_NEXTCLOUD_URL, _HOST, _USER and _PASSWORD, or None.
    pub fn from_env() -> Result<Option<Self>, String> {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        if var("DOCS_NEXTCLOUD_URL").is_empty() {
            return Ok(None);
        }
        Self::new(
            &var("DOCS_NEXTCLOUD_URL"),
            &var("DOCS_NEXTCLOUD_HOST"),
            &var("DOCS_NEXTCLOUD_USER"),
            &var("DOCS_NEXTCLOUD_PASSWORD"),
        )
        .map(Some)
    }

    pub fn is_refused(&self) -> bool {
        self.refused.load(Ordering::Relaxed)
    }

    fn request(
        &self,
        method: &str,
        path: &str,
    ) -> Result<reqwest::blocking::RequestBuilder, Error> {
        if self.is_refused() {
            return Err(Error::Nextcloud {
                message: REFUSED.into(),
                refused: true,
            });
        }
        let method = Method::from_bytes(method.as_bytes()).map_err(unreachable)?;
        Ok(self
            .client
            .request(method, format!("{}{}", self.url, quote(path)))
            .basic_auth(&self.user, Some(&self.password)))
    }

    /// The response, or an error for a refused password or a throttled server.
    fn answer(
        &self,
        resp: reqwest::blocking::Response,
    ) -> Result<reqwest::blocking::Response, Error> {
        match resp.status().as_u16() {
            401 => {
                self.refused.store(true, Ordering::Relaxed);
                Err(Error::Nextcloud {
                    message: REFUSED.into(),
                    refused: true,
                })
            }
            429 => Err(Error::Nextcloud {
                message: THROTTLED.into(),
                refused: false,
            }),
            _ => Ok(resp),
        }
    }

    /// [Entry] of path then, at depth 1, what it holds; None when it does not exist.
    pub fn list(&self, path: &str, depth: u8) -> Result<Option<Vec<Entry>>, Error> {
        let resp = self
            .request("PROPFIND", path)?
            .header("Depth", depth.to_string())
            .header("Content-Type", "application/xml")
            .body(PROPFIND)
            .send()
            .map_err(unreachable)?;
        let resp = self.answer(resp)?;
        match resp.status().as_u16() {
            404 => return Ok(None),
            207 => {}
            code => return Err(answered(code)),
        }
        let body = resp.text().map_err(unreachable)?;
        let unreadable = || Error::Nextcloud {
            message: "Nextcloud's answer could not be read.".into(),
            refused: false,
        };
        let responses = parse_multistatus(&body).ok_or_else(unreadable)?;
        let mut entries: Vec<Entry> = Vec::new();
        for r in responses {
            let href = percent_decode_str(href_path(&r.href))
                .decode_utf8_lossy()
                .into_owned();
            if href.trim_end_matches('/') != self.base
                && !href.starts_with(&format!("{}/", self.base))
            {
                continue;
            }
            let rel = href[self.base.len()..].trim_matches('/').to_string();
            let mtime_ns = r
                .modified
                .as_deref()
                .and_then(|m| chrono::DateTime::parse_from_rfc2822(m).ok())
                .and_then(|d| d.timestamp_nanos_opt())
                .unwrap_or(0);
            let entry = Entry {
                path: rel,
                is_dir: r.collection,
                size: r.size,
                mtime_ns,
                etag: r.etag,
            };
            entries.retain(|e| e.path != entry.path);
            entries.push(entry);
        }
        let wanted = path.trim_matches('/');
        let Some(at) = entries.iter().position(|e| e.path == wanted) else {
            let what = if path.is_empty() { "the account" } else { path };
            return Err(Error::Nextcloud {
                message: format!("Nextcloud did not describe {what}."),
                refused: false,
            });
        };
        let here = entries.remove(at);
        entries.sort_by(|a, b| {
            (&a.path, a.is_dir, a.size, a.mtime_ns, &a.etag)
                .cmp(&(&b.path, b.is_dir, b.size, b.mtime_ns, &b.etag))
        });
        entries.insert(0, here);
        Ok(Some(entries))
    }

    /// The bytes of a file, refused past limit.
    pub fn get(&self, path: &str, limit: u64) -> Result<Vec<u8>, Error> {
        let resp = self.request("GET", path)?.send().map_err(unreachable)?;
        let resp = self.answer(resp)?;
        match resp.status().as_u16() {
            404 => return Err(Error::NotFound),
            200 => {}
            code => return Err(answered(code)),
        }
        let mut data = Vec::new();
        resp.take(limit + 1)
            .read_to_end(&mut data)
            .map_err(unreachable)?;
        if data.len() as u64 > limit {
            return Err(Error::User(format!(
                "the file is larger than {} MB, too large to fetch from Nextcloud.",
                limit / 1_000_000
            )));
        }
        Ok(data)
    }

    /// The status of a PUT or MKCOL, one of expected; a refusal is a user error.
    pub fn send(
        &self,
        method: &str,
        path: &str,
        data: Option<&[u8]>,
        expected: &[u16],
    ) -> Result<u16, Error> {
        let mut req = self.request(method, path)?;
        if let Some(data) = data {
            req = req.body(data.to_vec());
        }
        let resp = self.answer(req.send().map_err(unreachable)?)?;
        let code = resp.status().as_u16();
        if code == 403 {
            return Err(Error::User(
                "Nextcloud does not let the app's account change it.".into(),
            ));
        }
        // A lock no client can write past: Text keeps one on every file opened
        // in it, sometimes long after it is closed (nextcloud/text#9042).
        if code == 423 {
            return Err(Error::User(
                "Nextcloud holds a lock on it: it is, or was, open in Nextcloud's Text editor. \
                 Close it there, or ask the admin to unlock it."
                    .into(),
            ));
        }
        if !expected.contains(&code) {
            return Err(answered(code));
        }
        Ok(code)
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The path of an href, which may be a full URL.
fn href_path(href: &str) -> &str {
    match href.find("://") {
        Some(i) => href[i + 3..]
            .find('/')
            .map(|j| &href[i + 3 + j..])
            .unwrap_or("/"),
        None => href,
    }
}

/// One <d:response> of a multistatus answer.
#[derive(Default)]
struct Response {
    href: String,
    collection: bool,
    size: i64,
    modified: Option<String>,
    etag: String,
}

/// The responses of a WebDAV multistatus body; None when it is not XML.
fn parse_multistatus(body: &str) -> Option<Vec<Response>> {
    use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
    let mut reader = NsReader::from_str(body);
    let mut out = Vec::new();
    let mut current: Option<Response> = None;
    let mut text = String::new();
    loop {
        let (ns, event) = reader.read_resolved_event().ok()?;
        let dav = matches!(ns, ResolveResult::Bound(n) if n.as_ref() == "DAV:");
        match event {
            Event::Start(e) | Event::Empty(e) if dav => {
                let local = e.local_name().as_ref().to_string();
                match local.as_str() {
                    "response" => current = Some(Response::default()),
                    "collection" => {
                        if let Some(r) = current.as_mut() {
                            r.collection = true;
                        }
                    }
                    _ => {}
                }
                text.clear();
            }
            Event::Text(t) => text.push_str(&t.html_content()),
            Event::GeneralRef(r) => {
                let name = r.html_content();
                text.push_str(&html_escape::decode_html_entities(&format!("&{name};")));
            }
            Event::CData(t) => text.push_str(&t.html_content()),
            Event::End(e) if dav => {
                let local = e.local_name().as_ref().to_string();
                if let Some(r) = current.as_mut() {
                    match local.as_str() {
                        "href" if r.href.is_empty() => r.href = text.clone(),
                        "getcontentlength" => r.size = text.trim().parse().unwrap_or(0),
                        "getlastmodified" if !text.is_empty() => r.modified = Some(text.clone()),
                        "getetag" => r.etag = text.clone(),
                        _ => {}
                    }
                }
                if local == "response"
                    && let Some(r) = current.take()
                {
                    out.push(r);
                }
                text.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_refusal_and_the_client_asks_no_more() {
        let (url, log) = crate::testing::serve(|_, _, _| (401, String::new()));
        let nc = Nextcloud::new(&format!("{url}/dav/files/alice"), "", "alice", "wrong").unwrap();
        for _ in 0..3 {
            let refused = nc.list("Partage", 0);
            assert!(matches!(
                refused,
                Err(Error::Nextcloud { refused: true, .. })
            ));
        }
        assert_eq!(log.lock().unwrap().len(), 1);
    }
}
