# Changelog

## 0.4.0

- One user, chosen at install; their folders come from sources chosen in the config panel: Syncthing folders ticked from Syncthing's list, and local folders.
- "Only these files" and "Leave out" patterns per source; what they leave out cannot be listed, found, read or searched, even through a link.
- The panel shows, per folder, the files the app cannot read, and whether Syncthing ignores permissions.
- A folder just added no longer stops search until the next index run.
- The upgrade turns the old `folders` setting into the new sources.

## 0.1.0 (unreleased)

- Lists, finds by name and reads each user's folders over MCP: text, Markdown, PDF by page, Word, OpenDocument and images.
- Each user's token reaches that user's folders only; nothing is ever written.
