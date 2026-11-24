# Changelog

Each version's section is its GitHub release notes.

## [0.2.0] — 2026-11-23

- New `get_audio` tool: a call's recording is returned to the client as an audio file (WAV, M4A or MP3, as the recorder saved it), up to a new
  **Largest recording to send in a reply** setting (10 MB).
- New `GET /audio/<call id>` on the server downloads the same file, with the same token and network rules, for clients that would rather fetch than receive.
- Encrypted calls' recordings follow **Include encrypted calls** like their transcripts. `get_call` now names the real file (not always `.wav`).

## [0.1.0] — 2026-11-13

- First release: serves an MCP server over HTTP (`POST /mcp`, optional bearer token) with tools to see what is on the radio now,
  search transcripts, read a window's traffic, follow an incident across talkgroups, list talkgroups and read one call.
- Indexes the recordings folder's calls and transcripts in SQLite with full-text search, newest day first, at a lower priority than the recorder.
- Clients are let in by network (`allowedNetworks`, default this machine) as well as by token.
- Encrypted calls are left out unless turned on (for the plugin, or for one system); a system can be left out altogether.
