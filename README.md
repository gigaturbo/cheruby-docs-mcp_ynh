# Cheruby Docs for YunoHost

Read-only MCP tools that let each user's AI agents list, find and read that
user's folders on the server. Meant to sit behind an MCP gateway such as
LiteLLM, which holds one entry per user with that user's token.

## Tools

- `list_folder(path)`: a folder's subfolders, then its files with size and date.
- `find_files(query, path)`: files whose path holds every word, ignoring case and accents.
- `read_file(path, ...)`: text and Markdown by character range, PDF by page,
  Word and OpenDocument, and images as pictures.

Paths start with a folder's label, such as `Documents/Factures`.

## Install

The `folders` question takes one line per folder: `<user> <Label>=<path>`.
Each user gets a token, read with
`yunohost app setting cheruby-docs-mcp token_<user>`. The server listens on
`http://127.0.0.1:<port>/mcp` (Streamable HTTP); the port is the app's `port`
setting.

The service joins each folder's group to read it, so the files must be
group-readable. Everything else on the server stays hidden from it, and it
can reach nothing but loopback.
