# Architecture

## Request flow

```mermaid
flowchart LR
    C1["MCP client<br/>/mail"] --> T
    C2["MCP client<br/>/calendar"] --> T
    C3["MCP client<br/>/contacts"] --> T
    C4["MCP client<br/>/mcp"] --> T
    T["HTTPS<br/>tunnel, proxy or direct"] --> S

    subgraph S["mcp-icloud binary (:8443)"]
        direction TB
        P["Public routes<br/>/healthz, /favicon.ico<br/>/.well-known/*, /register<br/>/authorize, /token"]
        G["Bearer check per endpoint<br/>token audience or static token"]
        W["Policy wrapper<br/>read-only and audit"]
        M["Mail service"]
        K["Calendar service"]
        A["Contacts service<br/>in-memory index"]
        X["Combined /mcp<br/>prefixed tool names"]
        D["mcp-icloud-dav<br/>shared WebDAV client"]
        G --> W
        W --> M
        W --> K
        W --> A
        W --> X
        X --> M
        X --> K
        X --> A
        K --> D
        A --> D
    end

    M -->|"IMAP / SMTP"| IC1["imap.mail.me.com<br/>smtp.mail.me.com"]
    D -->|"CalDAV"| IC2["caldav.icloud.com<br/>partition host"]
    D -->|"CardDAV"| IC3["contacts.icloud.com<br/>partition host"]
```

## Crates

| Crate | Role |
|---|---|
| `mcp-server-core` | Configuration reading, the OAuth authorization server, bearer guards, TLS, logging, metrics, the read-only and audit wrapper, and the `Service` trait every service implements |
| `mcp-icloud-dav` | The shared WebDAV client (scoped credentials, redirects, retries), multistatus parsing, home discovery, and the lossless content-line layer used for iCalendar and vCard |
| `mcp-icloud-mail` | The mail service: IMAP client and pool, message parsing, draft building, SMTP sending |
| `mcp-icloud-calendar` | The calendar service: discovery, recurrence expansion, event editing, invitations, time zones |
| `mcp-icloud-contacts` | The contacts service: address book discovery, the synced in-memory index, vCard reading and lossless patching, search, dates and duplicates |
| `mcp-icloud` | The binary: selects services, mounts each at `/<name>` and the combined `/mcp`, and runs the server |

The services do not depend on each other. Anything that combines them lives in the server core or the binary.

## A contact edit, end to end

```mermaid
sequenceDiagram
    participant C as Client
    participant S as mcp-icloud /contacts
    participant I as iCloud CardDAV
    C->>S: tools/call update_contact
    S->>I: REPORT sync-collection (last token)
    I-->>S: changed hrefs and etags, new token
    S->>I: REPORT addressbook-multiget (changed cards only)
    S->>S: patch only the touched lines
    S->>I: PUT card (If-Match: etag)
    I-->>S: 204 and a new ETag (stored byte for byte)
    S-->>C: the contact as stored
```

## Design choices

- **No database.** iCloud is the only store. Caches (message bodies, the calendar list, the contacts index) live in memory and are rebuilt after a restart.
- **Lossless edits.** Calendar and contact edits rewrite only the properties they change and keep every other line, including Apple's own folding, byte for byte.
- **Conditional writes.** Updates send `If-Match` so a change made elsewhere is never overwritten silently. CardDAV deletes are the exception, because iCloud ignores `If-Match` on them; see [Contacts](services/contacts.md#icloud-notes).
- **Safe defaults.** Sending mail and changing invitations need explicit switches, and any service can be made read-only.
