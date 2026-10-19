# untis-mcp

[![CI](https://github.com/DenizCiger/untis-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/DenizCiger/untis-mcp/actions/workflows/ci.yml)

[MCP](https://modelcontextprotocol.io) server for [WebUntis](https://webuntis.com), written in Rust.
It gives AI agents read-only access to timetables, timetable changes, free rooms, absences
and school years. The WebUntis client is ported from [`untis-tui`](https://github.com/DenizCiger/untis-tui).

## Tools

| Tool | What it does |
|---|---|
| `get_context` | Today's date and time, the connected school and account, and the current school year |
| `get_school_years` | School years, newest first, with the current one marked |
| `search_timetable_targets` | Fuzzy search over classes, rooms and teachers. Prefixes, initials and typos match |
| `get_day_timetable` | Lessons on one day (default today) |
| `get_week_timetable` | A full week plus the school's period grid |
| `get_timetable_changes` | Cancellations, substitutions, room changes, exams and additional lessons in a date range |
| `find_free_rooms` | Rooms without a lesson during a period (`"3"`, `"3-4"`), a time window, or right now |
| `get_absences` | Your absences with excuse status and totals |

The timetable tools read your own timetable by default. To read someone else's, pass
`target_type` (`class` / `room` / `teacher`) together with either `target_name` (e.g. `"4AHIF"`)
or `target_id`.
Dates can be `YYYY-MM-DD`, `DD.MM.YYYY`, `today`, `tomorrow` or `yesterday`.
Every tool is annotated read-only and returns structured content that has an output schema.

## Install

Download a prebuilt binary for Linux, macOS or Windows from the
[latest release](https://github.com/DenizCiger/untis-mcp/releases/latest), or build it with Cargo:

```sh
cargo install --git https://github.com/DenizCiger/untis-mcp
```

Or build from a checkout with `cargo build --release`; the binary is then `target/release/untis-mcp`.
Requires Rust 1.89 or newer.

## Configuration

Credentials come from environment variables, which you set in your MCP client's config:

| Variable | Example |
|---|---|
| `UNTIS_SERVER` | `nessa.webuntis.com` (a full WebUntis URL also works) |
| `UNTIS_SCHOOL` | the `school=` value from your WebUntis login URL |
| `UNTIS_USERNAME` | your username |
| `UNTIS_PASSWORD` | your password. Alternatively, put it in a file and set `UNTIS_PASSWORD_FILE` |

To check your credentials without starting the server:

```sh
UNTIS_SERVER=... UNTIS_SCHOOL=... UNTIS_USERNAME=... UNTIS_PASSWORD=... untis-mcp --check
```

To try the server without an account, run `--demo`. It serves the untis-tui sample data offline.

### Claude Code

```sh
claude mcp add untis \
  -e UNTIS_SERVER=nessa.webuntis.com -e UNTIS_SCHOOL=my-school \
  -e UNTIS_USERNAME=me -e UNTIS_PASSWORD=secret \
  -- /path/to/untis-mcp
```

### Claude Desktop and other JSON-configured clients

```json
{
  "mcpServers": {
    "untis": {
      "command": "/path/to/untis-mcp",
      "env": {
        "UNTIS_SERVER": "nessa.webuntis.com",
        "UNTIS_SCHOOL": "my-school",
        "UNTIS_USERNAME": "me",
        "UNTIS_PASSWORD": "secret"
      }
    }
  }
}
```

## Streamable HTTP

```sh
UNTIS_MCP_AUTH_TOKEN=$(openssl rand -hex 32) untis-mcp --transport http --bind 127.0.0.1:8787
# endpoint: http://127.0.0.1:8787/mcp   header: Authorization: Bearer <token>
```

| Flag / env | Default | Purpose |
|---|---|---|
| `--transport` / `UNTIS_MCP_TRANSPORT` | `stdio` | `stdio` or `http` |
| `--bind` / `UNTIS_MCP_BIND` | `127.0.0.1:8787` | listen address |
| `--path` / `UNTIS_MCP_PATH` | `/mcp` | endpoint path |
| `--auth-token` / `UNTIS_MCP_AUTH_TOKEN` | – | bearer token clients must send |
| `--allowed-host` / `UNTIS_MCP_ALLOWED_HOSTS` | loopback only | extra accepted `Host` headers, e.g. your proxy hostname |
| `--allow-unauthenticated` | off | allow a non-loopback bind without a token |

The HTTP server refuses to bind to a non-loopback address unless a token is set. It also rejects
unexpected `Host` headers, which protects against DNS rebinding. Anyone who can reach the
endpoint can read your timetable and absences, so keep the token secret.

## Behaviour notes

- **Sessions:** the server logs in once and reuses the WebUntis session. It logs in again
  when the session has been idle for a while or WebUntis reports it expired, and logs out on
  shutdown.
- **Caching:** master data (teachers, rooms, classes, timegrid, school years) is cached for
  1 hour and week timetables for 5 minutes. This is what keeps `find_free_rooms` cheap after
  its first scan.
- **Restricted accounts:** if the account can't read the teacher or room list (common for
  students), the server carries on without it. Rooms it can't read are listed under
  `unknownRooms`.
- **Logs:** logs go to stderr. Set `UNTIS_MCP_LOG=untis_mcp=debug` for more detail.

## Develop

```sh
cargo test
cargo clippy --all-targets
cargo run -- --demo
```

## License

MIT
