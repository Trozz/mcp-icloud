# Combined endpoint

Endpoint `/mcp`. It offers every enabled service's tools in one MCP server, so one connector gives a client mail, calendar and contacts together. The per-service endpoints keep working unchanged.

## Tool names

Under `/mcp` every tool is prefixed with its service: `<service>_<tool>`.

| Service | Example names |
|---|---|
| Mail | `mail_search_messages`, `mail_create_draft` |
| Calendar | `calendar_list_events`, `calendar_create_event` |
| Contacts | `contacts_search_contacts`, `contacts_upcoming_dates` |

Descriptions, parameters and annotations are the same as on the service's own endpoint, so the [Mail](mail.md), [Calendar](calendar.md) and [Contacts](contacts.md) pages describe every tool. Only services selected by `ICLOUD_SERVICES` appear.

## Safety switches apply

- A read-only service (`ICLOUD_READ_ONLY`) is read-only under `/mcp` too: its write tools are hidden and refused.
- `ICLOUD_ALLOW_SEND` and `ICLOUD_ALLOW_INVITES` decide whether `mail_send_draft` and `calendar_respond_to_invitation` exist, as on the service endpoints.
- Audit lines name the real service and tool, with `"endpoint": "/mcp"`.

## Sign-in

`/mcp` is its own OAuth protected resource (`https://icloud.example.com/mcp`, scope `mcp`), with metadata at `/.well-known/oauth-protected-resource/mcp`. A token issued for `/mcp` works only there, and a token for a service endpoint does not work on `/mcp`. Because `/mcp` is a resource of its own, an OAuth client must name the resource it wants (the `resource` parameter, which claude.ai sends), even when only one service runs. The static `MCP_AUTH_TOKEN` works on every endpoint.

## Turning it off

`/mcp` is served whenever at least one service runs. Set `ICLOUD_COMBINED=false` to turn it off; `/mcp` then returns 404.

## Choosing between them

- One connector for `/mcp` is simplest, and lets the client combine services in one conversation.
- Separate connectors let you enable services one by one in the client, and keep tool names short.
- Do not add both: every tool would appear twice.
