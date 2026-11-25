//! The MCP server: the tools at /mcp over Streamable HTTP, stateless, with
//! JSON answers. Run by cheruby-docs-mcp.service. Settings come from its
//! root-600 env file:
//!
//!     DOCS_BIND               address to listen on, default 127.0.0.1
//!     DOCS_PORT               default 8770
//!     DOCS_TOKEN_<user>       a request carrying "Authorization: Bearer <it>" acts as <user>
//!     DOCS_FOLDERS_FILE       each user's folders and their filters (folders::load_folders)
//!     DOCS_GATEWAY_TOKEN      optional: a token for the AI gateway, acting as
//!     DOCS_GATEWAY_USER       this user, on
//!     DOCS_GATEWAY_FOLDERS    these of the user's folders, labels joined by ":"
//!     DOCS_INDEX_DIR          where the indexer keeps <user>.sqlite, read here only
//!     DOCS_EMBED_URL          the embedding server, e.g. http://127.0.0.1:8771
//!     DOCS_EMBED_MODEL        the model's identity, as the indexer stores it
//!     DOCS_EMBED_NAME         the model's family (embed::query_prefix)
//!     DOCS_RERANK_URL         optional: the reranking server, e.g. http://127.0.0.1:8774
//!     DOCS_NEXTCLOUD_*        when a folder is in Nextcloud (nextcloud::Nextcloud)
//!
//! A token is one user's, with all or some of that user's folders; the
//! gateway token searches that user's index. The unit adds the outer wall:
//! the process sees only these folders on disk and the index, read-only but
//! for the writable folders.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use base64::Engine;
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::Extension, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::Deserialize;

use crate::{
    embed,
    folders::{self, UserFolders, py_repr},
    index,
    nextcloud::Nextcloud,
    tools::{self, Ctx, Docs, Grant, Reply},
};

/// Each token, and what it opens.
type Tokens = Arc<Vec<(String, Arc<Grant>)>>;

/// Refused below this: a token is the only thing guarding the server.
const MIN_TOKEN_LEN: usize = 32;

/// {token: what it opens} from the DOCS_TOKEN_* and DOCS_GATEWAY_* settings.
pub fn load_users(
    env: &BTreeMap<String, String>,
    users: &BTreeMap<String, UserFolders>,
) -> Result<Vec<(String, Grant)>, String> {
    let mut tokens: Vec<(String, String, Grant)> = Vec::new();
    for (key, token) in env {
        if let Some(name) = key.strip_prefix("DOCS_TOKEN_") {
            let folders = users.get(name).map(|u| u.roots.clone()).unwrap_or_default();
            tokens.push((
                key.clone(),
                token.clone(),
                Grant {
                    user: name.to_string(),
                    folders,
                },
            ));
        }
    }
    if let Some(token) = env.get("DOCS_GATEWAY_TOKEN").filter(|t| !t.is_empty()) {
        let name = env.get("DOCS_GATEWAY_USER").cloned().unwrap_or_default();
        let own = users
            .get(&name)
            .map(|u| u.roots.clone())
            .unwrap_or_default();
        let labels: Vec<&str> = env
            .get("DOCS_GATEWAY_FOLDERS")
            .map(String::as_str)
            .unwrap_or("")
            .split(':')
            .filter(|l| !l.is_empty())
            .collect();
        if !labels.iter().all(|l| own.iter().any(|(o, _)| o == l)) {
            return Err(format!(
                "DOCS_GATEWAY_FOLDERS names folders {} does not have.",
                py_repr(&name)
            ));
        }
        let folders = own
            .into_iter()
            .filter(|(l, _)| labels.contains(&l.as_str()))
            .collect();
        tokens.push((
            "DOCS_GATEWAY_TOKEN".into(),
            token.clone(),
            Grant {
                user: name,
                folders,
            },
        ));
    }
    let mut out: Vec<(String, Grant)> = Vec::new();
    for (key, token, grant) in tokens {
        if token.chars().count() < MIN_TOKEN_LEN {
            return Err(format!("{key} is shorter than {MIN_TOKEN_LEN} characters."));
        }
        if out.iter().any(|(t, _)| *t == token) {
            return Err(format!("{key} repeats another token."));
        }
        out.push((token, grant));
    }
    if out.is_empty() {
        return Err("No DOCS_TOKEN_<user> is set.".into());
    }
    Ok(out)
}

/// What every command that reads the folders needs, from the environment.
pub fn load_docs() -> Result<Docs, String> {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let users = folders::load_folders(
        &std::env::var("DOCS_FOLDERS_FILE").map_err(|_| "DOCS_FOLDERS_FILE is not set.")?,
    )?;
    let nextcloud = Nextcloud::from_env()?;
    index::needs_nextcloud(&users, nextcloud.as_ref())?;
    let index_dir = std::path::PathBuf::from(var("DOCS_INDEX_DIR"));
    // The indexer notes a refused password; it is not tried again until a new one restarts the server.
    if let Some(nc) = &nextcloud
        && users.keys().any(|u| {
            index::read_status(&index_dir.join(u)).get("nextcloud_refused")
                == Some(&serde_json::json!(nc.mark))
        })
    {
        nc.refused.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let family = var("DOCS_EMBED_NAME");
    let family = if family.is_empty() {
        "embeddinggemma".to_string()
    } else {
        family
    };
    let query_prefix = embed::query_prefix(&family)
        .ok_or_else(|| format!("DOCS_EMBED_NAME: no model family {}.", py_repr(&family)))?;
    let http = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?;
    Ok(Docs {
        users,
        nextcloud,
        index_dir,
        embed_url: var("DOCS_EMBED_URL"),
        embed_model: var("DOCS_EMBED_MODEL"),
        query_prefix,
        rerank_url: var("DOCS_RERANK_URL"),
        quiet: var("DOCS_QUIET_HOURS"),
        http,
    })
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ListArgs {
    /// A folder such as "Documents/Factures"; empty lists the top folders.
    #[serde(default)]
    path: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct FindArgs {
    query: String,
    /// Limit the search to one folder, such as "Documents/Santé".
    #[serde(default)]
    path: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ReadArgs {
    path: String,
    #[serde(default = "one")]
    page: i64,
    #[serde(default = "five")]
    pages: i64,
    #[serde(default)]
    offset: i64,
    #[serde(default = "eight_thousand")]
    max_chars: i64,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    after: String,
    #[serde(default)]
    before: String,
    #[serde(default = "eight")]
    limit: i64,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct WriteArgs {
    path: String,
    content: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct RunArgs {
    #[serde(default)]
    files: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct FolderArgs {
    path: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct MoveArgs {
    source: String,
    destination: String,
}

fn one() -> i64 {
    1
}
fn five() -> i64 {
    5
}
fn eight() -> i64 {
    8
}
fn eight_thousand() -> i64 {
    8000
}

#[derive(Clone)]
pub struct Tools {
    docs: Arc<Docs>,
    instructions: String,
    tool_router: ToolRouter<Tools>,
}

fn answer(result: Result<String, String>) -> Result<CallToolResult, ErrorData> {
    Ok(match result {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(text) => CallToolResult::error(vec![ContentBlock::text(text)]),
    })
}

impl Tools {
    /// The call's context: the grant the token middleware put on the request.
    fn ctx(&self, parts: &http::request::Parts) -> Result<Ctx, ErrorData> {
        let grant = parts
            .extensions
            .get::<Arc<Grant>>()
            .cloned()
            .ok_or_else(|| ErrorData::internal_error("no grant", None))?;
        Ok(Ctx {
            docs: Arc::clone(&self.docs),
            grant,
        })
    }
}

#[tool_router]
impl Tools {
    #[tool(
        description = "List one folder: its subfolders, then its files with size and date.\n\n\
        path: a folder such as \"Documents/Factures\"; empty lists the top folders."
    )]
    async fn list_folder(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(a): Parameters<ListArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        answer(tools::list_folder(&self.ctx(&parts)?, a.path).await)
    }

    #[tool(
        description = "Find files and folders whose path contains every word of the query.\n\n\
        Matching ignores case and accents. Searches names only, not contents.\n\
        path: limit the search to one folder, such as \"Documents/Santé\"."
    )]
    async fn find_files(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(a): Parameters<FindArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        answer(tools::find_files(&self.ctx(&parts)?, a.query, a.path).await)
    }

    #[tool(
        description = "Read one file: text, Markdown, PDF, Word, Excel, PowerPoint, OpenDocument,\n\
        EPUB, email (.eml) or an image.\n\n\
        PDFs are read by page: page is the first one, pages how many (1 to 20).\n\
        Other text is read by character: offset is where to start, max_chars how much\n\
        (up to 20000). An image comes back as the picture itself. A scanned PDF has\n\
        no text yet."
    )]
    async fn read_file(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(a): Parameters<ReadArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match tools::read_file(
            &self.ctx(&parts)?,
            a.path,
            a.page,
            a.pages,
            a.offset,
            a.max_chars,
        )
        .await
        {
            Ok(Reply::Image { data, mime }) => {
                Ok(CallToolResult::success(vec![ContentBlock::image(
                    base64::engine::general_purpose::STANDARD.encode(data),
                    mime,
                )]))
            }
            Ok(Reply::Text(text)) => answer(Ok(text)),
            Err(e) => answer(Err(e)),
        }
    }

    #[tool(
        description = "Find passages of documents by what they say, in the user's words.\n\n\
        Searches by meaning and by keywords, so \"the letter about the rent increase\"\n\
        finds a letter that never uses those words. Returns the best passage of\n\
        each of the best files, with its page and date; read more with read_file.\n\
        path: limit the search to one folder, such as \"Documents/Logement\".\n\
        after, before: the file's date, as 2025-03-31.\n\
        limit: how many files, 1 to 10.\n\
        Scans and images are not searchable yet; find them with find_files."
    )]
    async fn search_documents(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(a): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        answer(
            tools::search_documents(
                &self.ctx(&parts)?,
                a.query,
                a.path,
                a.after,
                a.before,
                a.limit,
            )
            .await,
        )
    }

    #[tool(
        description = "Write a text file in a folder listed as writable: create it, with any\n\
        missing folders, or replace it whole. Nothing is ever deleted.\n\n\
        path: such as \"Notes/Recettes/pain.md\". Text files only (.md, .txt, .csv...).\n\
        content: the whole new text of the file, UTF-8."
    )]
    async fn write_file(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(a): Parameters<WriteArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        answer(tools::write(&self.ctx(&parts)?, a.path, Some(a.content)).await)
    }

    #[tool(
        description = "Create a folder, with any missing folders above it, in a folder listed\n\
        as writable. A folder that exists already is left as it is.\n\n\
        path: such as \"Notes/Recettes/Pains\"."
    )]
    async fn create_folder(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(a): Parameters<FolderArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        answer(tools::write(&self.ctx(&parts)?, a.path, None).await)
    }

    #[tool(
        description = "Move or rename a file or a folder, of any kind, within one folder listed\n\
        as writable. Never replaces anything at the new place, never deletes.\n\n\
        source: such as \"Notes/brouillon.md\".\n\
        destination: its new path, such as \"Notes/Recettes/pain.md\"; the folder it goes\n\
        in must exist (create_folder)."
    )]
    async fn move_file(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(a): Parameters<MoveArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        answer(tools::move_file(&self.ctx(&parts)?, a.source, a.destination).await)
    }

    #[tool(
        description = "Ask for an index run, so search_documents finds what changed: of some\n\
        files, such as one just written, or of all the folders.\n\n\
        files: up to 50 paths such as \"Notes/Recettes/pain.md\"; empty asks for all the\n\
        folders, which redoes only what changed. With files, waits up to a minute and\n\
        says how each went. Refused in the index's quiet hours. Changes no file."
    )]
    async fn run_index(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(a): Parameters<RunArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        answer(tools::run_index(&self.ctx(&parts)?, a.files).await)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Tools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("docs", env!("CARGO_PKG_VERSION")))
            .with_instructions(self.instructions.clone())
    }
}

/// Constant-time comparison, so a wrong token's timing says nothing of the right one.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The bearer token picks the user, or the request gets a 401. The grant
/// goes on the request, where the tools find it.
async fn require_token(State(tokens): State<Tokens>, mut req: Request, next: Next) -> Response {
    let given = req
        .headers()
        .get("authorization")
        .map(|v| v.as_bytes().to_vec())
        .unwrap_or_default();
    let mut grant = None;
    for (token, candidate) in tokens.iter() {
        if same(&given, format!("Bearer {token}").as_bytes()) {
            grant = Some(Arc::clone(candidate));
        }
    }
    let Some(grant) = grant else {
        return (
            StatusCode::UNAUTHORIZED,
            [("content-type", "text/plain")],
            "missing or wrong token\n",
        )
            .into_response();
    };
    req.extensions_mut().insert(grant);
    next.run(req).await
}

/// Serves until stopped. docs holds blocking HTTP clients, which may be
/// neither made nor dropped inside the runtime: the caller keeps one handle.
pub async fn run(docs: Arc<Docs>) -> Result<(), String> {
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let tokens: Vec<(String, Arc<Grant>)> = load_users(&env, &docs.users)?
        .into_iter()
        .map(|(t, g)| (t, Arc::new(g)))
        .collect();
    let writable = docs.users.values().any(|u| !u.writable.is_empty());
    let mut instructions = "The user's own files. To find a document by what it says, \
        use search_documents; by its name, find_files; to browse, list_folder. Then read_file. \
        Paths start with a folder name such as Documents/. Cite the paths you used."
        .to_string();
    let mut tool_router = Tools::tool_router();
    if writable {
        instructions += " Folders listed as writable also take write_file, create_folder and move_file; the others are read-only.";
    } else {
        // Offered only where a folder takes them.
        tool_router.remove_route("write_file");
        tool_router.remove_route("create_folder");
        tool_router.remove_route("move_file");
    }
    let tools = Tools {
        docs,
        instructions,
        tool_router,
    };
    // Stateless and JSON: a restart leaves no session for a client to trip on.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None);
    let service = StreamableHttpService::new(
        move || Ok(tools.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let app = Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(
            Arc::new(tokens),
            require_token,
        ));
    let bind = std::env::var("DOCS_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let port = std::env::var("DOCS_PORT").unwrap_or_else(|_| "8770".into());
    let listener = tokio::net::TcpListener::bind(format!("{bind}:{port}"))
        .await
        .map_err(|e| format!("{bind}:{port}: {e}"))?;
    // "cheruby-docs: MCP at" is what docs_start waits for in the journal.
    eprintln!("cheruby-docs: MCP at http://{bind}:{port}/mcp");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("a signal handler");
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        })
        .await
        .map_err(|e| e.to_string())
}
