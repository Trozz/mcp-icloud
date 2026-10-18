# Troubleshooting

## The server does not start

The server lists every configuration problem at startup and exits. Common ones:

| Message | Fix |
|---|---|
| `PUBLIC_URL is required` | Set `PUBLIC_URL` to the external `https` address, without a path |
| `OAUTH_SIGNING_KEY must be at least 32 bytes` | Use `openssl rand -hex 32` |
| `OAUTH_ADMIN_PASSWORD must be at least 12 characters, not only spaces` | Choose a longer password; with 15 or more characters the startup warning goes too |
| `MCP_AUTH_TOKEN must not have the shape of a JWT (three dot-separated parts)` | Use a value without exactly two dots; `openssl rand -hex 32` is fine |
| `ICLOUD_SERVICES contains an unknown service: ...` | Use names from `mail`, `calendar`, `contacts` |
| `CALENDAR_TIMEZONE "..." is not an IANA time zone name such as Europe/London` | Use a name from the IANA list, such as `America/New_York` |
| `..._FILE: cannot read ...` | The secret file is missing or unreadable by UID 65532 in the container |

## claude.ai cannot connect

- The server must be reachable from the internet with a publicly trusted certificate. A self-signed certificate works only behind a tunnel or proxy; see [TLS and public access](tls.md).
- With a Cloudflare tunnel, the public hostname must point to `https://mcp-icloud:8443` with **No TLS Verify** on.
- `PUBLIC_URL` must be exactly the address claude.ai uses. OAuth metadata and token audiences are built from it.

## A connector stops working with 401

- Every sign-in ends 90 days after it was made, and changing `OAUTH_ADMIN_PASSWORD` ends them all: connect the connector again.
- After upgrading from 1.1 or earlier, every connector signs in once more (see [Upgrading](operations.md#upgrading-from-11-or-earlier)).
- Rotating `OAUTH_SIGNING_KEY` signs every client out: remove and add the connector again.
- A token for one endpoint is not accepted on another. Make sure the connector URL is the endpoint it signed in to.

## "iCloud rejected the login"

The app-specific password was revoked or mistyped, or `ICLOUD_EMAIL` (`ICLOUD_USERNAME` for mail) is wrong. Create a new app-specific password and restart the server.

## Tool errors

Calendar and contacts tool errors start with a kind, for example `conflict: ...`:

| Kind | Meaning |
|---|---|
| `conflict` | The item changed in iCloud since it was read. Read it again and retry |
| `not_found` | The id or mailbox does not exist (any more) |
| `validation` | An argument is wrong; the message says which |
| `read_only` | The service is read-only on this server (`ICLOUD_READ_ONLY`), or the calendar or item cannot be written |
| `auth` | iCloud rejected the login |
| `quota` | The iCloud storage quota is full |
| `upstream` | iCloud returned an unexpected answer; usually temporary |
| `internal` | An unexpected error inside the server; check the log and report it if it repeats |

Mail tool errors have no kind. They are plain messages that say what went wrong, such as `mailbox not found: ...` or `UIDVALIDITY of ... changed ...; re-run search_messages to get fresh uids`. The `conflict` and `quota` kinds therefore do not appear for mail.

## Contacts look out of date

The contacts index syncs at the start of every call, so changes on your devices appear on the next call. If the startup log shows `contacts discovery failed; will retry on the first tool call`, check the app-specific password and network access to `contacts.icloud.com`.

## More detail

Set `LOG_LEVEL=info,mcp_icloud_mail=debug` (or `mcp_icloud_calendar`, `mcp_icloud_contacts`) or `DEBUG=true` and reproduce the problem. These do not log your content. When reporting an issue, include the version (`mcp-icloud --version`) and the log lines, and check that they contain nothing private.
