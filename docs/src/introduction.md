# mcp-icloud

mcp-icloud is a self-hosted [Model Context Protocol](https://modelcontextprotocol.io) (MCP) server for iCloud. It lets MCP clients such as claude.ai, Claude Desktop and Claude Code work with your iCloud mail, calendars and contacts, using an app-specific password that never leaves your server.

One server runs every service. Each service is its own MCP endpoint, and a combined endpoint offers all of them in one connector:

| Endpoint | Service | What it does |
|---|---|---|
| `/mail` | Mail | Search, read, organise and draft email; optionally send |
| `/calendar` | Calendar | List, search, create, edit and delete events, including recurring ones; find free time; optionally manage invitations |
| `/contacts` | Contacts | Search, read, create, edit and delete contacts; groups (iOS Lists); upcoming birthdays and anniversaries; find and merge duplicates |
| `/mcp` | Combined | Every enabled service's tools in one connector, named `<service>_<tool>` |

## Highlights

- A single static Rust binary, also shipped as a `FROM scratch` container image (`ghcr.io/trozz/mcp-icloud`).
- TLS end to end: the server terminates TLS itself, and every connection to Apple is verified.
- A built-in OAuth 2.1 authorization server, so each endpoint works as a claude.ai custom connector. No database and no volume are needed.
- Safe by default: sending mail and changing invitations are off until you enable them, and any service can be made read-only.
- Every write is recorded in an audit log that never contains your content.

## How it talks to Apple

iCloud has no public API for these services. The server uses the standard protocols Apple supports for third-party apps:

| Purpose | Protocol | Host |
|---|---|---|
| Reading and organising mail | IMAP | `imap.mail.me.com:993` |
| Sending mail | SMTP with STARTTLS | `smtp.mail.me.com:587` |
| Calendars | CalDAV | `caldav.icloud.com`, then your account's partition host such as `p99-caldav.icloud.com` |
| Contacts | CardDAV | `contacts.icloud.com`, then your account's partition host such as `p99-contacts.icloud.com` |

## What it cannot do

Some iCloud data has no supported protocol, so the server does not offer it:

- **Reminders**: Apple no longer exposes upgraded iCloud Reminders over CalDAV.
- **Notes, iCloud Drive, Photos and Hide My Email**: only reachable through Apple's private web APIs with two-factor sign-in, which this project does not use.

## Privacy

Your Apple password stays on your server and is only sent to Apple. Logs record tool names, outcomes, durations and ids, never message, event or contact content, unless you explicitly turn on unsafe content logging while debugging.
