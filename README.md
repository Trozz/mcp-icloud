# mcp-icloud

A self-hosted [Model Context Protocol](https://modelcontextprotocol.io) server for iCloud. One server exposes each iCloud service as its own MCP endpoint, so MCP clients such as claude.ai, Claude Desktop and Claude Code can work with your mail, calendars and contacts.

| Service | Path | What it does |
|---|---|---|
| Mail | `/mail` | Search, read, organise and draft email; optionally send |
| Calendar | `/calendar` | List, search, create, edit and delete events, including recurring ones; find free time; optionally manage invitations |
| Contacts | `/contacts` | Search, read, create, edit and delete contacts; manage groups (iOS Lists) |

- Single static Rust binary, shipped as a `FROM scratch` container image.
- TLS end to end: the server terminates TLS itself, and every connection to Apple is verified.
- Built-in OAuth 2.1 authorization server so each service works as a claude.ai remote connector. No database, no volume.
- Safe by default: sending mail and changing invitations are off until you enable them.

## How it works

iCloud has no public API for these services. The server talks to Apple over standard protocols using an app-specific password:

| Purpose | Protocol | Host |
|---|---|---|
| Reading and organising mail | IMAP | `imap.mail.me.com:993` |
| Sending mail | SMTP, STARTTLS | `smtp.mail.me.com:587` |
| Calendars | CalDAV | `caldav.icloud.com` (then your account's partition host, for example `p99-caldav.icloud.com`) |
| Contacts | CardDAV | `contacts.icloud.com` (then your account's partition host, for example `p99-contacts.icloud.com`) |

MCP clients reach each service over HTTPS at its path (`https://<host>/mail`, `https://<host>/calendar`, `https://<host>/contacts`). They authenticate with an OAuth access token (claude.ai) or a static bearer token (Claude Code and other clients that can send a header). Your Apple password never leaves the server.

`ICLOUD_SERVICES` chooses which services run (default: all). A service that is not enabled is not mounted and does not read its configuration.

## Mail tools (`/mail`)

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

## Calendar tools (`/calendar`)

| Tool | What it does |
|---|---|
| `list_calendars` | Your event calendars: `id`, `name`, `color`, `writable` |
| `list_events` | Occurrences in a range (default today plus 7 days, at most 366 days), recurring events expanded; filter by calendars, text (`query`) or invitations awaiting your reply (`needs_response`). Stops at 500 occurrences with `truncated: true` |
| `get_event` | Full detail: description, recurrence in words and as RRULE, exceptions, attendees and organizer, alerts, URL |
| `find_free_time` | Free slots of at least `duration_minutes` in a range, optionally within `working_hours`; ignores events marked free and invitations you declined |
| `create_event` | Timed (`start`/`end`) or all-day (`start_date`/`end_date`, end date inclusive) events with location, description, alerts and an optional recurrence (`daily`, `weekly`, `monthly`, `yearly` with interval, days, until or count) |
| `update_event` | Change only the fields you pass, for the whole `series`, one `occurrence`, or `this_and_following` |
| `delete_event` | Delete the `series`, one `occurrence`, or `this_and_following` |
| `get_timezone`, `set_timezone` | Show or change the time zone used to read and show times |
| `respond_to_invitation` | Accept, decline or tentatively accept an invitation (only when `ICLOUD_ALLOW_INVITES=true`) |

Events are addressed by `event_id` (as returned by `list_events`); one occurrence of a recurring event additionally by `occurrence_start`, exactly as `list_events` returns it. Every change is a conditional write: if the event changed in iCloud since it was read, the tool returns a `conflict` error instead of overwriting it.

### Time zones

- Times are shown in the active zone, plus the event's own zone when it differs. Times you pass without an offset are read in the active zone; times with an offset are used as given. All-day events are dates and never shift.
- The active zone is `CALENDAR_TIMEZONE` (default `Europe/London`) unless overridden with `set_timezone`, for example when you tell the assistant you are travelling. The override lives in memory: a restart returns to `CALENDAR_TIMEZONE`.
- Every time-based tool also takes a `timezone` argument for one call.
- A local time that does not exist (skipped by a daylight saving change) is rejected; an ambiguous one (repeated hour) uses the earlier time.

### Invitations

With `ICLOUD_ALLOW_INVITES=false` (default) attendees are always shown and preserved, but the server will not change them: attendee arguments are refused, `respond_to_invitation` is not registered (clients will not see it), and events you organize that have attendees, and invitations from others (apart from your own alerts), cannot be changed or deleted, because iCloud would email the attendees. With `ICLOUD_ALLOW_INVITES=true`, adding or removing attendees makes iCloud send invitations or cancellations, and `respond_to_invitation` sends your reply to the organizer.

Reminders are not available: Apple no longer exposes upgraded iCloud Reminders over CalDAV.

## Contacts tools (`/contacts`)

| Tool | What it does |
|---|---|
| `search_contacts` | Find contacts by name, nickname, organization, department, job title, email or related name (every word must match; case and accents are ignored), or by a phone number in any format (`07700 900123` finds `+44 7700 900123`). Without a query it lists everyone; `group_id` limits it to one group. Paged with `limit` (default 25, at most 100) and `offset` |
| `get_contact` | Everything about one contact: names, organization, emails, phones, addresses, URLs, birthday, dates, related names, social profiles, instant messaging, note and groups |
| `get_my_contact` | Your own card (My Card) |
| `list_groups` | Groups (iOS Lists) with their member counts |
| `create_contact`, `update_contact` | Create a contact, or change only the fields you pass (`""` clears one). A list you pass (`emails`, `phones`, `urls`, `addresses`, `dates`, `related`) replaces that whole list; entries you pass back unchanged keep their exact lines |
| `delete_contact` | Delete a contact and remove it from its groups. Your own card cannot be deleted |
| `create_group`, `update_group`, `delete_group` | Create, rename, add or remove members of, or delete a group. Deleting a group keeps its contacts |

Contacts and groups are addressed by `id`, as `search_contacts` and `list_groups` return it. Labels are plain words: `home`, `work`, `other`, `mobile`, `iPhone`, `main`, `pager`, `home fax`, `work fax`, `home page`, `anniversary`, relations such as `mother` or `friend`, or any custom text. Birthdays and dates are `YYYY-MM-DD`, or `--MM-DD` without a year.

Edits are lossless: only the lines a change touches are rewritten, so photos, Apple's own fields and anything the server does not show survive every edit. Updates are conditional writes: if a contact changed in iCloud since it was read, the tool returns a `conflict` error instead of overwriting it.

iCloud does not enforce `If-Match` on contact deletes, so deletes cannot be made conditional: the server re-reads the address book at the start of every call and deletes immediately after, so only a change made on another device in those milliseconds could be lost.

## Quick start

### 1. Create an app-specific password

1. Sign in at [account.apple.com](https://account.apple.com).
2. Open **Sign-In and Security**, then **App-Specific Passwords**.
3. Create one named `mcp-icloud` and copy it (format `xxxx-xxxx-xxxx-xxxx`).

Two-factor authentication must be enabled on the Apple account. The same password is used for mail, calendar and contacts.

### 2. Run with Docker

```bash
docker run -d --name mcp-icloud -p 8443:8443 \
  -e ICLOUD_EMAIL=you@icloud.com \
  -e ICLOUD_APP_PASSWORD=xxxx-xxxx-xxxx-xxxx \
  -e PUBLIC_URL=https://icloud.example.com \
  -e OAUTH_SIGNING_KEY="$(openssl rand -hex 32)" \
  -e OAUTH_ADMIN_PASSWORD=choose-a-password-of-12-or-more-chars \
  -e CALENDAR_TIMEZONE=Europe/London \
  -v /path/to/certs:/certs:ro \
  -e TLS_CERT_PATH=/certs/fullchain.pem \
  -e TLS_KEY_PATH=/certs/privkey.pem \
  ghcr.io/trozz/mcp-icloud:latest
```

Add `-e ICLOUD_SERVICES=mail` (or `calendar`, `contacts`, or a comma separated list) to run only some services. Keep `OAUTH_SIGNING_KEY` stable: changing it signs everyone out. Store it in a secret, not in shell history.

### 3. Or run the binary

Download the archive for your platform from [Releases](https://github.com/Trozz/mcp-icloud/releases), verify the checksum, extract it and run:

```bash
shasum -a 256 -c mcp-icloud-<version>-<target>.tar.gz.sha256
tar -xzf mcp-icloud-<version>-<target>.tar.gz
cd mcp-icloud-<version>-<target>
ICLOUD_EMAIL=... ICLOUD_APP_PASSWORD=... PUBLIC_URL=... OAUTH_SIGNING_KEY=... OAUTH_ADMIN_PASSWORD=... ./mcp-icloud
```

`./mcp-icloud --version` prints the version. `./mcp-icloud --healthcheck` connects to `127.0.0.1` and exits 0 when the local server answers `/healthz` (used by the image `HEALTHCHECK`). `BIND_ADDR` must therefore listen on loopback; the default `0.0.0.0` does.

## Connecting clients

Each service is a separate MCP server, so add one connector per service you want.

### claude.ai, Claude Desktop and Claude mobile (remote connector)

1. In claude.ai open **Settings**, then **Connectors**, then **Add custom connector**.
2. Enter `https://<your PUBLIC_URL host>/mail` (or `/calendar`, `/contacts`).
3. A login page from this server opens, naming the service. Enter `OAUTH_ADMIN_PASSWORD`.

claude.ai connects from Anthropic's servers, so the server must be reachable from the internet with a **publicly trusted** certificate (directly through `TLS_CERT_PATH`/`TLS_KEY_PATH`, or at a reverse proxy or tunnel in front of it).

### Claude Code

Set `MCP_AUTH_TOKEN` on the server (for example `openssl rand -hex 32`), then:

```bash
claude mcp add --transport http icloud-mail https://icloud.example.com/mail \
  --header "Authorization: Bearer $MCP_AUTH_TOKEN"
claude mcp add --transport http icloud-calendar https://icloud.example.com/calendar \
  --header "Authorization: Bearer $MCP_AUTH_TOKEN"
claude mcp add --transport http icloud-contacts https://icloud.example.com/contacts \
  --header "Authorization: Bearer $MCP_AUTH_TOKEN"
```

## Configuration

Every secret can also be read from a file by appending `_FILE` to its name, for example `ICLOUD_APP_PASSWORD_FILE=/run/secrets/icloud`. A directly set value wins over the file.

### Server

| Variable | Required | Default | Description |
|---|---|---|---|
| `ICLOUD_SERVICES` | no | all (`mail,calendar,contacts`) | Comma separated services to run. An unknown name or an empty list stops startup |
| `PUBLIC_URL` | yes | | External base URL, for example `https://icloud.example.com`. `http` is accepted only for `localhost` |
| `OAUTH_SIGNING_KEY` | yes | | At least 32 bytes. Rotating it revokes every client and token |
| `OAUTH_ADMIN_PASSWORD` | yes | | Password for the OAuth login page, at least 12 characters |
| `MCP_AUTH_TOKEN` | no | | Static bearer token accepted on every service path |
| `TRUSTED_PROXIES` | no | | Comma separated proxy IPs whose `X-Forwarded-For` is trusted for login rate limiting |
| `ALLOWED_ORIGINS` | no | | Extra allowed `Origin` values, comma separated |
| `BIND_ADDR` | no | `0.0.0.0:8443` | Listen address. Must listen on loopback (`0.0.0.0` does) so `--healthcheck` can connect to `127.0.0.1` |
| `TLS_CERT_PATH`, `TLS_KEY_PATH` | no | | PEM certificate chain and key. Reloaded automatically when the files change |
| `INSECURE_HTTP` | no | `false` | Serve plain HTTP without TLS |
| `LOG_LEVEL` | no | `info` | Log filter, for example `info,mcp_icloud_mail::imap=debug` or `info,mcp_icloud_calendar=debug` |
| `LOG_FORMAT` | no | `json` | `json` or `pretty` |
| `DEBUG` | no | off | Debug logging with content redacted. Never logs IMAP protocol lines or credentials |
| `DEBUG_UNSAFE_LOG_CONTENT` | no | off | Also logs full tool results and CalDAV and CardDAV bodies, including message, event and contact content. Credentials are never logged |
| `METRICS_ADDR` | no | | Enables a Prometheus listener, for example `0.0.0.0:9090` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | no | | Enables OpenTelemetry trace export over OTLP/HTTP |

### Shared by mail, calendar and contacts

| Variable | Required | Default | Description |
|---|---|---|---|
| `ICLOUD_EMAIL` | yes | | Your Apple ID / iCloud address |
| `ICLOUD_APP_PASSWORD` | yes | | App-specific password |

### Mail

| Variable | Required | Default | Description |
|---|---|---|---|
| `ICLOUD_USERNAME` | no | `ICLOUD_EMAIL` | IMAP and SMTP login, if different |
| `ICLOUD_FROM_NAME` | no | | Display name on drafts |
| `ICLOUD_ALLOW_SEND` | no | `false` | Enables the `send_draft` tool |
| `MAX_ATTACHMENT_BYTES` | no | `10485760` | Largest attachment `get_attachment` returns |
| `CACHE_MAX_BYTES` | no | `67108864` | In-memory cache of message bodies. `0` disables it |
| `IMAP_IDLE_TIMEOUT` | no | `600` | Seconds before an idle IMAP connection is dropped |

### Calendar

| Variable | Required | Default | Description |
|---|---|---|---|
| `CALENDAR_TIMEZONE` | no | `Europe/London` | IANA time zone used when no override is set. An invalid name stops startup |
| `ICLOUD_ALLOW_INVITES` | no | `false` | Enables attendee changes and `respond_to_invitation` |
| `CALDAV_URL` | no | `https://caldav.icloud.com` | CalDAV entry point. Must be `https`, except a loopback address for testing |

### Contacts

| Variable | Required | Default | Description |
|---|---|---|---|
| `CARDDAV_URL` | no | `https://contacts.icloud.com` | CardDAV entry point. Must be `https`, except a loopback address for testing |

## TLS

The server picks its certificate at startup:

1. `INSECURE_HTTP=true`: plain HTTP. Use only when TLS is terminated elsewhere on a network you trust.
2. `TLS_CERT_PATH` and `TLS_KEY_PATH` both set: load them. If either file is missing or invalid the server refuses to start; it never falls back silently. Setting only one of them is an error.
3. Neither set: an in-memory self-signed certificate is generated for the `PUBLIC_URL` host, `localhost`, `127.0.0.1` and `::1`. It is never written to disk and changes on every restart. The log shows its SHA-256 fingerprint.

The self-signed certificate is fine for local use, Claude Code, or encrypting the hop from a reverse proxy or tunnel that skips verification or pins the fingerprint. **It does not work for claude.ai directly**, which requires a publicly trusted certificate.

### A stable local certificate with mkcert

```bash
brew install mkcert
mkcert -install
mkcert -cert-file cert.pem -key-file key.pem localhost 127.0.0.1 ::1
TLS_CERT_PATH=$PWD/cert.pem TLS_KEY_PATH=$PWD/key.pem ./mcp-icloud
```

### A self-signed certificate with openssl

```bash
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 825 \
  -keyout key.pem -out cert.pem -subj "/CN=icloud.example.com" \
  -addext "subjectAltName=DNS:icloud.example.com,DNS:localhost,IP:127.0.0.1"
```

Add `cert.pem` to the trust store of the machine running the client (or to `NODE_EXTRA_CA_CERTS` for Node based clients).

## Observability

- Logs go to stdout as JSON by default. Every tool call, OAuth request and IMAP, SMTP, CalDAV and CardDAV request is a span with name, outcome and duration.
- `DEBUG=1` adds tool arguments, result sizes, IMAP/SMTP command labels and CalDAV and CardDAV method, path and status. IMAP protocol lines, message, event and contact bodies, passwords, tokens and the `Authorization` header are never logged; contacts tools log contact ids only, never names, numbers or addresses.
- `DEBUG_UNSAFE_LOG_CONTENT=1` **writes your email, calendar and contact content into the logs**. The server prints a warning at startup. Use it only briefly while debugging.
- `METRICS_ADDR` starts a separate Prometheus listener (plain HTTP, keep it on an internal network): `mcp_tool_calls_total`, `mcp_tool_duration_seconds`, `imap_commands_total`, `imap_command_duration_seconds`, `imap_reconnects_total`, `smtp_sends_total`, `caldav_requests_total`, `caldav_request_duration_seconds`, `carddav_requests_total`, `carddav_request_duration_seconds`, `oauth_requests_total`, `oauth_login_failures_total`, `message_cache_hits_total`, `message_cache_misses_total`, `http_requests_total`.
- `OTEL_EXPORTER_OTLP_ENDPOINT` (with the standard `OTEL_EXPORTER_OTLP_HEADERS`) exports traces over OTLP/HTTP. Nothing is exported unless it is set.

## Security notes

- Each service is its own OAuth protected resource (RFC 9728), with metadata at `/.well-known/oauth-protected-resource/<service>`. Access tokens are bound to the service they were issued for (RFC 8707): a token issued for `/mail` is rejected on `/calendar`. One login page and one `OAUTH_ADMIN_PASSWORD` cover all services.
- `MCP_AUTH_TOKEN` is a single-owner shortcut accepted on every service path. Treat it like the admin password.
- The OAuth login is limited to 5 failed attempts per minute per client, plus 30 failures per minute globally. Trade-off: an attacker who keeps failing can lock the legitimate owner out of login for up to a minute, and the global ceiling can do so regardless of client address.
- Client address is the TCP peer. Behind a reverse proxy, set `TRUSTED_PROXIES` to the proxy IPs so the right-most untrusted `X-Forwarded-For` entry is used; otherwise every request shares the proxy IP and the per-client limit is effectively global.
- Access tokens last 1 hour, refresh tokens 30 days. There is no per-client revocation: rotate `OAUTH_SIGNING_KEY` to sign every client out.
- Calendar and contacts requests send your credentials only to `https` hosts under `icloud.com` (or the exact `CALDAV_URL` or `CARDDAV_URL` host when overridden); redirects elsewhere are refused.
- The container runs as UID 65532 with nothing else in the image.

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo deny check
bash scripts/test-next-version.sh
```

Live smoke tests against your real account are ignored by default:

```bash
ICLOUD_EMAIL=... ICLOUD_APP_PASSWORD=... cargo test -p mcp-icloud-mail --test live_icloud -- --ignored --nocapture
ICLOUD_EMAIL=... ICLOUD_APP_PASSWORD=... cargo test -p mcp-icloud-calendar --test live_icloud -- --ignored --nocapture --test-threads=1
ICLOUD_EMAIL=... ICLOUD_APP_PASSWORD=... cargo test -p mcp-icloud-contacts --test live_icloud -- --ignored --nocapture --test-threads=1
```

The calendar round trip creates one clearly named event in December 2030 and always deletes it. The contacts round trip creates a contact `ZZ MCP Probe` and a group `ZZ MCP Probe Group` with made-up data and always deletes both. The invitation test runs only when `ICLOUD_TEST_ATTENDEE` names an address you control.

### Adding a service

Each service is a library crate that implements the `Service` trait from `mcp-server-core::service`:

```rust
pub trait Service: Send + Sync + 'static {
    fn name(&self) -> &'static str;            // path, OAuth resource and scope, e.g. "contacts"
    fn tool_names(&self) -> Vec<String>;
    fn log_caps(&self) -> &'static [&'static str] { &[] }
    fn router(self: Arc<Self>, ctx: &ServiceContext) -> axum::Router;
}
```

1. Create `crates/mcp-icloud-<name>` as a library. Read its own variables (plus `ICLOUD_EMAIL`/`ICLOUD_APP_PASSWORD`) in `pub fn from_env(env: &mut ServiceEnv) -> Option<Arc<dyn Service>>`, reporting problems through `env.errors`. Never read the server variables.
2. Build the MCP endpoint in `router()` with `mcp_server_core::service::mcp_service(ctx, move || Ok(MyServer::new(...)))`.
3. Add the crate to `crates/mcp-icloud/Cargo.toml` and one line to `SERVICES` in `crates/mcp-icloud/src/lib.rs`.

Routing, OAuth metadata, token audiences, `ICLOUD_SERVICES` and the release pipeline pick the new service up without further changes.

## Releases

- Work happens on `feature/*` and `bugfix/*` branches merged into `develop` by pull request.
- Every merge into `develop` that touches the server publishes a **pre-release** `vX.Y.Z-dev.N` (GitHub pre-release, image tags `X.Y.Z-dev.N` and `develop`).
- A pull request from `develop` into `main` publishes a **release** `vX.Y.Z` when merged (image tags `X.Y.Z`, `X.Y`, `X`, `latest`).
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

After the first image is published, set the `mcp-icloud` package visibility to public under **Packages** on GitHub if you want anonymous pulls.

## Migrating from mcp-icloud-mail

This project was called `mcp-icloud-mail` and served only mail at `/mcp`.

- The repository is now `Trozz/mcp-icloud` (old URLs redirect), the image `ghcr.io/trozz/mcp-icloud`, the binary `mcp-icloud`, and release tags `vX.Y.Z` (the old `mail-v*` tags remain as history).
- Mail moved from `/mcp` to `/mail`. Remove the old connector and add `https://<host>/mail`; `/mcp` now returns 404.
- OAuth clients and tokens issued by the old server are no longer valid, so each connector signs in once more. `OAUTH_SIGNING_KEY`, `OAUTH_ADMIN_PASSWORD` and `MCP_AUTH_TOKEN` keep working unchanged.
- Calendar and contacts run by default; set `ICLOUD_SERVICES=mail` to keep the previous behaviour.

## License

MIT
