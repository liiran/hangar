# Security

hangar starts Claude Code sessions, and those sessions can run commands and edit files on your computer. Anything that can use hangar can therefore do the same. So the goal is simple: **only your own hangar window, on your own computer, can use it.**

## Reporting a problem

Please report security problems privately, not in a public issue: on this repository's **Security** tab, choose **Report a vulnerability**. Say what you did, what happened and what you expected.

## What protects you

**Nothing is reachable from outside your computer.**
- Every port hangar opens is bound to `127.0.0.1`. Nothing listens on your network address.
- hangar makes no network requests of its own: no telemetry, no update checks. Claude Code and the MCP servers you configure make theirs, as they would without hangar.

**Only your window can use the app.**
- `hangar up` opens the app with a one-time code (128 random bits, single use, valid for 2 minutes). The page trades it for a token that lives until hangar stops.
- The token is sent as a request header, never in a URL or a cookie, so another website cannot make your browser send it.
- Every request is refused unless it is addressed to `127.0.0.1:<port>` exactly, which stops DNS-rebinding attacks.
- Requests your browser marks as coming from another site, or from another port on your computer, are refused. Another site cannot call the API, open the live connection, load a script or image from hangar, or frame it. It can only link to the page.
- The page runs under a strict Content-Security-Policy: scripts and styles only from hangar itself, and no connections, images, frames or forms to anywhere else.
- Answers from the API are never cached by the browser.

**What Claude writes cannot act on its own.**
- Replies are rendered as text and Markdown, never as HTML. Scripts, frames, forms and styles in a reply are shown as text.
- Images in replies are not loaded. They become links.
- Only `http` and `https` links are links. Each opens in a new tab that cannot reach the app, and shows the site it really goes to when its text names a different one.
- Desktop notifications carry the session's name and what happened, not Claude's words.

**What you approve is what runs.**
- A permission request shows the exact command, file contents or diff, and every other field of the request.
- A long request is never hidden behind a scroll bar: Allow stays off until you have opened all of it.
- Invisible and text-direction characters are written out and marked (for example `⟨U+202E⟩`), so a command cannot read differently from what it does.
- "Allow for session" spells out the rule it adds. It can only add "allow" rules for that session, never change the permission mode or write to settings files.
- A new request ignores clicks for a moment, so a click meant for the previous one cannot land on it.
- A request ends with the process that made it, and cannot be approved afterwards.

**A folder cannot run things before you agree.**
- The first session in a folder shows what the folder's own Claude settings would do: hooks, pre-approved tools, MCP servers with their full command lines, and every other setting.
- Your agreement covers what was shown. If the folder's settings changed after you read the notice, hangar asks again.
- Settings files are read with a size limit, and only if they are ordinary files.

**Processes.**
- hangar never builds a shell command from text. Programs are started with separate arguments, and the values hangar passes to `claude` are either from a fixed list (permission modes) or checked (model names, conversation ids).
- The shim and the daemon prove to each other that they know a secret in `~/.hangar/daemon.json` before the shim sends anything. A program that cannot read that file cannot start anything through hangar, and a fake daemon cannot collect a shim's environment.
- On Windows, everything hangar starts lives in a job object that ends with hangar, even if hangar crashes.

**What is stored, and where.** Everything is under `~/.hangar`, readable only by you (on Unix by file mode; on Windows through your profile folder's permissions):

| File | Holds |
|---|---|
| `daemon.json` | the daemon's secret for this run |
| `sessions/<id>/events.jsonl` | each session's history: your messages, Claude's replies, tool calls |
| `trusted.json` | the folders you trusted |
| `logs/daemon.log` | start-up and server events. No messages, tokens or environment values |
| `backups/`, `wrapped.json` | copies of `~/.claude.json` made by `hangar wrap`, which include any keys in your MCP settings |

## What it does not protect you from

- **Other programs running as you.** They can read `~/.hangar/daemon.json`, as they can read your SSH keys. hangar is not a barrier between programs of the same user.
- **A command you approve.** It runs as you, with your files and your keys. Permission modes decide how much Claude may do without asking; "Full access" never asks. Text that Claude reads (a web page, a file in a repository) can try to steer it, which is why approvals show exactly what would run.
- **Computers shared with people you do not trust.** hangar is built for a computer that is yours.
  - Its address (`127.0.0.1:7317`) stays the same between runs so the app can be bookmarked. While hangar is not running, another user could listen there and leave something in your browser that is still there when hangar next opens.
  - Another user can also slow hangar down by flooding its ports.
  - Keep `HANGAR_HOME`, if you set it, in a folder only you can read.
- **One shared Playwright browser, if you turn it on.** `share = "native"` makes Playwright listen on a local port that has no password, because Playwright offers none. Any program on the computer, including another user's, could drive that browser and run code as you. It is off by default: each session gets its own Playwright. Turn it on only on a computer that only you use.
- **Mixed-up accounts, in rare cases.** Sessions never share a server when their environments differ in a variable that looks like a credential or names an account, server or data location (`*_TOKEN`, `AWS_PROFILE`, `DATABASE_URL`, ...). That goes by the variable's name, so it is a strong safeguard, not a guarantee. `share = "session"` for a server is the sure way.
- **A shared server's error text.** The last line a shared server printed can appear in an error shown to every session using it.
- **Disk use and old history.** Session histories are stored as plain text and are not trimmed. Delete a session to delete its history.
- **Linux and macOS.** They compile and pass static checks but have never been run. There, processes hangar started can outlive a crash of hangar.

## How this is checked

- `cargo test --workspace`: the token, Host, Origin and cross-site checks, pairing, the handshake (including impostor daemons), routing between sessions, folder trust, hostile paths and names.
- `npm run e2e`: the app in a real browser: hostile replies, links, long and disguised permission requests, sign-in and sign-out.
- Dependencies are checked against the public advisory databases before a release.
