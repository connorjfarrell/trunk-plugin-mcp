# Claude (MCP) — a Trunk Recorder Pro plugin

Lets Claude, or any [MCP](https://modelcontextprotocol.io) client, ask about the radio traffic your recorder has heard:
"I just heard sirens" → what was dispatched, what the units said, on which talkgroups.

It reads the recordings folder (each call's JSON, and the `.transcript.json` a transcription plugin such as
[Whisper](https://github.com/connorjfarrell/trunk-plugin-whisper) writes beside it), keeps a full-text index in the plugin's
data folder, and serves MCP over HTTP at `http://host:port/mcp`. It only reads: it never changes a recording.

## Tools

| Tool | For |
|---|---|
| `recent_activity` | what is on the radio now: the busiest talkgroups of the last N minutes, emergency-flagged calls, the newest transcripts |
| `search_traffic` | search what was said (`cardiac arrest`, `"structure fire"`, `seiz*`, `a OR b`), by time, talkgroup, agency, radio |
| `get_traffic` | a window's traffic in time order: read a conversation |
| `incident_context` | the traffic around one call: its talkgroup, and other talkgroups linked to it by the radios that answered it, and by the units and numbers its dispatch named |
| `list_talkgroups` | which talkgroups were active, with their numbers |
| `get_call` | one call in full, and where its audio is |
| `index_status` | how much has been read |

Times are your local time: `30m`, `2h ago`, `10:30`, `yesterday 22:30`, `2026-11-05 10:30`.

## Setting it up

1. Install the plugin (Setup → Plugins), and set a **Port** (8770), an **Access token**, and whether to **Include encrypted calls**.
   It listens on this machine only (`127.0.0.1`) unless you set **Listen on** to `0.0.0.0`, which needs a token of 16 characters or more.
   **Networks allowed to connect** (default this machine) limits who may connect at all: `172.16.0.0/12, 192.168.0.0/16` is the private networks. The connection is plain HTTP: on a network you don't trust, put a TLS proxy in front.
2. Add it to Claude Code:

   ```sh
   claude mcp add --transport http trunk-recorder http://127.0.0.1:8770/mcp --header "Authorization: Bearer YOUR-TOKEN"
   ```

   Other clients take the same URL and header.
3. Ask: "I just heard sirens: what was that?"

The first start reads the whole window (30 days by default), newest day first: about 700,000 calls took under three minutes on a
busy system, at a lower CPU and disk priority than the recorder. The newest traffic can be asked about while it runs, and the plugin's
status line says how far it is. After that, only the last two days are looked at, every 20 seconds and whenever a call concludes.

## Encrypted calls

What the tools return goes to the assistant's model, so it leaves your machine. Calls that were encrypted (and that the recorder
decrypted with your keys) are therefore left out unless you turn on **Include encrypted calls**, and for the recorder to hand such
calls to a plugin at all, **Share decrypted calls** has to be on for it too (the plugin's card in Setup → Plugins). A system can
override the setting in its own settings, and **Leave this system out** keeps a system's calls from being indexed at all.

## Limits

* Transcripts are machine-made: names, streets and numbers are often wrong. A call with no transcript is listed as such.
* An incident's other talkgroups are found by a heuristic (radios that answered; units and numbers the dispatch named): each line says why
  it is there, and it can include a radio's unrelated traffic or miss a unit that never spoke.
* The two sites' copies of a transmission are shown as one line when they say the same thing within a moment.
* Only folders laid out `<system>/<year>/<month>/<day>/` are read.
* Calls the recorder's retention deletes stay in the index until they pass out of the window.
