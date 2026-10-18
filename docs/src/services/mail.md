# Mail

Endpoint `/mail` (or `mail_<tool>` under [`/mcp`](combined.md)). The mail service reads and organises your iCloud mail over IMAP and saves drafts; it sends mail over SMTP only when you allow it.

## Tools

| Tool | Kind | What it does | Key parameters |
|---|---|---|---|
| `list_mailboxes` | read | Mailboxes with their role (Inbox, Sent, Drafts, Trash, Junk, Archive), total and unread counts | none |
| `search_messages` | read | Search one mailbox, newest first; every filter is optional and they combine with AND. Returns summaries, the mailbox `uidvalidity` and `next_cursor` | `mailbox` (default `INBOX`), `from`, `to`, `subject`, `text`, `since`, `before` (`YYYY-MM-DD`), `unseen`, `flagged`, `limit` (1 to 100, default 25), `cursor` |
| `get_message` | read | Headers, body and attachment list of one message. Does not mark it as read | `mailbox` (default `INBOX`), `uid`, `format` (`text`, `html` or `raw`), `max_chars` (default 20000) |
| `get_attachment` | read | One attachment as an embedded base64 resource, up to `MAX_ATTACHMENT_BYTES` | `mailbox` (default `INBOX`), `uid`, `index` (from `get_message`) |
| `get_thread` | read | The conversation a message belongs to, linked by Message-ID, References and In-Reply-To, from its mailbox and Sent, ordered by date | `mailbox` (default `INBOX`), `uid` |
| `mark_messages` | write | Mark as read or unread, flagged or unflagged | `mailbox` (default `INBOX`), `uids` (1 to 100), `seen`, `flagged`, `uidvalidity` |
| `move_messages` | write | Move messages to another mailbox | `mailbox` (default `INBOX`), `uids`, `target_mailbox`, `uidvalidity` |
| `delete_messages` | write | Move messages to Trash; with `permanent: true`, messages already in Trash are removed for good | `mailbox` (default `INBOX`), `uids`, `permanent`, `uidvalidity` |
| `create_mailbox` | write | Create a mailbox (folder) | `name` |
| `rename_mailbox` | write | Rename a mailbox; its child mailboxes move with it. Refuses INBOX and system mailboxes (Sent, Drafts, Trash, Junk, Archive), and a new name that already exists or is a system mailbox name | `name`, `new_name` |
| `delete_mailbox` | write | Delete an empty mailbox for good. Refuses INBOX, system mailboxes, and mailboxes that hold messages or child mailboxes | `name` |
| `create_draft` | write | Save a new draft in Drafts, optionally with attachments. Nothing is sent | `to`, `cc`, `bcc` (`name@example.com` or `Name <name@example.com>`), `subject`, `body_text`, `body_html`, `attachments` (each `filename`, `content_type`, `content_base64`; at most 10, decoded total up to `MAX_ATTACHMENT_BYTES`) |
| `create_reply_draft` | write | Save a threaded reply draft with the original quoted, optionally with attachments. Nothing is sent | `mailbox` (default `INBOX`), `uid`, `body_text`, `reply_all`, `attachments` (each `filename`, `content_type`, `content_base64`; at most 10, decoded total up to `MAX_ATTACHMENT_BYTES`) |
| `create_forward_draft` | write | Save a forward draft, optionally with the original attachments. Nothing is sent | `mailbox` (default `INBOX`), `uid`, `to`, `body_text`, `include_attachments` (default `true`) |
| `send_draft` | write | Send a draft over iCloud SMTP, save a copy to Sent and remove the draft. Registered only with `ICLOUD_ALLOW_SEND=true` | `uid` (in Drafts), `uidvalidity` |

## Prompts

Prompts are ready-made requests a client can offer you. They tell the model which tools to call and in what order; they change nothing by themselves.

| Prompt | Arguments | What it does |
|---|---|---|
| `triage_inbox` | `mailbox` (default `INBOX`), `limit` (1 to 100, default 25) | Summarises the unread messages and suggests replying, flagging, moving or deleting each one. Acts only on what you approve, writes reply drafts only when you ask, and never sends |

Arguments are text; a blank argument takes its default, and an invalid one (a `limit` outside 1 to 100) is refused. If a step needs a tool this server does not offer (a read-only service), the model is told to say so and skip it.

## Addressing messages

Messages are addressed by mailbox plus UID. Listings also return the mailbox `uidvalidity`; pass it back to the changing tools. If the mailbox was rebuilt in the meantime (its `uidvalidity` changed), the tool refuses instead of touching a different message that now has the same UID.

## Sending

- Without `ICLOUD_ALLOW_SEND=true`, `send_draft` is not registered: clients do not see it, and the other tools can only save drafts.
- `send_draft` only sends messages flagged as drafts in the Drafts mailbox, and sends one message at a time.
- `ICLOUD_FROM_NAME` sets the display name on drafts; the address is always `ICLOUD_EMAIL`.
- `create_draft` and `create_reply_draft` take `attachments` as `{filename, content_type, content_base64}` objects: `filename` 1 to 255 characters without `/`, `\` or control characters, `content_type` a plain `type/subtype` such as `application/pdf`, `content_base64` standard base64. If any attachment is invalid or the decoded total is over `MAX_ATTACHMENT_BYTES`, nothing is saved. The result lists each attachment's `filename`, `content_type` and `size`.

## iCloud notes

- When the server supports IMAP `MOVE` it is used; otherwise moves use COPY, then flag and expunge only the moved UIDs (UIDPLUS), so other deleted messages in the mailbox are never expunged.
- Message bodies are cached in memory (`CACHE_MAX_BYTES`), so reading the same message twice does not download it again.
- Idle IMAP connections are closed after `IMAP_IDLE_TIMEOUT` seconds and reopened on demand.
