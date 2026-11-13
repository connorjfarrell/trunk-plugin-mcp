# Changelog

Each version's section is its GitHub release notes.

## [0.1.0] — 2026-11-13

- First release: serves an MCP server over HTTP (`POST /mcp`, optional bearer token) with tools to see what is on the radio now,
  search transcripts, read a window's traffic, follow an incident across talkgroups, list talkgroups and read one call.
- Indexes the recordings folder's calls and transcripts in SQLite with full-text search, newest day first, at a lower priority than the recorder.
- Clients are let in by network (`allowedNetworks`, default this machine) as well as by token.
- Encrypted calls are left out unless turned on (for the plugin, or for one system); a system can be left out altogether.
