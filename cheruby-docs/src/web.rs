//! The page: Status, Search and Config for the docs user, behind the YunoHost
//! portal. Config changes the settings that do not confine the app
//! (settings::Settings): the schedule, the quiet hours and the filters.
//! Served by the tool server's process on its own loopback port, apart from
//! /mcp. Settings, from docs.env:
//!
//!     DOCS_PAGE_PORT      where nginx sends the page; unset, no page
//!     DOCS_PANEL_TOKEN    the X-Panel-Token nginx adds to every request
//!     DOCS_PAGE_USER      whose folders and index the page shows
//!
//! The portal decides who reaches nginx (the docs user only); the token
//! proves a request came through nginx, so another local process cannot
//! use the page. The portal's Basic header must name the docs user, in case
//! the permission is widened in the webadmin. A POST must also come from
//! the page itself.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    folders::NEXTCLOUD,
    index,
    settings::{SOURCES, Settings},
    status,
    tools::{self, Ctx, Docs, Grant},
};

const PAGE: &str = include_str!("../assets/index.html");
const FONT: &[u8] = include_bytes!("../assets/fredoka.woff2");
const MASCOT: &[u8] = include_bytes!("../assets/mascot.svg");
const FAVICON: &[u8] = include_bytes!("../assets/favicon.svg");
const FAVICON_ICO: &[u8] = include_bytes!("../assets/favicon.ico");
const TOUCH_ICON: &[u8] = include_bytes!("../assets/apple-touch-icon.png");
/// Refused below this, as the MCP tokens are.
const MIN_TOKEN_LEN: usize = 32;
const MAX_QUERY_CHARS: usize = 500;

#[derive(Clone)]
struct Page {
    docs: Arc<Docs>,
    grant: Arc<Grant>,
    token: Arc<String>,
}

impl Page {
    /// A request's context, with the settings as they are now.
    fn ctx(&self) -> Ctx {
        Ctx {
            live: self.docs.live(),
            docs: Arc::clone(&self.docs),
            grant: Arc::clone(&self.grant),
        }
    }
}

/// The page served until the process stops.
pub type Serving = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Binds the page's port, when DOCS_PAGE_PORT is set: a page that cannot
/// start fails here, before the server says it is ready.
pub async fn bind(docs: Arc<Docs>) -> Result<Option<Serving>, String> {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let port = var("DOCS_PAGE_PORT");
    if port.is_empty() {
        return Ok(None);
    }
    let token = var("DOCS_PANEL_TOKEN");
    if token.chars().count() < MIN_TOKEN_LEN {
        return Err(format!(
            "DOCS_PANEL_TOKEN is shorter than {MIN_TOKEN_LEN} characters."
        ));
    }
    let user = var("DOCS_PAGE_USER");
    let folders = docs
        .live()
        .users
        .get(&user)
        .map(|u| u.roots.clone())
        .ok_or_else(|| format!("DOCS_PAGE_USER: no folders for {user:?}."))?;
    let page = Page {
        docs,
        grant: Arc::new(Grant { user, folders }),
        token: Arc::new(token),
    };
    let app = Router::new()
        .route(
            "/",
            get(|| async { asset("text/html; charset=utf-8", PAGE.as_bytes()) }),
        )
        .route(
            "/fredoka.woff2",
            get(|| async { asset("font/woff2", FONT) }),
        )
        .route(
            "/favicon.svg",
            get(|| async { asset("image/svg+xml", FAVICON) }),
        )
        .route(
            "/favicon.ico",
            get(|| async { asset("image/x-icon", FAVICON_ICO) }),
        )
        .route(
            "/apple-touch-icon.png",
            get(|| async { asset("image/png", TOUCH_ICON) }),
        )
        .route(
            "/mascot.svg",
            get(|| async { asset("image/svg+xml", MASCOT) }),
        )
        .route("/api/status", get(status_json))
        .route("/api/search", post(search_json))
        .route("/api/settings", get(settings_json).post(save_settings))
        .layer(middleware::from_fn_with_state(page.clone(), guard))
        .with_state(page);
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .map_err(|e| format!("127.0.0.1:{port}: {e}"))?;
    eprintln!("cheruby-docs: page at http://127.0.0.1:{port}/");
    Ok(Some(Box::pin(async move {
        axum::serve(listener, app).await.map_err(|e| e.to_string())
    })))
}

fn asset(kind: &'static str, body: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, kind),
            (header::CACHE_CONTROL, "max-age=300"),
        ],
        body,
    )
        .into_response()
}

/// Constant-time comparison, so a wrong token's timing says nothing of the right one.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// nginx's token on every request; a POST from the page only, which a
/// browser marks same-origin and another site cannot fake.
async fn guard(State(page): State<Page>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let given = headers
        .get("x-panel-token")
        .map(|v| v.as_bytes())
        .unwrap_or_default();
    if !same(given, page.token.as_bytes()) {
        return (StatusCode::UNAUTHORIZED, "not through the portal\n").into_response();
    }
    if portal_user(headers).as_deref() != Some(page.grant.user.as_str()) {
        return (StatusCode::FORBIDDEN, "this page is another user's\n").into_response();
    }
    if req.method() != Method::GET && !same_origin(headers) {
        return (StatusCode::FORBIDDEN, "not from this page\n").into_response();
    }
    next.run(req).await
}

/// The user the portal logged in, from the "Basic base64(<user>:-)" header
/// SSOwat sets in place of any the browser sent.
fn portal_user(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    Some(text.split_once(':')?.0.to_string())
}

fn same_origin(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-site")
        .is_some_and(|v| v.as_bytes() == b"same-origin")
}

/// Whether a local server answers its /health, within two seconds.
async fn up(http: &reqwest::Client, url: &str) -> Option<bool> {
    if url.is_empty() {
        return None;
    }
    let answer = http
        .get(format!("{}/health", url.trim_end_matches('/')))
        .timeout(Duration::from_secs(2))
        .send()
        .await;
    Some(answer.is_ok_and(|r| r.status().is_success()))
}

/// The index runs, the servers and the folders, for the Status tab.
async fn status_json(State(page): State<Page>) -> Json<Value> {
    let ctx = page.ctx();
    let docs = &ctx.docs;
    let grant = &ctx.grant;
    let base = docs.index_dir.join(&grant.user);
    let running = index::running(&docs.index_dir);
    let run = index::read_status(&base);
    let lines = status::render(
        &run,
        if running { "activating" } else { "inactive" },
        Some(&status::next_run(&ctx.live.settings, running)),
        "cheruby-docs-mcp-index",
    );
    let own = &ctx.live.users[&grant.user];
    let folders: Vec<Value> = grant
        .folders
        .iter()
        .map(|(label, root)| {
            let state = if root.starts_with(NEXTCLOUD) {
                "in Nextcloud"
            } else if std::fs::read_dir(root).is_ok() {
                "readable"
            } else {
                "unreadable"
            };
            json!({"label": label, "state": state, "writable": own.writable.contains(label)})
        })
        .collect();
    Json(json!({
        "user": grant.user,
        "running": running,
        "index": lines.lines().collect::<Vec<_>>(),
        "embed": up(&docs.http, &docs.embed_url).await,
        "rerank": up(&docs.http, &docs.rerank_url).await,
        "folders": folders,
    }))
}

/// The Config tab: the settings, the sources the user has folders in, and
/// when the next run comes.
async fn settings_json(State(page): State<Page>) -> Json<Value> {
    let ctx = page.ctx();
    let own = &ctx.live.users[&ctx.grant.user];
    let sources: Vec<&str> = SOURCES
        .into_iter()
        .filter(|s| own.sources.values().any(|v| v == s))
        .collect();
    let running = index::running(&ctx.docs.index_dir);
    Json(json!({
        "settings": ctx.live.settings,
        "sources": sources,
        "next": status::next_run(&ctx.live.settings, running),
    }))
}

/// Saved and in use at once, for the tools as for the schedule.
async fn save_settings(State(page): State<Page>, Json(settings): Json<Settings>) -> Response {
    let docs = Arc::clone(&page.docs);
    let saved = tokio::task::spawn_blocking(move || docs.set(settings))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    if let Err(e) = saved {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response();
    }
    settings_json(State(page)).await.into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    path: String,
}

/// The Search tab: the best passage of each file, as search_documents finds them.
async fn search_json(State(page): State<Page>, Json(a): Json<SearchArgs>) -> Response {
    if a.query.chars().count() > MAX_QUERY_CHARS {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("a query of {MAX_QUERY_CHARS} characters at most.")})),
        )
            .into_response();
    }
    match tools::search(
        &page.ctx(),
        a.query,
        a.path,
        String::new(),
        String::new(),
        10,
    )
    .await
    {
        Ok(found) => Json(found).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compare_whole() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"abcd"));
        assert!(!same(b"", b"a"));
    }

    #[test]
    fn the_portal_names_the_user() {
        let mut h = HeaderMap::new();
        assert_eq!(portal_user(&h), None);
        h.insert(header::AUTHORIZATION, "Basic YWxpY2U6LQ==".parse().unwrap());
        assert_eq!(portal_user(&h).as_deref(), Some("alice"));
        h.insert(
            header::AUTHORIZATION,
            "Bearer YWxpY2U6LQ==".parse().unwrap(),
        );
        assert_eq!(portal_user(&h), None);
    }

    #[test]
    fn a_post_needs_the_page() {
        let mut h = HeaderMap::new();
        assert!(!same_origin(&h));
        h.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(!same_origin(&h));
        h.insert("sec-fetch-site", "same-origin".parse().unwrap());
        assert!(same_origin(&h));
    }
}
