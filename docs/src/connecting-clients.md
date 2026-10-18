# Connecting clients

Each service is a separate MCP server at its own path, and `/mcp` offers all of them together. Add one connector per endpoint you want, or a single connector for `/mcp`.

| Endpoint | Connector URL |
|---|---|
| Mail | `https://icloud.example.com/mail` |
| Calendar | `https://icloud.example.com/calendar` |
| Contacts | `https://icloud.example.com/contacts` |
| All services | `https://icloud.example.com/mcp` |

Use either the per-service connectors or the combined one, not both: with both, every tool appears twice.

## claude.ai, Claude Desktop and Claude mobile

These connect from Anthropic's servers, so the server must be reachable from the internet with a publicly trusted certificate (see [TLS and public access](tls.md)).

1. In claude.ai open **Settings**, then **Connectors**, then **Add custom connector**.
2. Enter the connector URL, for example `https://icloud.example.com/contacts`.
3. A login page from your server opens and names the service. Enter `OAUTH_ADMIN_PASSWORD`.

The connector then signs in with OAuth on its own. Access tokens last one hour and refresh tokens 30 days; the client refreshes its access token itself. Every sign-in ends 90 days after it was made, or when `OAUTH_ADMIN_PASSWORD` changes; the connector then asks you to connect again. Connectors added in claude.ai are also available in Claude Desktop and the Claude mobile apps.

## Claude Code

Claude Code can send a header, so it can use the static token. Set `MCP_AUTH_TOKEN` on the server, then:

```bash
claude mcp add --transport http icloud https://icloud.example.com/mcp \
  --header "Authorization: Bearer $MCP_AUTH_TOKEN"
```

or one server per service:

```bash
claude mcp add --transport http icloud-mail https://icloud.example.com/mail \
  --header "Authorization: Bearer $MCP_AUTH_TOKEN"
claude mcp add --transport http icloud-calendar https://icloud.example.com/calendar \
  --header "Authorization: Bearer $MCP_AUTH_TOKEN"
claude mcp add --transport http icloud-contacts https://icloud.example.com/contacts \
  --header "Authorization: Bearer $MCP_AUTH_TOKEN"
```

Claude Code also works against a local self-signed certificate if you trust it (see [TLS and public access](tls.md)).

## Other MCP clients

Any client that speaks the MCP Streamable HTTP transport works. It either completes the OAuth flow (dynamic client registration, authorization code with PKCE) or sends `Authorization: Bearer <MCP_AUTH_TOKEN>`. A request without a token gets a `401` whose `WWW-Authenticate` header points to `/.well-known/oauth-protected-resource/<endpoint>`, where discovery starts.

## Icons

Each endpoint sends its own icon in `serverInfo`, and `/favicon.ico` serves the project icon. Some clients, including claude.ai at the time of writing, show a generic icon for custom connectors regardless.
