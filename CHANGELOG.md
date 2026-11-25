# Changelog

## Unreleased

- How to add the server to Whoopy since its 0.2.0~ynh5: on Whoopy's status page, as the server `docs`; a new token is given to it in Whoopy's admin page.

## 0.11.0~ynh2

- `sudo cheruby-docs-mcp token` shows the address and the tokens, and `token --new` makes a new Whoopy token, as the panel's "New token" does. Every other command of the binary answers as `sudo cheruby-docs-mcp <command>`.
- The panel's "Whoopy token" section is now "MCP token", and its setting `mcp_token`, as in the other MCP apps here. The upgrade keeps the token as it is: Whoopy needs nothing.
- Each tool, its arguments and its limits in [MCP tools](doc/mcp.md).

## 0.11.0

- The tool server, the indexer and their commands are one binary built for the Pi 5, in place of the Python and its virtual environment. The index is kept.
- `run_index`: an agent asks for an index run of some files, such as one it just wrote, or of all the folders, so search finds what changed. Refused in the quiet hours, past 50 files and while 20 requests wait.
- The binary answers on the shell as root: `ls`, `find`, `read`, `search`, `run-index`, `status` and `check-desktop`.

## 0.10.1

- A redrawn logo.

## 0.10.0

- "Run now on the desktop" in "Index runs": the passages are turned into vectors by the desktop's GPU through Whoopy, so a first index of many files no longer takes hours. Set the model as Whoopy names it and a Whoopy key under "Desktop embedding". The run checks first that the desktop's vectors agree with the server's, and goes on on the server when they do not or when Whoopy stops answering. The schedule never uses it.
- An index run stops, instead of marking each file failed, when the embedding server refuses it or holds it back.

## 0.9.1

- Qwen3-Embedding can be picked in the panel: its two choices were unreadable and the panel refused them.

## 0.9.0

- "Embedding model" in the config panel: EmbeddingGemma 300M (the default, as before), jina-embeddings-v5-text-nano (non-commercial use only) or Qwen3-Embedding 0.6B, each in Q8 or a smaller Q4. Only the chosen model is downloaded. A change makes the next index run read every file again; until then search matches keywords only, and says so.
- Install, upgrade and restore no longer compile llama-server on the server, about 8 minutes each: they download a release built for the Raspberry Pi 5 and pinned by its checksum. The build tools are no longer installed.
- The first index run after the upgrade reads every file again, about half an hour, since the new llama-server counts as a new model; search matches keywords only until its first results are published.

## 0.8.0

- Search passages follow the document's headings: a short note or section stays whole, a longer one is cut at its subheadings, and each passage starts with its file name and headings. Passages are about half as long, so an agent reads less beside the answer. The first index run after the upgrade reads every file again, about half an hour, while search answers from what it has done so far.
- `read_file` shows the headings of Word and OpenDocument files as Markdown `#` lines.

## 0.7.0

- Search orders its results by how well they answer the question, with a reranking model run on the server, and shows the best passage of each file. On the author's test queries, 9 of 9 expected documents come in the top 5, against 7 before, in about 4 seconds a search. The model, jina-reranker-v2-base-multilingual, is for non-commercial use only.
- When the reranking model does not answer, search still answers, in a rougher order, and says so.

## 0.6.2

- "Whoopy access" is picked from the open folders instead of typed, so a mistyped label can no longer be saved.

## 0.6.1

- An index run stuck past 6 hours now fails, so the timer starts the next one.
- A Nextcloud app password refused once is not tried again until a new one is saved: each failed login made Nextcloud hold back logins from every app on the server. When it does, the panel says so and how to clear it.
- The panel checks again whether the app can read each folder on disk; it said "could not be checked" for all of them.

## 0.6.0

- "Writable folders" per source, and a `write_file` tool that creates or replaces a text file in them, never deletes. Never through a link, never a file with other hard links, never the last 2 GB of the disk; the panel says what a folder's group cannot change.
- A `create_folder` tool beside `write_file`.
- The index runs every hour, every 6 hours, every night, never, or on a custom cron schedule, chosen in the panel, never in the quiet hours set there; a run still going stops when they begin and the next one carries on.
- "Index runs" in the panel: how the last run ended, what the index holds, the next run, and "Run now" and "Rebuild" buttons.

## 0.5.0

- A Nextcloud source: folders of a Nextcloud account, shares from others included, read through Nextcloud with an app password made there and pasted in the panel. The account can be another login than the user, given read-only shares.
- The panel shows, per Nextcloud folder, whether Nextcloud lets the app read it.
- The index lists again only the Nextcloud folders that changed; when Nextcloud does not answer, its passages stay and the run is reported as failed.

## 0.4.0

- One user, chosen at install; their folders come from sources chosen in the config panel: Syncthing folders ticked from Syncthing's list, and local folders.
- "Only these files" and "Leave out" patterns per source; what they leave out cannot be listed, found, read or searched, even through a link.
- The panel shows, per folder, the files the app cannot read, and whether Syncthing ignores permissions.
- A folder just added no longer stops search until the next index run.
- The upgrade turns the old `folders` setting into the new sources.

## 0.1.0 (unreleased)

- Lists, finds by name and reads each user's folders over MCP: text, Markdown, PDF by page, Word, OpenDocument and images.
- Each user's token reaches that user's folders only; nothing is ever written.
