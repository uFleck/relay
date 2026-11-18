# Slack setup (DM capture)

Relay reads your 1:1 DMs with a **user token** over **Socket Mode**. There is no public URL, no bot in the
conversation, and no message is sent to Slack. Two tokens are needed; both are stored locally (Windows
Credential Manager once the keyring step lands; a DPAPI-protected file before that) and never leave the
machine except to call `slack.com`.

An MCP server is **not** a substitute: an MCP tool is request/response, so it would have to poll with a
token anyway, and Slack's hosted MCP is OAuth-installed per workspace and does not push new DM events.

## 1. Create the app

1. Open <https://api.slack.com/apps> → **Create New App** → **From scratch**.
2. Name it `Relay`, pick the workspace, create.

## 2. User token (`xoxp-`)

**OAuth & Permissions** → **User Token Scopes** → add:

| Scope | Why |
| --- | --- |
| `im:read` | list 1:1 DM channels |
| `im:history` | read the messages in them |
| `users:read` | show peer names instead of ids |
| `mpim:read`, `mpim:history` | only if you enable group DMs (off by default) |

Then **Install to Workspace** (or **Reinstall**) and copy the **User OAuth Token** (`xoxp-…`).

> Some workspaces require an admin to approve the install. If the button is greyed out or the install is
> pending, ask the admin; Relay cannot work around that.

## 3. App-level token (`xapp-`)

**Basic Information** → **App-Level Tokens** → **Generate Token and Scopes**:

- Name: `relay-socket`
- Scope: `connections:write`

Copy the token (`xapp-…`). This is what opens the Socket Mode websocket.

## 4. Events

1. **Socket Mode** → **Enable Socket Mode** (uses the app-level token).
2. **Event Subscriptions** → **Enable Events**.
3. Under **Subscribe to events on behalf of users**, add:
   - `message.im` — new DMs and thread replies in DMs
   - `message.mpim` — only if group DMs are enabled
4. Save. Slack may ask to reinstall the app after changing scopes.

Relay subscribes to `message.im`; it ignores channels, bots and its own messages.

## 5. Paste into Relay

Settings → **Slack**:

- User token → `xoxp-…`
- App token → `xapp-…`
- **Connect**

Relay calls `auth.test` (proves the user token), `apps.connections.open` (proves the app token), then opens
the websocket. On the first connect it backfills 14 days of DMs, marks those episodes `historical`, and runs
JEV in **shadow** mode (answers stored, no task changes).

The status row shows the workspace, the user, the last event and the number of open episodes. **Pause
capture** stops writes immediately; **Disconnect** deletes the tokens.

## Checking on demand

The socket is optional. Press **Ctrl+Alt+S** anywhere, choose **Sync now** in Settings → Slack, or use the tray's
**Sync Slack now**. Relay lists your 1:1 DMs, pulls everything newer than the last checkpoint, and runs JEV — the
same work the socket does continuously. It reads only the user token, so a dead socket or app token does not block
it, and it reports a short summary (`Slack: 3 new messages in 2 conversations · 2 analyzed`).

The check does **not** bypass the app install: the user token still comes from an app installed in the workspace.
If your workspace requires approval, the shortcut cannot read its DMs either.

The first pull of a conversation is a 14-day shadow backfill (answers stored, no tasks); later pulls act normally.
The check and the socket's catch-up never overlap, and a check while capture is paused is refused.

## Troubleshooting

| Symptom | Meaning |
| --- | --- |
| `invalid_auth` | the token is wrong, revoked, or from another workspace |
| `not_allowed_token_type` | a bot token was pasted where the user token belongs (or the reverse) |
| `missing_scope` | a scope from the tables above is missing; reinstall after adding it |
| `not_in_channel` / empty DM list | the user has no 1:1 DMs, or the app was installed by another user |
| Socket connects but no events | `message.im` was not added under **events on behalf of users**, or the app was not reinstalled |
| Workspace blocks user-token apps | only the workspace admin can allow it; the fallback is the rate-limited catch-up poller (also needs the user token) |
| `Ctrl+Alt+S` does nothing | another app owns the key; use the tray's **Sync Slack now** or the Settings button |

## Privacy

- DMs are stored in the local SQLite database only (`%APPDATA%\dev.relay.app\relay.db`).
- Default retention is 30 days; task changes keep their excerpt and permalink after the message is pruned.
- Paused capture and excluded peers are enforced before any write.
- Message text leaves the machine only for JEV classification and the reviewer, and both see it as
  untrusted data.
