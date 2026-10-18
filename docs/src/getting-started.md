# Getting started

This page takes you from nothing to a running server. You need an Apple account with two-factor authentication, and a machine that can run Docker or a single binary.

## 1. Create an app-specific password

1. Sign in at [account.apple.com](https://account.apple.com).
2. Open **Sign-In and Security**, then **App-Specific Passwords**.
3. Create one named `mcp-icloud` and copy it. It looks like `xxxx-xxxx-xxxx-xxxx`.

The same password is used for mail, calendar and contacts. You can revoke it at any time on the same page, which immediately cuts the server off from your account.

## 2. Choose secrets for the server

The server needs two secrets of its own:

- `OAUTH_SIGNING_KEY`: at least 32 bytes, used to sign OAuth tokens. Generate one with `openssl rand -hex 32` and keep it stable: changing it signs every client out.
- `OAUTH_ADMIN_PASSWORD`: at least 12 characters (15 or more avoids a startup warning). You type it on the server's login page when you add a connector in claude.ai.

Optionally also `MCP_AUTH_TOKEN` (for example `openssl rand -hex 32`), a static bearer token for clients that can send a header, such as Claude Code.

Store these in a secret manager or an environment file readable only by you. Every secret can also be read from a file: append `_FILE` to the variable name, for example `ICLOUD_APP_PASSWORD_FILE=/run/secrets/icloud`.

## 3. Run it

### With Docker

```bash
docker run -d --name mcp-icloud -p 8443:8443 \
  -e ICLOUD_EMAIL=you@icloud.com \
  -e ICLOUD_APP_PASSWORD=xxxx-xxxx-xxxx-xxxx \
  -e PUBLIC_URL=https://icloud.example.com \
  -e OAUTH_SIGNING_KEY="$(openssl rand -hex 32)" \
  -e OAUTH_ADMIN_PASSWORD=choose-a-password-of-12-or-more-chars \
  -e CALENDAR_TIMEZONE=Europe/London \
  -e CONTACTS_TIMEZONE=Europe/London \
  ghcr.io/trozz/mcp-icloud:latest
```

`PUBLIC_URL` is the address clients use to reach the server. See [TLS and public access](tls.md) for how to make it reachable with a trusted certificate.

### With Docker Compose and a Cloudflare tunnel

A tunnel gives the server a public HTTPS address with a trusted certificate, without opening a port on your network. Create a tunnel in the Cloudflare dashboard, add a public hostname such as `icloud.example.com` pointing to `https://mcp-icloud:8443` with **No TLS Verify** turned on (the server's own certificate is self-signed in this setup), and copy the tunnel token.

`compose.yaml`:

```yaml
services:
  mcp-icloud:
    image: ghcr.io/trozz/mcp-icloud:latest
    container_name: mcp-icloud
    restart: unless-stopped
    env_file: app.env
    ports:
      - "127.0.0.1:8443:8443"
    volumes:
      - ./audit:/audit
  tunnel:
    image: cloudflare/cloudflared:latest
    restart: unless-stopped
    command: tunnel --no-autoupdate run
    environment:
      TUNNEL_TOKEN: ${TUNNEL_TOKEN}
    depends_on:
      - mcp-icloud
```

`app.env` (readable only by you, for example `chmod 600 app.env`):

```bash
ICLOUD_EMAIL=you@icloud.com
ICLOUD_APP_PASSWORD=xxxx-xxxx-xxxx-xxxx
PUBLIC_URL=https://icloud.example.com
OAUTH_SIGNING_KEY=replace-with-the-output-of-openssl-rand-hex-32
OAUTH_ADMIN_PASSWORD=choose-a-password-of-12-or-more-chars
MCP_AUTH_TOKEN=replace-with-another-random-value
CALENDAR_TIMEZONE=Europe/London
CONTACTS_TIMEZONE=Europe/London
AUDIT_LOG_PATH=/audit/audit.jsonl
```

The server runs as UID 65532, so the audit directory must be writable by it: `mkdir -p audit && sudo chown 65532:65532 audit`. Then start both containers with `TUNNEL_TOKEN=<token> docker compose up -d`.

### With the binary

Download the archive for your platform from [Releases](https://github.com/Trozz/mcp-icloud/releases) (Linux x86_64 and arm64, macOS arm64), verify the checksum, extract it and run:

```bash
shasum -a 256 -c mcp-icloud-<version>-<target>.tar.gz.sha256
tar -xzf mcp-icloud-<version>-<target>.tar.gz
cd mcp-icloud-<version>-<target>
ICLOUD_EMAIL=you@icloud.com ICLOUD_APP_PASSWORD=xxxx-xxxx-xxxx-xxxx \
  PUBLIC_URL=https://icloud.example.com OAUTH_SIGNING_KEY=... OAUTH_ADMIN_PASSWORD=... \
  ./mcp-icloud
```

All configuration comes from environment variables; the [configuration reference](configuration.md) lists every one.

## 4. Check it is running

```bash
curl -k https://127.0.0.1:8443/healthz
```

prints `ok`. The startup log has one `service enabled` line per service with its URL and tools, and the contacts service logs `contacts loaded` with the number of cards once it has read your address book.

`mcp-icloud --version` prints the version, and `mcp-icloud --healthcheck` exits 0 when the local server answers `/healthz` (the container image uses it as its `HEALTHCHECK`).

## 5. Choose what runs

- `ICLOUD_SERVICES=mail,calendar` runs only some services (default: all). A service that is not enabled is not mounted and does not read its configuration.
- `ICLOUD_READ_ONLY=contacts` (or `all`) hides and refuses the write tools of those services.
- `ICLOUD_ALLOW_SEND=true` enables sending mail, and `ICLOUD_ALLOW_INVITES=true` enables changing attendees and replying to invitations. Both are off by default.

Next: [Connecting clients](connecting-clients.md).
