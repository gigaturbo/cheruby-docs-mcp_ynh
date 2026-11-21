//! cheruby-docs: MCP tools on one user's homelab folders, and their search index.
//!
//! `serve` is the tool server (cheruby-docs-mcp.service), `index` one index
//! run (cheruby-docs-mcp-index.service). The other commands are for the
//! shell and the package's scripts. Settings come from the DOCS_* variables,
//! which systemd reads from docs.env and index.env; a shell command reads
//! the file beside bin/ itself, so it needs root.

mod calendar;
mod embed;
mod folders;
mod index;
mod nextcloud;
mod passages;
mod serve;
mod status;
#[cfg(test)]
mod testing;
mod text;
mod tools;

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    version,
    about = "MCP tools on a user's homelab folders, and their search index"
)]
struct Cli {
    /// KEY=VALUE lines to read the DOCS_* settings from; by default the
    /// command's own beside bin/ (docs.env or index.env), when it exists.
    #[arg(long, global = true)]
    env_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
struct As {
    /// Act as the Whoopy token, on its folders only, not as the user's own.
    #[arg(long)]
    gateway: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Run the tool server.
    Serve,
    /// Run the index once.
    Index,
    /// Say how the index runs went, as the config panel does.
    Status {
        /// The user whose index it is; by default the folders file's first.
        #[arg(long)]
        user: Option<String>,
        /// Where the index is; by default DOCS_INDEX_DIR.
        #[arg(long)]
        index_dir: Option<PathBuf>,
        /// The index unit, without .service.
        #[arg(long, default_value = "cheruby-docs-mcp-index")]
        unit: String,
    },
    /// List a folder, as list_folder.
    Ls {
        #[arg(default_value = "")]
        path: String,
        #[command(flatten)]
        r#as: As,
    },
    /// Find files by name, as find_files.
    Find {
        query: String,
        #[arg(long, default_value = "")]
        path: String,
        #[command(flatten)]
        r#as: As,
    },
    /// Read a file, as read_file.
    Read {
        path: String,
        #[arg(long, default_value_t = 1)]
        page: i64,
        #[arg(long, default_value_t = 5)]
        pages: i64,
        #[arg(long, default_value_t = 0)]
        offset: i64,
        #[arg(long, default_value_t = 8000)]
        max_chars: i64,
        #[command(flatten)]
        r#as: As,
    },
    /// Search by meaning and keywords, as search_documents.
    Search {
        query: String,
        #[arg(long, default_value = "")]
        path: String,
        #[arg(long, default_value = "")]
        after: String,
        #[arg(long, default_value = "")]
        before: String,
        #[arg(long, default_value_t = 8)]
        limit: i64,
        #[command(flatten)]
        r#as: As,
    },
    /// Ask the index unit for a run of these files, or of everything, as run_index.
    RunIndex {
        paths: Vec<String>,
        #[command(flatten)]
        r#as: As,
    },
    /// Print Syncthing's folders from its config.xml, a tab-separated line each.
    SyncthingFolders { config: PathBuf },
    /// Print the OnCalendar of a cron schedule without the quiet hours, or nothing.
    Calendar { cron: String, quiet: String },
    /// Check that the desktop's vectors, through Whoopy, agree with this server's.
    CheckDesktop,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("cheruby-docs: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The file beside bin/, such as <install dir>/docs.env.
fn beside_bin(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.parent()?.join(name))
}

/// Sets the DOCS_* variables of an env file of KEY=VALUE lines. The given
/// file must exist; the default one, beside bin/, only when it does.
fn load_env(given: Option<&Path>, default: Option<&str>) -> Result<(), String> {
    let path = match (given, default.and_then(beside_bin)) {
        (Some(p), _) => p.to_path_buf(),
        (None, Some(p)) if p.is_file() => p,
        _ => return Ok(()),
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}: {e}; run as root.", path.display()))?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            // SAFETY: before any thread starts.
            unsafe { std::env::set_var(key.trim(), value.trim()) };
        }
    }
    Ok(())
}

/// As root, become the app's user, owner of the index folder, with the
/// groups of the user's folders on disk, as the unit runs: the shell tools
/// then read what the server could, and no more.
fn drop_root(docs: &tools::Docs, user: &str) -> Result<(), String> {
    use nix::unistd::{Gid, Uid, getuid, setgid, setgroups, setuid};
    use std::os::unix::fs::MetadataExt;
    if !getuid().is_root() {
        return Ok(());
    }
    let owner = std::fs::metadata(&docs.index_dir)
        .map_err(|e| format!("{}: {e}", docs.index_dir.display()))?;
    let mut groups: Vec<Gid> = Vec::new();
    for (_, root) in docs
        .users
        .get(user)
        .map(|u| u.roots.as_slice())
        .unwrap_or(&[])
    {
        if let Ok(m) = std::fs::metadata(root)
            && !groups.contains(&Gid::from_raw(m.gid()))
        {
            groups.push(Gid::from_raw(m.gid()));
        }
    }
    let fail = |e: nix::Error| format!("dropping root: {e}");
    setgroups(&groups).map_err(fail)?;
    setgid(Gid::from_raw(owner.gid())).map_err(fail)?;
    setuid(Uid::from_raw(owner.uid())).map_err(fail)?;
    Ok(())
}

/// The context of a shell tool: the user's own token's folders, or the gateway's.
fn shell_ctx(gateway: bool) -> Result<tools::Ctx, String> {
    let docs = serve::load_docs()?;
    let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let grants = serve::load_users(&env, &docs.users)?;
    let gateway_user = env.get("DOCS_GATEWAY_USER").cloned().unwrap_or_default();
    let grant = if gateway {
        grants
            .into_iter()
            .map(|(_, g)| g)
            .find(|g| g.user == gateway_user && env.contains_key("DOCS_GATEWAY_TOKEN"))
    } else {
        let own = env
            .keys()
            .find_map(|k| k.strip_prefix("DOCS_TOKEN_"))
            .map(String::from);
        grants
            .into_iter()
            .map(|(_, g)| g)
            .find(|g| Some(&g.user) == own.as_ref())
    };
    let grant = grant.ok_or("no such token is set.")?;
    drop_root(&docs, &grant.user)?;
    Ok(tools::Ctx {
        docs: Arc::new(docs),
        grant: Arc::new(grant),
    })
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

fn print(result: Result<String, String>) -> Result<(), String> {
    let text = result?;
    println!("{text}");
    Ok(())
}

fn run(cli: Cli) -> Result<(), String> {
    let given = cli.env_file.as_deref();
    match cli.command {
        Command::Serve => {
            load_env(given, None)?;
            let docs = Arc::new(serve::load_docs()?);
            runtime()?.block_on(serve::run(Arc::clone(&docs)))
        }
        Command::Index => {
            load_env(given, None)?;
            index::main(&index::Settings::from_env()?)
        }
        Command::Status {
            user,
            index_dir,
            unit,
        } => {
            load_env(given, Some("index.env"))?;
            let index_dir = index_dir
                .or_else(|| std::env::var_os("DOCS_INDEX_DIR").map(PathBuf::from))
                .ok_or("no index folder: give --index-dir.")?;
            let user = match user {
                Some(u) => u,
                None => {
                    let file =
                        std::env::var("DOCS_FOLDERS_FILE").map_err(|_| "no user: give --user.")?;
                    folders::load_folders(&file)?
                        .into_keys()
                        .next()
                        .ok_or("no user in the folders file.")?
                }
            };
            let state = status::systemd(&format!("{unit}.service"), "ActiveState");
            let next = status::systemd(&format!("{unit}.timer"), "NextElapseUSecRealtime");
            let next = if next.is_empty() || next == "n/a" {
                "none: the schedule is never.".to_string()
            } else {
                next
            };
            let watching = status::systemd(&format!("{unit}.path"), "ActiveState");
            println!(
                "{}",
                status::render(
                    &index::read_status(&index_dir.join(&user)),
                    &state,
                    Some(&next),
                    &unit
                )
            );
            // run_index's requests wait for the path unit.
            if !watching.is_empty() && watching != "active" {
                println!(
                    "- **Requests from the tools**: not taken, the path unit is {watching}: systemctl status {unit}.path"
                );
            }
            Ok(())
        }
        Command::Ls { path, r#as } => {
            load_env(given, Some("docs.env"))?;
            let ctx = shell_ctx(r#as.gateway)?;
            print(runtime()?.block_on(tools::list_folder(&ctx, path)))
        }
        Command::Find { query, path, r#as } => {
            load_env(given, Some("docs.env"))?;
            let ctx = shell_ctx(r#as.gateway)?;
            print(runtime()?.block_on(tools::find_files(&ctx, query, path)))
        }
        Command::Read {
            path,
            page,
            pages,
            offset,
            max_chars,
            r#as,
        } => {
            load_env(given, Some("docs.env"))?;
            let ctx = shell_ctx(r#as.gateway)?;
            match runtime()?
                .block_on(tools::read_file(&ctx, path, page, pages, offset, max_chars))?
            {
                tools::Reply::Text(text) => println!("{text}"),
                tools::Reply::Image { data, mime } => {
                    println!("[{mime}, {}]", tools::human(data.len() as f64))
                }
            }
            Ok(())
        }
        Command::Search {
            query,
            path,
            after,
            before,
            limit,
            r#as,
        } => {
            load_env(given, Some("docs.env"))?;
            let ctx = shell_ctx(r#as.gateway)?;
            print(runtime()?.block_on(tools::search_documents(
                &ctx, query, path, after, before, limit,
            )))
        }
        Command::RunIndex { paths, r#as } => {
            load_env(given, Some("docs.env"))?;
            let ctx = shell_ctx(r#as.gateway)?;
            print(runtime()?.block_on(tools::run_index(&ctx, paths)))
        }
        Command::SyncthingFolders { config } => {
            // Nothing when it cannot be read, as when Syncthing has no config yet.
            let lines = std::fs::read(&config)
                .ok()
                .and_then(|xml| folders::syncthing_folders(&xml))
                .unwrap_or_default();
            for line in lines {
                println!("{line}");
            }
            Ok(())
        }
        Command::Calendar { cron, quiet } => {
            if let Some(expr) = calendar::calendar(&cron, &quiet) {
                println!("{expr}");
            }
            Ok(())
        }
        Command::CheckDesktop => {
            load_env(given, Some("index.env"))?;
            let s = index::Settings::from_env()?;
            if s.remote_key.is_empty() || s.remote_model.is_empty() {
                return Err("no Whoopy key or model is saved in the panel.".into());
            }
            let local = embed::Embedder::local(&s.embed_url);
            match index::desktop_check(&local, &s.remote(), &s.family)
                .map_err(|e| format!("this server's embedding: {e}"))?
            {
                Ok(lowest) => {
                    let verdict = if lowest >= index::MIN_COSINE {
                        "a run on the desktop would use it"
                    } else {
                        "a run would stay on this server"
                    };
                    println!(
                        "Lowest cosine {lowest:.4} against {}: {verdict}.",
                        index::MIN_COSINE
                    );
                    Ok(())
                }
                Err(why) => Err(why),
            }
        }
    }
}
