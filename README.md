<p align="center"><img src="doc/logo.svg" width="160" alt="Cheruby Docs"></p>

# Cheruby Docs for YunoHost

MCP tools that let AI agents list, find, read and search by meaning one
user's folders on the server, and write text files in the folders marked
writable. Meant to sit behind an MCP gateway.

## Tools

- `list_folder`, `find_files`, `read_file`: a folder's content, files by
  name, and one file: text, PDF by page, images as pictures, Word, Excel,
  PowerPoint, OpenDocument, EPUB and saved emails.
- `search_documents`: the best passage of each matching file, by meaning
  and keywords, from an index refreshed on the schedule chosen in the panel.
- `write_file`, `create_folder`, `move_file`: a text file or a folder, made,
  replaced, moved or renamed in a folder marked writable, never a delete.
- `run_index`: an index run of some files or of everything, so search finds
  what changed.

Each tool, its arguments and its limits: [MCP tools](doc/mcp.md).

## The page

The user chosen at install, and only them, opens the app's page through the
portal:

- **Status**: how the last index run went and what the index holds, whether
  the embedding and ranking servers answer, and which folders can be read
  or written.
- **Search**: the same search as `search_documents`, the best passage of
  each file.

Paths start with a folder's label, such as `Documents/Factures`.

## Install

The install asks for one user. Their folders are chosen afterwards in the
config panel, from three sources, each turned on or off:

- **Syncthing**: the folders Syncthing syncs on the server, ticked from a list
  (empty opens them all), each under its Syncthing label.
- **Local storage**: any folder, one per line as `<Label>=<path>`.
- **Nextcloud**: folders of a Nextcloud account, shares included, one per
  line as `<Label>=<folder in the account>` (empty opens all of it). They are
  read through Nextcloud with an app password made in Nextcloud (Settings,
  Security) and pasted in the panel. An app password can also change files:
  give another login, holding read-only shares of the folders, to keep the
  app from changing anything.

Under each source, the panel shows which folders hold files the app cannot
read. Which files of a source the agents see is set on the app's page (below).

"Writable folders", per source, lists the folders the agents may write in.
A change to a Syncthing folder goes to every device, and its old content
survives only on the devices with File Versioning on; a receive-only folder
stays read-only. A folder on disk needs its files changeable by its group,
and the panel says which are not. The multimedia and backup shares always
stay read-only. A Nextcloud folder needs a
login allowed to change it; Nextcloud keeps the old versions.

"Index runs" shows how the last run ended, what the index holds and when
the next run comes, with "Run now" and "Rebuild", which makes the next run
read every file again. "Index monitoring" pings a healthchecks.io check when
any run fails, and on the success of a run the schedule or Run now started,
so a check with the schedule's period alerts when runs stop.

## The page

The app's page, at its domain and path behind the portal, opens for the
docs user only. Status shows the index runs, the servers and the folders;
Search finds documents as the agents' search does. Config holds what does
not confine the app, applied at once with no restart:

- **Index schedule**: every hour, every 6 hours, every night, never, or a
  custom schedule in cron's five fields, and never in the quiet hours (01 to
  06 by default, around the backups): no run starts then, and one still
  going stops and carries on at the next run.
- **Filters**: per source, "Only these files" and "Leave out" patterns, such
  as `*.pdf`, `Archives` or `Documents/Old`. What they leave out cannot be
  listed, found, read or searched.

They are kept in `/var/lib/cheruby-docs-mcp/settings.json`, backed up with
the app.

"Embedding model" picks what search compares meanings with: EmbeddingGemma
300M (the default), jina-embeddings-v5-text-nano or Qwen3-Embedding 0.6B,
each in a Q8 and a smaller Q4. Only the chosen one is downloaded. A change
makes the next run read every file again; until then search matches
keywords only.

"Run now on the desktop", under "Index runs", is for a first index of many
files, which takes hours on the server. It reads the files on the server
but has another machine's GPU, reached through the Whoopy gateway, turn the
passages into vectors. Under "Desktop embedding", give the model as Whoopy
names it there and paste a Whoopy key allowed that model only. Search still
turns each question into a vector on the server, so the run first does about
20 passages both ways and keeps to the server unless they agree. When Whoopy
stops answering, the run carries on on the server. The schedule never uses it.

The server listens on `http://127.0.0.1:<port>/mcp` (Streamable HTTP).
It is one binary, built for the Pi 5 from `cheruby-docs/` in this
repository and downloaded at install, with PDFium to read PDFs.

The gateway gets its own token, shown in the config panel under "MCP
token", with its address beside it: add both on Whoopy's status page (MCP
Servers tab) as the server `docs`. It opens all the folders, or the
ones listed under "Whoopy access", and searches the user's index. "New token",
or `sudo cheruby-docs-mcp token --new`, replaces it; give the new one to the
server `docs` in Whoopy's admin page, MCP Servers. The user also has a token
of their own, on all the folders. `sudo cheruby-docs-mcp token` shows the
address and both tokens. Whoopy publishes the tools to the local network and
WireGuard only.

Search orders its results with jina-reranker-v2-base-multilingual, which
the install downloads from Hugging Face. Its licence, CC-BY-NC-4.0, allows
non-commercial use only, as does jina-embeddings-v5-text-nano's. When it does not answer, search still works, in a
rougher order.

The service joins each folder's group to read the folders on disk, so their
files must be group-readable. It sees the folders read-only, but for the
writable ones. Everything else on the server stays hidden from it, and it
can reach nothing but loopback.

## Commands

As root, `sudo cheruby-docs-mcp <command>`, with the app's own settings.
Each command that does what a tool does is named beside it.

- **`token [--new]`**: the address and the tokens; `--new` makes a new
  Whoopy token, as the panel's "New token" does.
- **`ls`, `find`, `read`, `search`**: `list_folder`, `find_files`,
  `read_file` and `search_documents`, as the user's own token, or
  as the Whoopy token with `--gateway`. They first drop root for the app's
  user and its folders' groups, as the service runs.
- **`run-index [path...]`**: asks for an index run, as `run_index` does.
- **`status`**: the index runs, as the panel shows them.
- **`check-desktop`**: whether the desktop's vectors, through Whoopy, agree
  with the server's, without starting a run.
