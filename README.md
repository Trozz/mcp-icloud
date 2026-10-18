# mcp-icloud

A self-hosted [Model Context Protocol](https://modelcontextprotocol.io) server for iCloud. MCP clients such as claude.ai, Claude Desktop and Claude Code can work with your iCloud mail, calendars and contacts through it, using an app-specific password that never leaves your server.

**Documentation: [trozz.github.io/mcp-icloud](https://trozz.github.io/mcp-icloud/)**

| Endpoint | What it does |
|---|---|
| `/mail` | Search, read, organise and draft email; optionally send |
| `/calendar` | List, search, create, edit and delete events, including recurring ones; find free time; optionally manage invitations |
| `/contacts` | Search, read, create, edit and delete contacts; groups (iOS Lists); upcoming birthdays and anniversaries; find and merge duplicates |
| `/mcp` | Every enabled service's tools in one connector, named `<service>_<tool>` |

- A single static Rust binary, also shipped as a `FROM scratch` container image.
- TLS end to end, and a built-in OAuth 2.1 server so each endpoint works as a claude.ai custom connector. No database.
- Safe by default: sending mail and changing invitations are off until you enable them, any service can be made read-only, and every write is audited without recording content.

## Quick start

1. Create an app-specific password at [account.apple.com](https://account.apple.com) under **Sign-In and Security**, **App-Specific Passwords**.
2. Run the server:

   ```bash
   docker run -d --name mcp-icloud -p 8443:8443 \
     -e ICLOUD_EMAIL=you@icloud.com \
     -e ICLOUD_APP_PASSWORD=xxxx-xxxx-xxxx-xxxx \
     -e PUBLIC_URL=https://icloud.example.com \
     -e OAUTH_SIGNING_KEY="$(openssl rand -hex 32)" \
     -e OAUTH_ADMIN_PASSWORD=choose-a-password-of-12-or-more-chars \
     ghcr.io/trozz/mcp-icloud:latest
   ```

3. Make it reachable over HTTPS with a trusted certificate (for example a Cloudflare tunnel), then add a custom connector in claude.ai for `https://icloud.example.com/mcp` or a single service such as `https://icloud.example.com/contacts`.

The documentation covers [getting started](https://trozz.github.io/mcp-icloud/getting-started.html), [connecting clients](https://trozz.github.io/mcp-icloud/connecting-clients.html), [every configuration variable](https://trozz.github.io/mcp-icloud/configuration.html), [TLS](https://trozz.github.io/mcp-icloud/tls.html), each service's tools, [security](https://trozz.github.io/mcp-icloud/security.html) and [operations](https://trozz.github.io/mcp-icloud/operations.html).

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo deny check
bash scripts/test-next-version.sh
bash scripts/build-docs.sh
```

See [Contributing](https://trozz.github.io/mcp-icloud/contributing.html) for live tests, the documentation tools and adding a service, and [Releases](https://trozz.github.io/mcp-icloud/releases.html) for how versions are published.

## License

MIT
