# Configuration reference

Everything is configured with environment variables. The server checks all of them at startup and refuses to start with a list of every problem it found, so a typo never leaves it half configured.

General rules:

- An empty value counts as unset.
- Every variable can also be read from a file by appending `_FILE` to its name, for example `ICLOUD_APP_PASSWORD_FILE=/run/secrets/icloud`. A value set directly wins over the file, and a trailing newline in the file is ignored. Use this for secrets with Docker or Kubernetes secrets.
- Booleans accept `true`, `false`, `1`, `0`, `yes`, `no`, `on` and `off`.
- Lists are comma separated; spaces around names are ignored.

## Server and HTTP

| Variable | Required | Default | Example | Description |
|---|---|---|---|---|
| `PUBLIC_URL` | yes | | `https://icloud.example.com` | External base URL clients use, without a path. Must be `https`; `http` is accepted only for `localhost` |
| `BIND_ADDR` | no | `0.0.0.0:8443` | `127.0.0.1:8443` | Listen address. It must listen on loopback (`0.0.0.0` does) so `--healthcheck` can reach `127.0.0.1` |
| `TLS_CERT_PATH`, `TLS_KEY_PATH` | no | | `/certs/fullchain.pem`, `/certs/privkey.pem` | PEM certificate chain and private key. Both or neither; reloaded automatically when the files change. See [TLS and public access](tls.md) |
| `INSECURE_HTTP` | no | `false` | `true` | Serve plain HTTP. Only when TLS is terminated in front of the server on a network you trust |
| `ALLOWED_ORIGINS` | no | | `https://app.example.com` | Extra `Origin` values accepted on MCP requests, comma separated. The `PUBLIC_URL` origin is always allowed |
| `TRUSTED_PROXIES` | no | | `10.0.0.2,10.0.0.3` | Proxy IP addresses whose `X-Forwarded-For` is trusted for login rate limiting |

## Services

| Variable | Required | Default | Example | Description |
|---|---|---|---|---|
| `ICLOUD_SERVICES` | no | all (`mail,calendar,contacts`) | `mail,calendar` | Services to run. An unknown name or an empty list stops startup. A service that is not selected is not mounted and does not read its variables |
| `ICLOUD_READ_ONLY` | no | none | `contacts` or `all` | Services whose write tools are hidden and refused, on their own endpoint and under `/mcp`. An unknown name stops startup. See [Security](security.md#read-only-services) |
| `ICLOUD_COMBINED` | no | `true` | `false` | Serve the [combined endpoint](services/combined.md) `/mcp`. With `false`, `/mcp` returns 404 |

## OAuth and tokens

| Variable | Required | Default | Example | Description |
|---|---|---|---|---|
| `OAUTH_SIGNING_KEY` | yes | | output of `openssl rand -hex 32` | Key that signs OAuth tokens, at least 32 bytes. Changing it signs every client out |
| `OAUTH_ADMIN_PASSWORD` | yes | | a passphrase | Password for the server's login page, at least 12 characters (the server warns below 15). Changing it signs every connector out |
| `MCP_AUTH_TOKEN` | no | | output of `openssl rand -hex 32` | Static bearer token accepted on every endpoint, for clients that can send a header. Treat it like the admin password. Must not contain exactly two dots |

## iCloud account

| Variable | Required | Default | Example | Description |
|---|---|---|---|---|
| `ICLOUD_EMAIL` | yes | | `you@icloud.com` | Your Apple ID or iCloud address |
| `ICLOUD_APP_PASSWORD` | yes | | `xxxx-xxxx-xxxx-xxxx` | App-specific password, used by every service |

## Mail

| Variable | Required | Default | Example | Description |
|---|---|---|---|---|
| `ICLOUD_USERNAME` | no | `ICLOUD_EMAIL` | `you` | IMAP and SMTP login, if it differs from the address |
| `ICLOUD_FROM_NAME` | no | | `Alex Example` | Display name on drafts |
| `ICLOUD_ALLOW_SEND` | no | `false` | `true` | Registers the `send_draft` tool. Without it nothing can be sent |
| `MAX_ATTACHMENT_BYTES` | no | `10485760` (10 MiB) | `20971520` | Largest attachment `get_attachment` returns, and the largest decoded total of the attachments on one `create_draft` or `create_reply_draft` |
| `CACHE_MAX_BYTES` | no | `67108864` (64 MiB) | `0` | In-memory cache of message bodies; `0` disables it |
| `IMAP_IDLE_TIMEOUT` | no | `600` | `300` | Seconds before an idle IMAP connection is closed |

## Calendar

| Variable | Required | Default | Example | Description |
|---|---|---|---|---|
| `CALENDAR_TIMEZONE` | no | `Europe/London` | `America/New_York` | IANA time zone used to read and show times when no override is set. An unknown name stops startup |
| `ICLOUD_ALLOW_INVITES` | no | `false` | `true` | Allows changing attendees (iCloud emails them) and registers `respond_to_invitation` |
| `CALDAV_URL` | no | `https://caldav.icloud.com` | `http://127.0.0.1:8080` | CalDAV entry point. Must be `https`, except a loopback address for testing |

## Contacts

| Variable | Required | Default | Example | Description |
|---|---|---|---|---|
| `CONTACTS_TIMEZONE` | no | `UTC` | `Europe/London` | IANA time zone that decides "today" for `upcoming_dates`. An unknown name stops startup |
| `CARDDAV_URL` | no | `https://contacts.icloud.com` | `http://127.0.0.1:8080` | CardDAV entry point. Must be `https`, except a loopback address for testing |

## Logging, metrics and audit

| Variable | Required | Default | Example | Description |
|---|---|---|---|---|
| `LOG_LEVEL` | no | `info` | `info,mcp_icloud_contacts=debug` | Log filter in `tracing` syntax. Targets: `mcp_icloud_mail`, `mcp_icloud_calendar`, `mcp_icloud_contacts`, `mcp_server_core`. The `audit` target always stays on at `info` |
| `LOG_FORMAT` | no | `json` | `pretty` | `json` or `pretty` |
| `DEBUG` | no | `false` | `true` | Debug logging with content redacted: tool arguments, result sizes, protocol command labels and status codes. Never IMAP protocol lines, bodies or credentials |
| `DEBUG_UNSAFE_LOG_CONTENT` | no | `false` | `true` | Also logs full tool results and CalDAV and CardDAV bodies, which **puts your mail, event and contact content in the logs**. A warning is printed at startup. Credentials are never logged |
| `METRICS_ADDR` | no | | `0.0.0.0:9090` | Starts a separate plain-HTTP Prometheus listener. Keep it on an internal network |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | no | | `https://otel.example.com` | Exports traces over OTLP/HTTP. The standard `OTEL_EXPORTER_OTLP_HEADERS` is honoured. Nothing is exported unless it is set |
| `AUDIT_LOG_PATH` | no | | `/audit/audit.jsonl` | File every write tool call is appended to as one JSON line (created with mode 0600). Audit lines are always logged too. See [Security](security.md#audit-log) |

## A complete example

```bash
ICLOUD_EMAIL=you@icloud.com
ICLOUD_APP_PASSWORD_FILE=/run/secrets/icloud_app_password
PUBLIC_URL=https://icloud.example.com
OAUTH_SIGNING_KEY_FILE=/run/secrets/oauth_signing_key
OAUTH_ADMIN_PASSWORD_FILE=/run/secrets/oauth_admin_password
MCP_AUTH_TOKEN_FILE=/run/secrets/mcp_auth_token
ICLOUD_SERVICES=mail,calendar,contacts
ICLOUD_READ_ONLY=mail
CALENDAR_TIMEZONE=Europe/London
CONTACTS_TIMEZONE=Europe/London
AUDIT_LOG_PATH=/audit/audit.jsonl
LOG_FORMAT=json
```
