# mcp-icloud-mail

A self-hosted [Model Context Protocol](https://modelcontextprotocol.io) server for iCloud Mail. It lets MCP clients such as claude.ai, Claude Desktop and Claude Code search, read, organise and draft email in one iCloud account, and optionally send it.

- Single static Rust binary, shipped as a `FROM scratch` container image.
- TLS end to end: the server terminates TLS itself, and every connection to Apple is verified.
- Built-in OAuth 2.1 authorization server so it works as a claude.ai remote connector. No database, no volume.
- Sending is off by default. All composing creates drafts in your iCloud Drafts folder.

## How it works

iCloud Mail has no public API. The server talks to Apple over standard protocols using an app-specific password:

| Purpose | Host |
|---|---|
| Reading and organising (IMAP) | `imap.mail.me.com:993` |
| Sending (SMTP, STARTTLS) | `smtp.mail.me.com:587` |

MCP clients reach the server over HTTPS at `/mcp`. They authenticate with an OAuth access token (claude.ai) or a static bearer token (Claude Code and other clients that can send a header). Your Apple password never leaves the server.

## Tools

| Tool | What it does |
|---|---|
| `list_mailboxes` | Folders with role (Inbox, Sent, Drafts, Trash, Junk, Archive), total and unread counts |
| `search_messages` | Search a folder by sender, recipient, subject, text, date range, unread, flagged; paginated |
| `get_message` | Headers, body (text, HTML or raw) and attachment list |
| `get_attachment` | One attachment as an embedded resource (size capped) |
| `get_thread` | Related messages across the folder and Sent |
| `mark_messages` | Set or clear read and flagged |
| `move_messages` | Move to another folder |
| `delete_messages` | Move to Trash; permanent delete only from Trash |
| `create_mailbox` | Create a folder |
| `create_draft`, `create_reply_draft`, `create_forward_draft` | Save a draft to iCloud Drafts |
| `send_draft` | Send a draft (only when `ICLOUD_ALLOW_SEND=true`). Only messages flagged `\Draft` are sent, and sends are serialized |

Messages are addressed by folder plus UID. Results include the folder `uidvalidity`; pass it back to changing tools so a stale UID can never touch the wrong message.

## Quick start

### 1. Create an app-specific password

1. Sign in at [account.apple.com](https://account.apple.com).
2. Open **Sign-In and Security**, then **App-Specific Passwords**.
3. Create one named `mcp-icloud-mail` and copy it (format `xxxx-xxxx-xxxx-xxxx`).

Two-factor authentication must be enabled on the Apple account.

### 2. Run with Docker

```bash
docker run -d --name mcp-icloud-mail -p 8443:8443 \
  -e ICLOUD_EMAIL=you@icloud.com \
  -e ICLOUD_APP_PASSWORD=xxxx-xxxx-xxxx-xxxx \
  -e PUBLIC_URL=https://mail.example.com \
  -e OAUTH_SIGNING_KEY="$(openssl rand -hex 32)" \
  -e OAUTH_ADMIN_PASSWORD=choose-a-password-of-12-or-more-chars \
  -v /path/to/certs:/certs:ro \
  -e TLS_CERT_PATH=/certs/fullchain.pem \
  -e TLS_KEY_PATH=/certs/privkey.pem \
  ghcr.io/trozz/mcp-icloud-mail:latest
```

Keep `OAUTH_SIGNING_KEY` stable: changing it signs everyone out. Store it in a secret, not in shell history.

### 3. Or run the binary

Download the archive for your platform from [Releases](https://github.com/Trozz/mcp-icloud-mail/releases), verify the checksum, extract it and run:

```bash
shasum -a 256 -c mcp-icloud-mail-<version>-<target>.tar.gz.sha256
tar -xzf mcp-icloud-mail-<version>-<target>.tar.gz
cd mcp-icloud-mail-<version>-<target>
ICLOUD_EMAIL=... ICLOUD_APP_PASSWORD=... PUBLIC_URL=... OAUTH_SIGNING_KEY=... OAUTH_ADMIN_PASSWORD=... ./mcp-icloud-mail
```

`./mcp-icloud-mail --version` prints the version. `./mcp-icloud-mail --healthcheck` connects to `127.0.0.1` and exits 0 when the local server answers `/healthz` (used by the image `HEALTHCHECK`). `BIND_ADDR` must therefore listen on loopback; the default `0.0.0.0` does.

## Connecting clients

### claude.ai, Claude Desktop and Claude mobile (remote connector)

1. In claude.ai open **Settings**, then **Connectors**, then **Add custom connector**.
2. Enter the URL `https://<your PUBLIC_URL host>/mcp`.
3. A login page from this server opens. Enter `OAUTH_ADMIN_PASSWORD`.

claude.ai connects from Anthropic's servers, so the server must be reachable from the internet with a **publicly trusted** certificate (directly through `TLS_CERT_PATH`/`TLS_KEY_PATH`, or at a reverse proxy in front of it).

### Claude Code

Set `MCP_AUTH_TOKEN` on the server (for example `openssl rand -hex 32`), then:

```bash
claude mcp add --transport http icloud-mail https://mail.example.com:8443/mcp \
  --header "Authorization: Bearer $MCP_AUTH_TOKEN"
```

## Configuration

Every secret can also be read from a file by appending `_FILE` to its name, for example `ICLOUD_APP_PASSWORD_FILE=/run/secrets/icloud`. A directly set value wins over the file.

| Variable | Required | Default | Description |
|---|---|---|---|
| `ICLOUD_EMAIL` | yes | | Your iCloud address. Used as From address and default username |
| `ICLOUD_USERNAME` | no | `ICLOUD_EMAIL` | IMAP and SMTP login, if different |
| `ICLOUD_APP_PASSWORD` | yes | | App-specific password |
| `ICLOUD_FROM_NAME` | no | | Display name on drafts |
| `ICLOUD_ALLOW_SEND` | no | `false` | Enables the `send_draft` tool |
| `PUBLIC_URL` | yes | | External base URL, for example `https://mail.example.com`. `http` is accepted only for `localhost` |
| `OAUTH_SIGNING_KEY` | yes | | At least 32 bytes. Rotating it revokes every client and token |
| `OAUTH_ADMIN_PASSWORD` | yes | | Password for the OAuth login page, at least 12 characters |
| `MCP_AUTH_TOKEN` | no | | Static bearer token accepted on `/mcp` |
| `TRUSTED_PROXIES` | no | | Comma separated proxy IPs whose `X-Forwarded-For` is trusted for login rate limiting |
| `ALLOWED_ORIGINS` | no | | Extra allowed `Origin` values, comma separated |
| `BIND_ADDR` | no | `0.0.0.0:8443` | Listen address. Must listen on loopback (`0.0.0.0` does) so `--healthcheck` can connect to `127.0.0.1` |
| `TLS_CERT_PATH`, `TLS_KEY_PATH` | no | | PEM certificate chain and key. Reloaded automatically when the files change |
| `INSECURE_HTTP` | no | `false` | Serve plain HTTP without TLS |
| `MAX_ATTACHMENT_BYTES` | no | `10485760` | Largest attachment `get_attachment` returns |
| `CACHE_MAX_BYTES` | no | `67108864` | In-memory cache of message bodies. `0` disables it |
| `IMAP_IDLE_TIMEOUT` | no | `600` | Seconds before an idle IMAP connection is dropped |
| `LOG_LEVEL` | no | `info` | Log filter, for example `info,mcp_icloud_mail::imap=debug` |
| `LOG_FORMAT` | no | `json` | `json` or `pretty` |
| `DEBUG` | no | off | Debug logging with message content redacted. Never logs IMAP protocol lines or credentials |
| `DEBUG_UNSAFE_LOG_CONTENT` | no | off | Also logs full tool results, including message content. Credentials are never logged |
| `METRICS_ADDR` | no | | Enables a Prometheus listener, for example `0.0.0.0:9090` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | no | | Enables OpenTelemetry trace export over OTLP/HTTP |

## TLS

The server picks its certificate at startup:

1. `INSECURE_HTTP=true`: plain HTTP. Use only when TLS is terminated elsewhere on a network you trust.
2. `TLS_CERT_PATH` and `TLS_KEY_PATH` both set: load them. If either file is missing or invalid the server refuses to start; it never falls back silently. Setting only one of them is an error.
3. Neither set: an in-memory self-signed certificate is generated for the `PUBLIC_URL` host, `localhost`, `127.0.0.1` and `::1`. It is never written to disk and changes on every restart. The log shows its SHA-256 fingerprint.

The self-signed certificate is fine for local use, Claude Code, or encrypting the hop from a reverse proxy that skips verification or pins the fingerprint. **It does not work for claude.ai**, which requires a publicly trusted certificate.

### A stable local certificate with mkcert

```bash
brew install mkcert
mkcert -install
mkcert -cert-file cert.pem -key-file key.pem localhost 127.0.0.1 ::1
TLS_CERT_PATH=$PWD/cert.pem TLS_KEY_PATH=$PWD/key.pem ./mcp-icloud-mail
```

### A self-signed certificate with openssl

```bash
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 825 \
  -keyout key.pem -out cert.pem -subj "/CN=mail.example.com" \
  -addext "subjectAltName=DNS:mail.example.com,DNS:localhost,IP:127.0.0.1"
```

Add `cert.pem` to the trust store of the machine running the client (or to `NODE_EXTRA_CA_CERTS` for Node based clients).

## Observability

- Logs go to stdout as JSON by default. Every tool call, OAuth request and IMAP/SMTP command is a span with name, outcome and duration.
- `DEBUG=1` adds tool arguments, result sizes and IMAP/SMTP command labels with outcomes. IMAP protocol lines, message bodies, passwords and tokens are never logged.
- `DEBUG_UNSAFE_LOG_CONTENT=1` **writes your email content into the logs**. The server prints a warning at startup. Use it only briefly while debugging.
- `METRICS_ADDR` starts a separate Prometheus listener (plain HTTP, keep it on an internal network): `mcp_tool_calls_total`, `mcp_tool_duration_seconds`, `imap_commands_total`, `imap_command_duration_seconds`, `imap_reconnects_total`, `smtp_sends_total`, `oauth_requests_total`, `oauth_login_failures_total`, `message_cache_hits_total`, `message_cache_misses_total`, `http_requests_total`.
- `OTEL_EXPORTER_OTLP_ENDPOINT` (with the standard `OTEL_EXPORTER_OTLP_HEADERS`) exports traces over OTLP/HTTP. Nothing is exported unless it is set.

## Security notes

- The OAuth login is protected by `OAUTH_ADMIN_PASSWORD` (at least 12 characters) and limited to 5 failed attempts per minute per client, plus 30 failures per minute globally. Trade-off: an attacker who keeps failing can lock the legitimate owner out of login for up to a minute, and the global ceiling can do so regardless of client address.
- Client address is the TCP peer. Behind a reverse proxy, set `TRUSTED_PROXIES` to the proxy IPs so the right-most untrusted `X-Forwarded-For` entry is used; otherwise every request shares the proxy IP and the per-client limit is effectively global.
- Access tokens last 1 hour, refresh tokens 30 days. There is no per-client revocation: rotate `OAUTH_SIGNING_KEY` to sign every client out.
- The container runs as UID 65532 with nothing else in the image.

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo deny check
bash scripts/test-next-version.sh
```

A live smoke test against your real account is ignored by default:

```bash
ICLOUD_EMAIL=... ICLOUD_APP_PASSWORD=... cargo test -p mcp-icloud-mail --test live_icloud -- --ignored --nocapture
```

## Releases

- Work happens on `feature/*` and `bugfix/*` branches merged into `develop` by pull request.
- Every merge into `develop` that touches the server publishes a **pre-release** `mail-vX.Y.Z-dev.N` (GitHub pre-release, image tags `X.Y.Z-dev.N` and `develop`).
- A pull request from `develop` into `main` publishes a **release** `mail-vX.Y.Z` when merged (image tags `X.Y.Z`, `X.Y`, `X`, `latest`).
- The version bump comes from the `develop` to `main` pull request: a `major`, `minor` or `patch` label, otherwise `[major]`, `[minor]` or `[patch]` in its title, otherwise patch. Pre-releases on `develop` follow the currently open release pull request.
- Only merge commits are allowed. Squash and rebase merging are disabled so `develop` and `main` never diverge.
- Pull requests into `main` must come from `develop` in this repository.

### Repository setup (once)

```bash
gh label create major --color B60205 --description "Release: major version bump"
gh label create minor --color FBCA04 --description "Release: minor version bump"
gh label create patch --color 0E8A16 --description "Release: patch version bump"
```

Under **Settings > Actions > General > Workflow permissions**, the workflows must be allowed to grant `contents: write`, `packages: write`, `id-token: write` and `attestations: write` (the jobs request them explicitly through `permissions` blocks).

After the first image is published, set the `mcp-icloud-mail` package visibility to public under **Packages** on GitHub if you want anonymous pulls.

## License

MIT
