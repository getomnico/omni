# Gmail Skill

Source: adapted from `googleworkspace/cli` `skills/gws-gmail/SKILL.md`
at https://github.com/googleworkspace/cli. Omni uses Google connector MCP tools
instead of direct local `gws` commands.

## Tool Use
Use available connector tools for Gmail, usually named with the `gmail__`
prefix. Do not run local `gws` commands from the sandbox for mailbox access;
Omni owns authentication and routes requests through connector tools with source
permissions.

## Search Before Reading
Use Gmail search/list tools before reading full messages or threads. Gmail query
syntax is useful for narrowing:
- `from:`, `to:`, `cc:`, `subject:`
- `has:attachment`, `filename:pdf`
- `newer_than:`, `older_than:`, `after:`, `before:`
- `label:`, `in:inbox`, `is:unread`, `is:starred`

Inspect candidate sender, subject, date, labels, message ID, and thread ID
before reading full content.

## Messages And Threads
Use thread tools when the user asks about a conversation, reply chain, or
context around an email. Use message tools when the user asks for a specific
email, header, attachment, or single-message action.

Prefer stable message and thread IDs from tool results over subject text.
Subjects are not unique and may change across replies.

## Attachments
Before downloading or saving attachments, list message parts and confirm file
name, MIME type, and size. If the user only needs a summary, inspect or preview
the attachment content before fetching large binary data.

## Labels And Mailbox State
Treat label changes, read/unread changes, archive, trash, draft, and send
operations as write actions. Confirm the target messages/threads and intended
state change before executing.

For bulk updates, search first and show a compact count/sample of matching
messages. Avoid broad mailbox mutations from ambiguous queries.

## Drafts And Sending
Prefer draft creation/update over immediate send when composing mail. Confirm
recipients, subject, body, attachments, and thread context before sending.

Use reply/reply-all/forward tools when available so Gmail threading headers and
recipient behavior are handled by the connector tool rather than reconstructed
manually.
