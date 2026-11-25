//! What the tool server and the indexer both know about the users' folders.
//!
//! The folders file and its filters, which names stay hidden, how a folder on
//! disk or in Nextcloud is listed and read; `text` reads what is inside a
//! file. Only `Tree::write` writes, for the tool server, in the folders marked
//! writable. Errors a user should see are `Error::User`.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsFd, OwnedFd},
        unix::fs::MetadataExt,
    },
    path::{Path, PathBuf},
};

use nix::{
    errno::Errno,
    fcntl::{AtFlags, OFlag, RenameFlags, openat, renameat2},
    sys::stat::{Mode, fstat, fstatat, mkdirat},
    unistd::{Gid, UnlinkatFlags, fchown, unlinkat},
};
use serde_json::Value;

use crate::nextcloud::{Entry, Nextcloud};

/// A write never leaves the disk, shared with everything else, fuller than this.
pub const MIN_FREE_BYTES: u64 = 2_000_000_000;
pub const TEXT_SUFFIXES: &[&str] = &[
    ".md", ".txt", ".csv", ".json", ".vcf", ".ics", ".xml", ".html", ".htm", ".yml", ".yaml",
    ".log",
];
/// A folder in the user's Nextcloud is "nextcloud:<path in the account>"
/// where one on disk is its real path.
pub const NEXTCLOUD: &str = "nextcloud:";

/// The image format a suffix names, as MCP and the indexer know it.
pub fn image_format(suffix: &str) -> Option<&'static str> {
    match suffix {
        ".jpg" | ".jpeg" => Some("jpeg"),
        ".png" => Some("png"),
        ".webp" => Some("webp"),
        ".gif" => Some("gif"),
        _ => None,
    }
}

/// What went wrong reading or writing a folder. `User` is what a user should
/// see as it is; `Nextcloud` says Nextcloud did not answer or refused the app
/// password (`refused`), and nothing else will answer either then.
#[derive(Debug)]
pub enum Error {
    User(String),
    NotFound,
    Io(io::Error),
    Nextcloud { message: String, refused: bool },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::User(m) | Error::Nextcloud { message: m, .. } => f.write_str(m),
            Error::NotFound => f.write_str("not found."),
            Error::Io(e) => f.write_str(&io_message(e)),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::NotFound {
            return Error::NotFound;
        }
        Error::Io(e)
    }
}

impl From<Errno> for Error {
    fn from(e: Errno) -> Self {
        Error::from(io::Error::from(e))
    }
}

/// An OS error's text without its number and with a full stop, never a path.
pub fn io_message(e: &io::Error) -> String {
    let text = e.to_string();
    let text = text.split(" (os error").next().unwrap_or(&text);
    format!("{text}.")
}

/// The suffix of a path's last name, lower case, with its dot: ".pdf", or ""
/// for none. A leading dot does not start one.
pub fn suffix(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let bare = name.trim_start_matches('.');
    match bare.rfind('.') {
        Some(i) => bare[i..].to_lowercase(),
        None => String::new(),
    }
}

/// Syncthing's .stfolder and .stversions, and every other dot name.
pub fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// A folder's filters, lower case, without the slashes around them.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    pub only: Vec<String>,
    pub skip: Vec<String>,
}

/// One user's folders as the folders file gives them, in its order.
#[derive(Clone, Debug, Default)]
pub struct UserFolders {
    /// (label, root): a root is a real path, or NEXTCLOUD and the account path.
    pub roots: Vec<(String, String)>,
    pub rules: HashMap<String, Rules>,
    pub writable: HashSet<String>,
}

/// Each user's folders, from the folders file.
///
/// The file is {user: {label: {"path": ..., "only": [...], "skip": [...]}}},
/// written by the package from the config panel's sources; a Nextcloud folder
/// has "nextcloud": <path in the account> in place of "path", and a writable
/// one "writable": true.
pub fn load_folders(path: &str) -> Result<BTreeMap<String, UserFolders>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let data: Value = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    let mut users = BTreeMap::new();
    for (user, entries) in data.as_object().ok_or(format!("{path}: not an object."))? {
        let mut own = UserFolders::default();
        for (label, entry) in entries.as_object().into_iter().flatten() {
            if label.is_empty() || label.contains('/') || hidden(label) {
                return Err(format!("{path}: bad folder label {}.", py_repr(label)));
            }
            let root = if let Some(cloud) = entry["nextcloud"].as_str() {
                let parts: Vec<&str> = cloud.split('/').filter(|p| !p.is_empty()).collect();
                format!("{NEXTCLOUD}{}", parts.join("/"))
            } else {
                let real = entry["path"]
                    .as_str()
                    .ok_or(format!("{path}: {label} has no path."))?;
                realpath(Path::new(real)).to_string_lossy().into_owned()
            };
            let patterns = |key: &str| -> Vec<String> {
                entry[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|p| !p.trim_matches('/').is_empty())
                    .map(|p| p.to_lowercase().trim_matches('/').to_string())
                    .collect()
            };
            own.rules.insert(
                label.clone(),
                Rules {
                    only: patterns("only"),
                    skip: patterns("skip"),
                },
            );
            if entry["writable"].as_bool() == Some(true) {
                own.writable.insert(label.clone());
            }
            own.roots.push((label.clone(), root));
        }
        users.insert(user.clone(), own);
    }
    Ok(users)
}

/// A string as Python's repr() shows it, for messages that name a value.
pub fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// A path with every link resolved, as far as it exists; the rest kept as named.
pub fn realpath(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if parent != path => realpath(parent).join(name),
        _ => path.to_path_buf(),
    }
}

/// Whether a glob matches a whole name or path, as Python's fnmatchcase:
/// `*` crosses `/`, `?` is one character, `[...]` a set, `[!...]` its negation.
fn fnmatch(name: &[char], pattern: &[char]) -> bool {
    let Some((&first, rest)) = pattern.split_first() else {
        return name.is_empty();
    };
    match first {
        '*' => (0..=name.len()).any(|n| fnmatch(&name[n..], rest)),
        '?' => !name.is_empty() && fnmatch(&name[1..], rest),
        '[' => match char_set(rest) {
            Some((set_len, negate, ranges)) => {
                let Some((&c, tail)) = name.split_first() else {
                    return false;
                };
                let inside = ranges.iter().any(|&(a, b)| a <= c && c <= b);
                inside != negate && fnmatch(tail, &rest[set_len..])
            }
            None => name.first() == Some(&'[') && fnmatch(&name[1..], rest),
        },
        c => name.first() == Some(&c) && fnmatch(&name[1..], rest),
    }
}

/// A glob's `[...]` set: its length up to and with its `]`, whether it is
/// negated, and its ranges.
type CharSet = (usize, bool, Vec<(char, char)>);

/// The set after a `[`; None when it is not closed, and `[` is a letter.
fn char_set(pattern: &[char]) -> Option<CharSet> {
    let mut i = 0;
    let negate = pattern.first() == Some(&'!');
    if negate {
        i += 1;
    }
    let start = i;
    let mut ranges = Vec::new();
    while i < pattern.len() {
        let c = pattern[i];
        if c == ']' && i > start {
            return Some((i + 1, negate, ranges));
        }
        if i + 2 < pattern.len() && pattern[i + 1] == '-' && pattern[i + 2] != ']' {
            ranges.push((c, pattern[i + 2]));
            i += 3;
        } else {
            ranges.push((c, c));
            i += 1;
        }
    }
    None
}

/// Whether a pattern matches a tool path, given split at /, or a folder above it.
///
/// Without a /, it matches any one name (*.pdf, Archives); with one, the path
/// from its label on (Documents/Old, Documents/*/2019). * crosses /.
fn matches(pattern: &str, parts: &[&str]) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    if pattern.contains(&'/') {
        return (1..=parts.len())
            .any(|n| fnmatch(&parts[..n].join("/").chars().collect::<Vec<_>>(), &pattern));
    }
    parts
        .iter()
        .any(|p| fnmatch(&p.chars().collect::<Vec<_>>(), &pattern))
}

/// Whether its folder's filters let a tool path through, ignoring case.
///
/// Skip takes out what it matches and everything below. Only, when set, keeps
/// just the files it matches or that lie in a folder it matches; folders pass
/// it, since they may hold such files.
pub fn kept(rules: &HashMap<String, Rules>, shown: &str, is_dir: bool) -> bool {
    let label = shown.split('/').next().unwrap_or("");
    let Some(rule) = rules.get(label) else {
        return true;
    };
    let lower = shown.to_lowercase();
    let parts: Vec<&str> = lower.split('/').collect();
    if rule.skip.iter().any(|p| matches(p, &parts)) {
        return false;
    }
    is_dir || rule.only.is_empty() || rule.only.iter().any(|p| matches(p, &parts))
}

/// Whether a resolved path lies in root without passing a hidden name, so a
/// link to .stversions/ inside the folder is refused like the name itself.
fn inside(real: &str, root: &str) -> bool {
    if real == root {
        return true;
    }
    match real.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
        Some(rel) => !rel.split('/').any(hidden),
        None => false,
    }
}

/// Whether path, once links are followed, stays in root out of hidden names
/// and is kept by the filters: a link cannot show a filtered file under its
/// own name.
fn kept_target(
    rules: &HashMap<String, Rules>,
    label: &str,
    root: &str,
    path: &Path,
    is_dir: bool,
) -> bool {
    let real = realpath(path).to_string_lossy().into_owned();
    if !inside(&real, root) {
        return false;
    }
    match real.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
        Some(rel) => kept(rules, &format!("{label}/{rel}"), is_dir),
        None => kept(rules, label, is_dir),
    }
}

/// Where an Item is: a real path, or a path in the Nextcloud account.
#[derive(Clone, Debug)]
pub enum Location {
    Disk(PathBuf),
    Cloud(String),
}

/// A file or folder as a token sees it.
#[derive(Clone, Debug)]
pub struct Item {
    /// Its tool path, Documents/Factures/x.pdf.
    pub shown: String,
    pub is_dir: bool,
    pub size: i64,
    pub mtime_ns: i64,
    /// Nextcloud has none: its mtime.
    pub ctime_ns: i64,
    pub location: Location,
    pub link: bool,
    pub etag: String,
}

/// A file to read an Item from: on disk, or fetched whole from Nextcloud.
pub enum Source {
    Disk(File),
    Fetched(Cursor<Vec<u8>>),
}

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Source::Disk(f) => f.read(buf),
            Source::Fetched(c) => c.read(buf),
        }
    }
}

impl Seek for Source {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            Source::Disk(f) => f.seek(pos),
            Source::Fetched(c) => c.seek(pos),
        }
    }
}

/// A Nextcloud folder's ETag and what it held, as the index keeps them.
pub type Listing = (String, Vec<Entry>);

/// One token's folders, (label, root), with their filters: what it can see in
/// them, and how to read it. Every listing goes through here, so hidden
/// names, links out and what the filters leave out are dropped one way only.
///
/// `cache`, for the indexer: {account path: listing} of Nextcloud folders
/// already listed. A folder whose ETag has not changed holds the same things,
/// so it is not asked again. Every folder listed goes to `listed`, to be
/// stored for the next run.
pub struct Tree<'a> {
    pub roots: &'a [(String, String)],
    pub rules: &'a HashMap<String, Rules>,
    pub nextcloud: Option<&'a Nextcloud>,
    pub cache: HashMap<String, Listing>,
    pub listed: HashMap<String, Listing>,
    /// Tool paths of the Nextcloud folders a walk could not list.
    pub unlisted: Vec<String>,
}

impl<'a> Tree<'a> {
    pub fn new(
        roots: &'a [(String, String)],
        rules: &'a HashMap<String, Rules>,
        nextcloud: Option<&'a Nextcloud>,
    ) -> Self {
        Tree {
            roots,
            rules,
            nextcloud,
            cache: HashMap::new(),
            listed: HashMap::new(),
            unlisted: Vec::new(),
        }
    }

    fn root(&self, label: &str) -> &str {
        self.roots
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, r)| r.as_str())
            .unwrap_or("")
    }

    fn cloud(&self) -> Result<&'a Nextcloud, Error> {
        self.nextcloud.ok_or(Error::Nextcloud {
            message: "Nextcloud is not set up.".into(),
            refused: false,
        })
    }

    /// The Item at label/parts, None when it is missing or filtered out.
    ///
    /// Refuses a path that leads out of its folder or into a hidden one.
    /// parts holds no "..", no hidden name: the caller checks.
    pub fn item(&self, label: &str, parts: &[String]) -> Result<Option<Item>, Error> {
        let root = self.root(label);
        let shown = std::iter::once(label)
            .chain(parts.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("/");
        if let Some(account) = root.strip_prefix(NEXTCLOUD) {
            let place = std::iter::once(account)
                .chain(parts.iter().map(String::as_str))
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("/");
            let Some(entries) = self.cloud()?.list(&place, 0)? else {
                return Ok(None);
            };
            let e = &entries[0];
            if !kept(self.rules, &shown, e.is_dir) {
                return Ok(None);
            }
            return Ok(Some(Item {
                shown,
                is_dir: e.is_dir,
                size: e.size,
                mtime_ns: e.mtime_ns,
                ctime_ns: e.mtime_ns,
                location: Location::Cloud(place),
                link: false,
                etag: e.etag.clone(),
            }));
        }
        let mut path = PathBuf::from(root);
        path.extend(parts);
        let real = realpath(&path);
        if !inside(&real.to_string_lossy(), root) {
            return Err(Error::User(format!(
                "leads out of {label} or into a hidden folder."
            )));
        }
        // Left out by a filter, under its name or where it leads, reads as missing.
        let Ok(st) = std::fs::metadata(&real) else {
            return Ok(None);
        };
        let is_dir = st.is_dir();
        if !kept(self.rules, &shown, is_dir) || !kept_target(self.rules, label, root, &real, is_dir)
        {
            return Ok(None);
        }
        Ok(Some(Item {
            shown,
            is_dir,
            size: st.len() as i64,
            mtime_ns: st.mtime() * 1_000_000_000 + st.mtime_nsec(),
            ctime_ns: st.ctime() * 1_000_000_000 + st.ctime_nsec(),
            location: Location::Disk(real),
            link: false,
            etag: String::new(),
        }))
    }

    /// The Items in a folder Item, sorted by name.
    pub fn children(&mut self, folder: &Item) -> Result<Vec<Item>, Error> {
        let label = folder.shown.split('/').next().unwrap_or("").to_string();
        let real = match &folder.location {
            Location::Cloud(place) => return self.cloud_children(folder, place),
            Location::Disk(real) => real,
        };
        let root = self.root(&label).to_string();
        let mut items = Vec::new();
        for e in std::fs::read_dir(real)? {
            let Ok(e) = e else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if hidden(&name) {
                continue;
            }
            let shown = format!("{}/{name}", folder.shown);
            // A broken link.
            let Ok(st) = std::fs::metadata(e.path()) else {
                continue;
            };
            if !st.is_dir() && !st.is_file() {
                continue;
            }
            let link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            // A link is judged by where it leads too.
            if !kept(self.rules, &shown, st.is_dir())
                || (link && !kept_target(self.rules, &label, &root, &e.path(), st.is_dir()))
            {
                continue;
            }
            items.push(Item {
                shown,
                is_dir: st.is_dir(),
                size: st.len() as i64,
                mtime_ns: st.mtime() * 1_000_000_000 + st.mtime_nsec(),
                ctime_ns: st.ctime() * 1_000_000_000 + st.ctime_nsec(),
                location: Location::Disk(e.path()),
                link,
                etag: String::new(),
            });
        }
        items.sort_by(|a, b| a.shown.cmp(&b.shown));
        Ok(items)
    }

    fn cloud_children(&mut self, folder: &Item, place: &str) -> Result<Vec<Item>, Error> {
        let entries = match self.cache.get(place) {
            Some((etag, entries)) if !folder.etag.is_empty() && *etag == folder.etag => {
                entries.clone()
            }
            _ => match self.cloud()?.list(place, 1)? {
                Some(mut entries) => entries.split_off(1),
                None => return Err(Error::NotFound),
            },
        };
        self.listed
            .insert(place.to_string(), (folder.etag.clone(), entries.clone()));
        let mut items = Vec::new();
        for e in entries {
            let name = e.path.rsplit('/').next().unwrap_or("").to_string();
            let shown = format!("{}/{name}", folder.shown);
            if hidden(&name) || !kept(self.rules, &shown, e.is_dir) {
                continue;
            }
            items.push(Item {
                shown,
                is_dir: e.is_dir,
                size: e.size,
                mtime_ns: e.mtime_ns,
                ctime_ns: e.mtime_ns,
                location: Location::Cloud(e.path.clone()),
                link: false,
                etag: e.etag.clone(),
            });
        }
        Ok(items)
    }

    /// Every Item below a folder, depth first. Links to folders are not
    /// followed. A folder that cannot be listed is passed over, a Nextcloud
    /// one noted in `unlisted`; a refused app password is the only error.
    pub fn walk(&mut self, folder: &Item) -> Result<Vec<Item>, Error> {
        let mut out = Vec::new();
        self.walk_into(folder, &mut out)?;
        Ok(out)
    }

    fn walk_into(&mut self, folder: &Item, out: &mut Vec<Item>) -> Result<(), Error> {
        let items = match self.children(folder) {
            Ok(items) => items,
            Err(e @ Error::Nextcloud { refused: true, .. }) => return Err(e),
            Err(Error::Nextcloud { .. }) => {
                self.unlisted.push(folder.shown.clone());
                return Ok(());
            }
            Err(_) => return Ok(()),
        };
        for item in items {
            let descend = item.is_dir && !item.link;
            out.push(item.clone());
            if descend {
                self.walk_into(&item, out)?;
            }
        }
        Ok(())
    }

    /// A file to read the Item from. A Nextcloud file is fetched whole, up to limit.
    pub fn open(&self, item: &Item, limit: u64) -> Result<Source, Error> {
        match &item.location {
            Location::Cloud(place) => Ok(Source::Fetched(Cursor::new(
                self.cloud()?.get(place, limit)?,
            ))),
            Location::Disk(real) => Ok(Source::Disk(File::open(real)?)),
        }
    }

    /// Write data to the file at label/parts, creating it and its missing
    /// folders, or replacing it; returns "created" or "replaced". With data
    /// None, make the folder at label/parts and those above it; returns
    /// "created" or "exists". Never through a link, never in place of a
    /// folder. The caller checks that the folder is writable and that parts
    /// holds no "..", no hidden name.
    ///
    /// On disk, what it creates takes its folder's group, and with the
    /// unit's UMask=0007 mode 660 or 770, so the folder's owner can change
    /// it; a replaced file keeps its owner and mode. Refused when the disk
    /// would be left with less than MIN_FREE_BYTES.
    pub fn write(
        &self,
        label: &str,
        parts: &[String],
        data: Option<&[u8]>,
    ) -> Result<&'static str, Error> {
        let root = self.root(label).to_string();
        let shown = std::iter::once(label)
            .chain(parts.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("/");
        if !kept(self.rules, &shown, data.is_none()) {
            return Err(Error::User(
                "the folder's filters leave this name out, so it could not be read back.".into(),
            ));
        }
        if let Some(account) = root.strip_prefix(NEXTCLOUD) {
            return self.write_cloud(label, account, parts, data);
        }
        let folders = if data.is_none() {
            parts.len()
        } else {
            parts.len() - 1
        };
        let (d, made) = open_folders(label, &root, &parts[..folders], true)?;
        let Some(data) = data else {
            return Ok(if made { "created" } else { "exists" });
        };
        let free = nix::sys::statvfs::fstatvfs(&d)?;
        if (free.blocks_available() as u64) * (free.fragment_size() as u64)
            < data.len() as u64 + MIN_FREE_BYTES
        {
            return Err(Error::User(
                "the disk is nearly full; nothing was written.".into(),
            ));
        }
        let name = parts.last().map(String::as_str).unwrap_or("");
        // Never through a link; O_NONBLOCK so a pipe cannot hang the open.
        let flags = OFlag::O_WRONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
        let (fd, outcome) = match openat(
            &d,
            name,
            flags | OFlag::O_CREAT | OFlag::O_EXCL,
            Mode::from_bits_truncate(0o660),
        ) {
            Ok(fd) => {
                if let Err(e) = group(&fd, &d) {
                    drop(fd);
                    let _ = unlinkat(&d, name, UnlinkatFlags::NoRemoveDir);
                    return Err(e);
                }
                (fd, "created")
            }
            Err(Errno::EEXIST) => match openat(&d, name, flags, Mode::empty()) {
                Ok(fd) => (fd, "replaced"),
                Err(Errno::ELOOP) => {
                    return Err(Error::User("is a link; it is not replaced.".into()));
                }
                Err(e) => return Err(e.into()),
            },
            Err(e) => return Err(e.into()),
        };
        let st = fstat(&fd)?;
        if st.st_mode & libc_s_ifmt() != libc_s_ifreg() {
            return Err(Error::User("is not a plain file.".into()));
        }
        // A hard link could be a file outside the folder.
        if st.st_nlink > 1 {
            return Err(Error::User(
                "has other names (hard links); it is not replaced.".into(),
            ));
        }
        let mut file = File::from(fd);
        file.set_len(0)?;
        file.write_all(data)?;
        Ok(outcome)
    }

    fn write_cloud(
        &self,
        label: &str,
        account: &str,
        parts: &[String],
        data: Option<&[u8]>,
    ) -> Result<&'static str, Error> {
        let nc = self.cloud()?;
        let mut place: Vec<&str> = account.split('/').filter(|p| !p.is_empty()).collect();
        let base = place.len();
        place.extend(parts.iter().map(String::as_str));
        let mut made = false;
        let end = place.len() + usize::from(data.is_none());
        for n in base + 1..end {
            let at = place[..n].join("/");
            match nc.list(&at, 0)? {
                None => {
                    made = true;
                    nc.send("MKCOL", &at, None, None, &[201])?;
                }
                Some(entries) => {
                    made = false;
                    if !entries[0].is_dir {
                        let shown = std::iter::once(label)
                            .chain(parts[..n - base].iter().map(String::as_str))
                            .collect::<Vec<_>>()
                            .join("/");
                        return Err(Error::User(format!("{shown} is a file.")));
                    }
                }
            }
        }
        let Some(data) = data else {
            return Ok(if made { "created" } else { "exists" });
        };
        let at = place.join("/");
        if let Some(here) = nc.list(&at, 0)?
            && here[0].is_dir
        {
            return Err(Error::User("is a folder.".into()));
        }
        Ok(
            if nc.send("PUT", &at, Some(data), None, &[201, 204])? == 201 {
                "created"
            } else {
                "replaced"
            },
        )
    }
}

impl Tree<'_> {
    /// Move or rename label/from to label/to, a file or a folder of any
    /// kind, in one folder. Never replaces what is at to, never moves a
    /// link, never a folder into itself; to's folder must exist. Both
    /// names must be ones the filters keep. The caller checks that the
    /// folder is writable and that from and to hold no "..", no hidden name.
    pub fn rename(&self, label: &str, from: &[String], to: &[String]) -> Result<(), Error> {
        let Some(item) = self.item(label, from)? else {
            return Err(Error::NotFound);
        };
        let shown = std::iter::once(label)
            .chain(to.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("/");
        if !kept(self.rules, &shown, item.is_dir) {
            return Err(Error::User(format!(
                "the folder's filters leave {shown} out, so it could not be found again."
            )));
        }
        // Moving a folder changes the folders above each file in it. A pattern
        // with a / may then judge them otherwise, and so may an "only" name
        // that matches one path's folders but not the other's: the move could
        // show what the filters hide, or hide what they show. A skip name
        // cannot: no name of the source matches it, and kept() checked to's.
        let lower = |parts: &[String]| {
            std::iter::once(label)
                .chain(parts.iter().map(String::as_str))
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
        };
        let (old, new) = (lower(from), lower(to));
        let (old, new): (Vec<&str>, Vec<&str>) = (
            old.iter().map(String::as_str).collect(),
            new.iter().map(String::as_str).collect(),
        );
        if item.is_dir
            && let Some(rule) = self.rules.get(label)
            && (rule.only.iter().chain(&rule.skip).any(|p| p.contains('/'))
                || rule
                    .only
                    .iter()
                    .any(|p| matches(p, &old) != matches(p, &new)))
        {
            return Err(Error::User(format!(
                "{label}'s filters judge files by their folders, so its folders are not moved; move the files."
            )));
        }
        if to.starts_with(from) {
            return Err(Error::User(format!(
                "{shown} is the same place or inside it."
            )));
        }
        let (Some((name, from_up)), Some((new_name, to_up))) = (from.split_last(), to.split_last())
        else {
            return Err(Error::User("name something inside the folder.".into()));
        };
        if let Location::Cloud(place) = &item.location {
            let account = self.root(label).strip_prefix(NEXTCLOUD).unwrap_or("");
            let target = std::iter::once(account)
                .chain(to.iter().map(String::as_str))
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("/");
            // Between two owners' files Nextcloud copies then deletes: a move
            // out of a share would take the file from its owner.
            let nc = self.cloud()?;
            let up = target.rsplit_once('/').map(|(u, _)| u).unwrap_or("");
            let (here, there) = (nc.owner(place)?, nc.owner(up)?);
            if there.is_none() {
                return Err(Error::User(format!(
                    "the folder {shown} would go in does not exist; create it with create_folder."
                )));
            }
            if here != there {
                return Err(Error::User(
                    "a move stays within one account's files; this one is shared from another."
                        .into(),
                ));
            }
            return match nc.send("MOVE", place, None, Some(&target), &[201, 409, 412])? {
                409 => Err(Error::User(format!(
                    "the folder {shown} would go in does not exist; create it with create_folder."
                ))),
                412 => Err(Error::User(format!(
                    "{shown} already exists; nothing is replaced."
                ))),
                _ => Ok(()),
            };
        }
        let root = self.root(label).to_string();
        let (src, _) = open_folders(label, &root, from_up, false)?;
        let (dst, _) = open_folders(label, &root, to_up, false)?;
        let st = fstatat(&src, name.as_str(), AtFlags::AT_SYMLINK_NOFOLLOW)?;
        if st.st_mode & libc_s_ifmt() == libc_s_iflnk() {
            return Err(Error::User("is a link; links are not moved.".into()));
        }
        match renameat2(
            &src,
            name.as_str(),
            &dst,
            new_name.as_str(),
            RenameFlags::RENAME_NOREPLACE,
        ) {
            Ok(()) => Ok(()),
            Err(Errno::EEXIST) => Err(Error::User(format!(
                "{shown} already exists; nothing is replaced."
            ))),
            Err(e) => Err(e.into()),
        }
    }
}

/// The folder at root/parts, each opened from the one above it, never by its
/// path, so no link swapped in on the way is followed; and whether the last
/// one was made. With make, missing folders are made in their parent's
/// group, mode 770 through the unit's umask; without, a missing one is an error.
fn open_folders(
    label: &str,
    root: &str,
    parts: &[String],
    make: bool,
) -> Result<(OwnedFd, bool), Error> {
    let shown_upto = |n: usize| {
        std::iter::once(label)
            .chain(parts[..n].iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("/")
    };
    let mut d: OwnedFd = File::open(root)?.into();
    let mut made = false;
    for (n, name) in parts.iter().enumerate() {
        if make {
            made = match mkdirat(&d, name.as_str(), Mode::from_bits_truncate(0o770)) {
                Ok(()) => true,
                Err(Errno::EEXIST) => false,
                Err(e) => return Err(e.into()),
            };
        }
        let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
        let inner = match openat(&d, name.as_str(), flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::ENOTDIR | Errno::ELOOP) => {
                return Err(Error::User(format!(
                    "{} is a file or a link.",
                    shown_upto(n + 1)
                )));
            }
            Err(Errno::ENOENT) => {
                return Err(Error::User(format!(
                    "{} does not exist; create it with create_folder.",
                    shown_upto(n + 1)
                )));
            }
            Err(e) => return Err(e.into()),
        };
        if made && let Err(e) = group(&inner, &d) {
            drop(inner);
            let _ = unlinkat(&d, name.as_str(), UnlinkatFlags::RemoveDir);
            return Err(e);
        }
        d = inner;
    }
    Ok((d, made))
}

const fn libc_s_iflnk() -> u32 {
    0o120000
}

const fn libc_s_ifmt() -> u32 {
    0o170000
}

const fn libc_s_ifreg() -> u32 {
    0o100000
}

/// Gives what fd opens its parent folder's group, unless a setgid folder did already.
fn group(fd: &OwnedFd, parent: &OwnedFd) -> Result<(), Error> {
    let gid = fstat(parent.as_fd())?.st_gid;
    if fstat(fd.as_fd())?.st_gid != gid {
        fchown(fd.as_fd(), None, Some(Gid::from_raw(gid)))?;
    }
    Ok(())
}

/// Syncthing's top-level folders, one "<label>\t<path>\t<ignorePerms>\t<type>"
/// each, type such as sendreceive or receiveonly, from its config.xml. The
/// folder under <defaults> is a template. None when it cannot be read.
pub fn syncthing_folders(xml: &[u8]) -> Option<Vec<String>> {
    let root = crate::text::parse_xml(xml).ok()?;
    let mut out = Vec::new();
    for folder in root.elements().filter(|e| e.tag == "folder") {
        let label = folder
            .get("label")
            .filter(|l| !l.is_empty())
            .or(folder.get("id"))
            .unwrap_or("");
        let path = folder.get("path").unwrap_or("");
        if label.is_empty()
            || path.is_empty()
            || format!("{label}{path}").contains(['\t', '\n', ','])
        {
            continue;
        }
        let ignore = folder.get("ignorePerms").unwrap_or("false");
        let kind = folder
            .get("type")
            .filter(|t| !t.is_empty())
            .unwrap_or("sendreceive");
        out.push(format!("{label}\t{path}\t{ignore}\t{kind}"));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(only: &[&str], skip: &[&str]) -> HashMap<String, Rules> {
        let s = |v: &[&str]| {
            v.iter()
                .map(|p| p.to_lowercase().trim_matches('/').to_string())
                .collect()
        };
        HashMap::from([(
            "Documents".to_string(),
            Rules {
                only: s(only),
                skip: s(skip),
            },
        )])
    }

    #[test]
    fn globs_match_as_fnmatch() {
        let m = |n: &str, p: &str| {
            fnmatch(
                &n.chars().collect::<Vec<_>>(),
                &p.chars().collect::<Vec<_>>(),
            )
        };
        assert!(m("a/b/c.pdf", "*.pdf"));
        assert!(m("documents/x/2019", "documents/*/2019"));
        assert!(m("x1", "x[0-9]"));
        assert!(!m("xa", "x[0-9]"));
        assert!(m("xa", "x[!0-9]"));
        assert!(m("[a", "[a"));
        assert!(m("a?c", "a?c") && m("abc", "a?c"));
    }

    #[test]
    fn filters_skip_and_keep() {
        let r = rules(&[], &["brouillons", "/Documents/*.tmp.md"]);
        assert!(!kept(&r, "Documents/Brouillons/draft.md", false));
        assert!(!kept(&r, "Documents/Factures/Copie.TMP.md", false));
        assert!(kept(&r, "Documents/Factures/x.md", false));
        let r = rules(&["*.md"], &[]);
        assert!(kept(&r, "Documents/a", true));
        assert!(!kept(&r, "Documents/photo.png", false));
        assert!(kept(&r, "Other/photo.png", false));
    }

    #[test]
    fn suffixes_as_splitext() {
        assert_eq!(suffix("Documents/a.B.PDF"), ".pdf");
        assert_eq!(suffix("Documents/.bashrc"), "");
        assert_eq!(suffix("Documents/x"), "");
    }

    #[test]
    fn repr_as_python() {
        assert_eq!(py_repr("alias"), "'alias'");
        assert_eq!(py_repr("l'été"), "\"l'été\"");
    }
}
