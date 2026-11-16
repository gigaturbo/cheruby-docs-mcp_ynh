<p align="center"><img src="doc/logo.svg" width="160" alt="Cheruby Docs"></p>

# Cheruby Docs for YunoHost

MCP tools that let AI agents list, find, read and search by meaning one
user's folders on the server, and write text files in the folders marked
writable. Meant to sit behind an MCP gateway.

## Tools

- `list_folder(path)`: a folder's subfolders, then its files with size and date.
- `find_files(query, path)`: files whose path holds every word, ignoring case and accents.
- `read_file(path, ...)`: text and Markdown by character range, PDF by page,
  Word and OpenDocument, and images as pictures.
- `search_documents(query, path, after, before, limit)`: the best passage of
  each matching file, found by meaning and keywords, then ordered by how
  well it answers the query. Searches an index refreshed on the schedule
  chosen in the panel.
- `write_file(path, content)`: creates a text file, with its missing folders,
  or replaces one, in a writable folder. Offered only when a folder is
  writable; it never deletes.
- `create_folder(path)`: creates a folder, with those above it, in a
  writable folder.

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

Each source also has "Only these files" and "Leave out" patterns, such as
`*.pdf`, `Archives` or `Documents/Old`. What they leave out cannot be listed,
found, read or searched. Under each source, the panel shows which folders hold
files the app cannot read.

"Writable folders", per source, lists the folders the agents may write in.
A change to a Syncthing folder goes to every device, and its old content
survives only on the devices with File Versioning on; a receive-only folder
stays read-only. A folder on disk needs its files changeable by its group,
and the panel says which are not. The multimedia and backup shares always
stay read-only. A Nextcloud folder needs a
login allowed to change it; Nextcloud keeps the old versions.

Under "Search index", the index is refreshed every hour, every 6 hours,
every night, never, or on a custom schedule in cron's five fields, and
never in the quiet hours (01 to 06 by default, around the backups): no run
starts then, and one still going stops and carries on at the next run. "Index runs" shows how the last run ended, what the
index holds and when the next run comes, with "Run now" and "Rebuild",
which makes the next run read every file again.

The server listens on `http://127.0.0.1:<port>/mcp` (Streamable HTTP).

The gateway gets its own token, shown in the config panel under "Whoopy
token", with the address to paste beside it. It opens all the folders, or the
ones listed under "Whoopy access", and searches the user's index. "New token"
replaces it. The user also has a token of their own, on all the folders, read
with `yunohost app setting cheruby-docs-mcp token_<user>`.

Search orders its results with jina-reranker-v2-base-multilingual, which
the install downloads from Hugging Face. Its licence, CC-BY-NC-4.0, allows
non-commercial use only. When it does not answer, search still works, in a
rougher order.

The service joins each folder's group to read the folders on disk, so their
files must be group-readable. It sees the folders read-only, but for the
writable ones. Everything else on the server stays hidden from it, and it
can reach nothing but loopback.
