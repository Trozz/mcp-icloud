# Operations

## Health

- `GET /healthz` returns `ok` without authentication.
- `mcp-icloud --healthcheck` connects to `127.0.0.1` on the `BIND_ADDR` port and exits 0 when `/healthz` answers. The container image uses it as its `HEALTHCHECK`, so `docker ps` shows `healthy`.

## Logs

- Logs go to stdout, as JSON by default (`LOG_FORMAT=pretty` for humans).
- Every tool call, OAuth request and IMAP, SMTP, CalDAV and CardDAV request is a span with its name, outcome and duration. Startup logs one `service enabled` line per service, with its URL and tools.
- `LOG_LEVEL` takes `tracing` filters, for example `info,mcp_icloud_contacts=debug`. Libraries that could log secrets are capped at `info` whatever you set.
- `DEBUG=true` adds tool arguments, result sizes, protocol command labels and status codes, without content.
- `DEBUG_UNSAFE_LOG_CONTENT=true` adds full content. Use it only briefly; see [Security](security.md#what-is-never-logged).

## Metrics

`METRICS_ADDR` (for example `0.0.0.0:9090`) starts a separate plain-HTTP Prometheus listener. Keep it on an internal network. Metrics:

| Metric | What it counts |
|---|---|
| `mcp_tool_calls_total`, `mcp_tool_duration_seconds` | Tool calls by tool and outcome, and their duration |
| `imap_commands_total`, `imap_command_duration_seconds`, `imap_reconnects_total` | IMAP commands and reconnects |
| `smtp_sends_total` | Sent messages |
| `caldav_requests_total`, `caldav_request_duration_seconds` | CalDAV requests by method and status class |
| `carddav_requests_total`, `carddav_request_duration_seconds` | CardDAV requests by method and status class |
| `oauth_requests_total`, `oauth_login_failures_total` | OAuth requests and failed logins |
| `message_cache_hits_total`, `message_cache_misses_total` | Message body cache |
| `http_requests_total` | HTTP requests by route and status |

## Traces

`OTEL_EXPORTER_OTLP_ENDPOINT`, with the standard `OTEL_EXPORTER_OTLP_HEADERS` when your collector needs them, exports traces over OTLP/HTTP. Nothing is exported unless it is set.

## Audit file

With `AUDIT_LOG_PATH`, the audit file only grows: one short line per write. Rotate it with a tool that truncates in place, such as logrotate with `copytruncate`. See [Security](security.md#audit-log) for the line format.

## Upgrading

- Docker: pull the new image and recreate the container (`docker compose pull && docker compose up -d`). The container is stateless.
- Binary: replace the file and restart.
- Connectors stay signed in across upgrades and restarts as long as `OAUTH_SIGNING_KEY` stays the same.
- Release notes list every change; see [Releases](releases.md).

## Backups

There is nothing to back up: iCloud holds all data, and the server keeps only caches in memory. Keep your configuration and secrets somewhere safe.

## Resource use

Memory holds the message body cache (`CACHE_MAX_BYTES`, default 64 MiB), the calendar list, and the contacts index (a few kilobytes per contact). The container needs no writable filesystem apart from the audit volume.
