// Records the README's screenshots and demo video from the real app, with
// real Claude Code sessions.
//
//   cargo build --release && cd web && npm run media
//
// Three sessions (Sonnet and Haiku; a few cents of usage) build and fix small
// projects in a throwaway home folder, so the app shows no personal folders,
// and Claude gets only the two MCP servers set up here (Playwright and
// Context7, shared through hangar's pool). One of them starts in "Plan only":
// its plan is approved in the video. Waiting for Claude is sped up in the
// video; everything else plays at its real speed.
//
// E2E_PROFILE picks the build, as for the tests (default: release).
//
// HANGAR_MEDIA_DIR picks the throwaway folder. It must not exist yet, and it
// must be outside your home folder, because its path is in the pictures (the
// default is the temp folder, which on Windows is inside your home folder). Encoding needs ffmpeg: FFMPEG_PATH, or
// ffmpeg on PATH. Without it the frames are kept, and the script prints how to
// encode them later with `npm run media -- --encode <frames folder>`.
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(import.meta.dirname, "../..");
const IMG = path.join(root, "docs", "img");
const MEDIA = path.join(root, "docs", "media");
const SIZE = { width: 1920, height: 1080 };
const MAX_GIF_MB = 15;
// The README's pictures use the dark theme. MEDIA_THEME=light records the light one.
const THEME = process.env.MEDIA_THEME === "light" ? "light" : "dark";
const FFMPEG = process.env.FFMPEG_PATH || "ffmpeg";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ------------------------------------------------------------ recording

class Recorder {
  frames = [];
  cuts = [];
  stopped = false;
  /** While set, frames are dropped and the previous one stays on screen. */
  paused = false;

  constructor(dir) {
    this.dir = dir ?? fs.mkdtempSync(path.join(os.tmpdir(), "hangar-frames-"));
  }

  /** Records `page` from now on (a second page continues the same video). */
  async attach(page) {
    if (this.client) {
      await this.client.send("Page.stopScreencast").catch(() => {});
      await this.client.detach().catch(() => {});
    }
    const client = await page.context().newCDPSession(page);
    this.client = client;
    client.on("Page.screencastFrame", (e) => {
      client.send("Page.screencastFrameAck", { sessionId: e.sessionId }).catch(() => {});
      if (this.stopped || this.paused || this.client !== client) return;
      const file = path.join(this.dir, `${String(this.frames.length).padStart(6, "0")}.jpg`);
      fs.writeFileSync(file, Buffer.from(e.data, "base64"));
      this.frames.push({ time: Date.now(), file });
    });
    await client.send("Page.startScreencast", { format: "jpeg", quality: 92, maxWidth: SIZE.width, maxHeight: SIZE.height, everyNthFrame: 1 });
  }

  async stop() {
    this.stopped = true;
    await this.client?.send("Page.stopScreencast").catch(() => {});
  }

  /** Waiting for Claude between `start` and `end` plays in about 1.5 s. */
  cut(start, end) {
    this.cuts.push({ start, end });
  }

  mapped(time) {
    let out = time;
    for (const { start, end } of this.cuts) {
      const keep = 1500;
      const length = end - start;
      if (length <= keep) continue;
      if (time >= end) out -= length - keep;
      else if (time > start) out -= (time - start) * (1 - keep / length);
    }
    return out;
  }

  /** Writes ffmpeg's frame list: each frame shows until the next one. */
  writeList(tail = 2000) {
    const f = this.frames;
    let list = "";
    f.forEach((frame, i) => {
      const next = i + 1 < f.length ? this.mapped(f[i + 1].time) : this.mapped(frame.time) + tail;
      list += `file '${frame.file.replace(/\\/g, "/")}'\nduration ${Math.max(0.001, (next - this.mapped(frame.time)) / 1000).toFixed(4)}\n`;
    });
    list += `file '${f[f.length - 1].file.replace(/\\/g, "/")}'\n`;
    fs.writeFileSync(path.join(this.dir, "list.txt"), list);
  }
}

function hasFfmpeg() {
  return spawnSync(FFMPEG, ["-version"], { encoding: "utf8" }).status === 0;
}

/** demo.mp4 at full size, and demo.gif for the README (as wide as fits the size limit). */
function encode(dir) {
  const run = (args) => {
    const r = spawnSync(FFMPEG, ["-y", "-loglevel", "error", ...args], { encoding: "utf8" });
    if (r.status !== 0) throw new Error(`ffmpeg failed: ${r.stderr || r.error}`);
  };
  fs.mkdirSync(MEDIA, { recursive: true });
  const mp4 = path.join(MEDIA, "demo.mp4");
  const gif = path.join(MEDIA, "demo.gif");
  const list = path.join(dir, "list.txt");
  run(["-f", "concat", "-safe", "0", "-i", list, "-vf", "fps=30,scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p", "-c:v", "libx264", "-crf", "20", "-preset", "slow", "-movflags", "+faststart", mp4]);
  // From the video, whose frames repeat cleanly, rather than the raw frames.
  for (const width of [1920, 1600, 1280, 1024]) {
    run(["-i", mp4, "-vf", `fps=12,scale=${width}:-1:flags=lanczos,mpdecimate=hi=512:lo=256:frac=0.1,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle`, gif]);
    const mb = fs.statSync(gif).size / 1e6;
    console.log(`demo.gif at ${width} px: ${mb.toFixed(1)} MB`);
    if (mb <= MAX_GIF_MB) break;
  }
  if (fs.statSync(gif).size / 1e6 > MAX_GIF_MB) throw new Error(`demo.gif is over ${MAX_GIF_MB} MB even at 1024 px. Do not commit it.`);
  console.log(`demo.mp4: ${(fs.statSync(mp4).size / 1e6).toFixed(1)} MB`);
}

if (process.argv[2] === "--encode") {
  encode(path.resolve(process.argv[3]));
  process.exit(0);
}

// ------------------------------------------------------------ the demo setup

if (process.platform !== "win32") throw new Error("the recording script is written for Windows for now");
const exe = (name) => path.join(root, "target", process.env.E2E_PROFILE ?? "release", `${name}.exe`);
const [hangar, shim] = [exe("hangar"), exe("hangar-shim")];
for (const f of [hangar, shim]) if (!fs.existsSync(f)) throw new Error(`${f} is missing: run cargo build --release first`);

function findClaude() {
  for (const dir of (process.env.PATH ?? "").split(path.delimiter)) {
    const native = path.join(dir, "claude.exe");
    if (fs.existsSync(native)) return native;
    const npm = path.join(dir, "node_modules", "@anthropic-ai", "claude-code", "bin", "claude.exe");
    if (fs.existsSync(path.join(dir, "claude.cmd")) && fs.existsSync(npm)) return npm;
  }
  throw new Error("claude is not installed (npm i -g @anthropic-ai/claude-code)");
}

const work = process.env.HANGAR_MEDIA_DIR ?? fs.mkdtempSync(path.join(os.tmpdir(), "hangar-media-"));
if (process.env.HANGAR_MEDIA_DIR) {
  if (fs.existsSync(work)) throw new Error(`${work} already exists`);
  fs.mkdirSync(work, { recursive: true });
}
const home = path.join(work, "home"); // the app's home folder: it lists nothing else
// The folder's path is in the pictures, so one inside your home folder would show your user name.
if (path.resolve(work).toLowerCase().startsWith(os.homedir().toLowerCase() + path.sep)) {
  throw new Error("Set HANGAR_MEDIA_DIR to a folder outside your home folder, for example C:" + path.sep + "hangar-media, so that the pictures do not show your user name.");
}
const state = path.join(work, "state"); // HANGAR_HOME
const project = (name, files) => {
  const dir = path.join(home, "code", name);
  fs.mkdirSync(dir, { recursive: true });
  for (const [file, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
    fs.writeFileSync(path.join(dir, file), text);
  }
  return dir;
};

const recipe = project("recipe-site", { "README.md": "# Recipe site\n\nA one-page site for a favourite recipe.\n" });
const budget = project("budget-cli", {
  "budget.py": `"""Small budget helpers. Amounts are in cents."""


def total(expenses):
    """Sum of every amount."""
    return sum(e["cents"] for e in expenses)


def by_category(expenses):
    """Total per category, largest first."""
    sums = {}
    for e in expenses:
        sums[e["category"]] = e["cents"]
    return dict(sorted(sums.items(), key=lambda kv: kv[1], reverse=True))
`,
  "test_budget.py": `import unittest

from budget import by_category, total

EXPENSES = [
    {"category": "food", "cents": 1250},
    {"category": "rent", "cents": 90000},
    {"category": "food", "cents": 830},
    {"category": "transport", "cents": 300},
]


class BudgetTest(unittest.TestCase):
    def test_total(self):
        self.assertEqual(total(EXPENSES), 92380)

    def test_by_category_adds_up_each_category(self):
        self.assertEqual(by_category(EXPENSES), {"rent": 90000, "food": 2080, "transport": 300})


if __name__ == "__main__":
    unittest.main()
`,
});
const notes = project("react-notes", { "README.md": "# React notes\n\nShort notes and examples while learning React 19.\n" });

// Trusted ahead of time, as they would be after their first session.
fs.mkdirSync(state, { recursive: true });
fs.writeFileSync(path.join(state, "trusted.json"), JSON.stringify([recipe, budget, notes].map((d) => d.replaceAll("\\", "/").toLowerCase())));

// The demo turns on the one shared Playwright browser, which is off by
// default (it needs a local port without a password; see SECURITY.md).
fs.writeFileSync(path.join(state, "config.toml"), '[servers."@playwright/mcp"]' + "\n" + 'share = "native"' + "\n");

// Every session gets the same two local MCP servers, through hangar's pool.
const npx = (...args) => ["cmd", "/c", "npx", "-y", ...args];
const servers = { playwright: npx("@playwright/mcp@latest", "--headless", "--isolated"), context7: npx("@upstash/context7-mcp@latest") };
const mcpConfig = path.join(work, "mcp.json");
fs.writeFileSync(
  mcpConfig,
  JSON.stringify({ mcpServers: Object.fromEntries(Object.entries(servers).map(([name, cmd]) => [name, { command: shim, args: ["--name", name, "--", ...cmd] }])) }, null, 2),
);
// Claude Code saves a plan under the real home folder unless told otherwise, and the plan card
// shows where. Kept in the project, the path shown has no user name in it.
const settings = path.join(work, "settings.json");
fs.writeFileSync(settings, JSON.stringify({ plansDirectory: ".claude/plans" }));
// hangar starts this instead of claude: Claude keeps the real home (its
// sign-in lives there) and sees only the servers above.
const wrapper = path.join(work, "claude.cmd");
fs.writeFileSync(
  wrapper,
  `@echo off\r\nset "USERPROFILE=${os.homedir()}"\r\nset "HOME=${os.homedir()}"\r\n"${findClaude()}" %* --strict-mcp-config --mcp-config "${mcpConfig}" --settings "${settings}"\r\n`,
);

const env = { ...process.env, HANGAR_HOME: state, HANGAR_CLAUDE: wrapper, USERPROFILE: home, HOME: home };
const up = () => execFileSync(hangar, ["up", "--no-open"], { env, encoding: "utf8" }).trim().split(/\r?\n/).pop();

// ------------------------------------------------------------ helpers

const firstUrl = up();
const base = new URL(firstUrl).origin;
const { token } = await (
  await fetch(`${base}/api/pair`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ code: firstUrl.split("#pair=")[1] }) })
).json();
const api = async (method, p, body) =>
  (await fetch(`${base}/api${p}`, { method, headers: { "x-hangar-token": token, "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined })).json();
const session = async (dir) => (await api("GET", "/state")).sessions.filter((s) => s.cwd.toLowerCase() === dir.toLowerCase()).at(-1);

async function until(check, timeout = 300_000) {
  const end = Date.now() + timeout;
  for (;;) {
    const v = await check();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${check}`);
    await sleep(400);
  }
}

const browser = await chromium.launch({ channel: process.env.E2E_CHANNEL ?? "chrome", headless: true });
const context = await browser.newContext({ viewport: SIZE, colorScheme: THEME, deviceScaleFactor: 1 });
// A visible pointer and the keys being pressed, since a headless browser shows neither.
await context.addInitScript((dark) => {
  addEventListener("DOMContentLoaded", () => {
    const pointer = document.createElement("div");
    pointer.dataset.demoOverlay = "";
    pointer.style.cssText = "position:fixed;left:0;top:0;width:22px;height:26px;z-index:99999;pointer-events:none;will-change:transform;filter:drop-shadow(0 1px 2px rgba(0,0,0,.35))";
    pointer.innerHTML = '<svg width="22" height="26" viewBox="0 0 22 26"><path d="M2 2v19l5-4.6 3.4 7.6 3.6-1.6-3.4-7.4H17z" fill="#fff" stroke="#111" stroke-width="1.5" stroke-linejoin="round"/></svg>';
    pointer.style.transform = "translate(960px,540px)";
    document.documentElement.append(pointer);
    addEventListener("mousemove", (e) => (pointer.style.transform = `translate(${e.clientX}px,${e.clientY}px)`), true);
    // A click shows as a ring that spreads from the pointer.
    addEventListener(
      "mousedown",
      (e) => {
        const ring = document.createElement("div");
        ring.dataset.demoOverlay = "";
        ring.style.cssText = `position:fixed;left:${e.clientX - 18}px;top:${e.clientY - 18}px;width:36px;height:36px;border-radius:50%;z-index:99998;pointer-events:none;border:3px solid ${dark ? "rgba(255,255,255,.9)" : "rgba(24,24,27,.8)"};transform:scale(.3);opacity:1;transition:transform .45s ease-out,opacity .45s ease-out`;
        document.documentElement.append(ring);
        requestAnimationFrame(() => {
          ring.style.transform = "scale(1.5)";
          ring.style.opacity = "0";
        });
        setTimeout(() => ring.remove(), 520);
      },
      true,
    );
    const keys = document.createElement("div");
    keys.dataset.demoOverlay = "";
    keys.style.cssText =
      "position:fixed;left:50%;bottom:28px;transform:translateX(-50%);z-index:99999;pointer-events:none;padding:8px 14px;border-radius:10px;font:600 15px/1.2 'Inter Variable',system-ui,sans-serif;letter-spacing:.02em;opacity:0;transition:opacity .2s";
    // The label for a pressed key stands out from the page: light on a dark page, dark on a light one.
    keys.style.background = dark ? "rgba(244,244,245,.94)" : "rgba(24,24,27,.86)";
    keys.style.color = dark ? "#18181b" : "#fff";
    document.documentElement.append(keys);
    let hide;
    window.__showKeys = (text, ms = 1100) => {
      keys.textContent = text;
      keys.style.opacity = "1";
      clearTimeout(hide);
      hide = setTimeout(() => (keys.style.opacity = "0"), ms);
    };
  });
}, THEME === "dark");
const page = await context.newPage();
const rec = new Recorder();

let pointer = { x: 960, y: 540 };
async function glide(x, y, ms = 450) {
  const from = { ...pointer };
  const steps = Math.max(4, Math.round(ms / 16));
  for (let i = 1; i <= steps; i++) {
    const k = i / steps;
    const e = k < 0.5 ? 2 * k * k : 1 - Math.pow(-2 * k + 2, 2) / 2;
    await page.mouse.move(from.x + (x - from.x) * e, from.y + (y - from.y) * e);
    await sleep(16);
  }
  pointer = { x, y };
}
async function clickOn(locator, ms = 450) {
  await locator.waitFor();
  const box = await locator.boundingBox();
  await glide(box.x + Math.min(box.width / 2, 120), box.y + box.height / 2, ms);
  await sleep(120);
  await page.mouse.down();
  await sleep(70);
  await page.mouse.up();
}
async function press(key, label = key) {
  // A one-letter shortcut must not land in a text box.
  if (key.length === 1) await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
  await page.evaluate((t) => window.__showKeys?.(t), label);
  await sleep(250);
  await page.keyboard.press(key);
}
/** A line of words at the foot of the picture, where a pressed key shows: what is about to happen. */
const caption = (on, text, ms = 2600) => on.evaluate(([t, ms]) => window.__showKeys?.(t, ms), [text, ms]);
/** Waits for Claude; the wait plays sped up in the video. */
async function waitClaude(check, timeout) {
  const start = Date.now();
  const v = await until(check, timeout);
  rec.cut(start, Date.now());
  return v;
}
/** A still for the README, without the pointer (and left out of the video). */
async function shot(name) {
  const overlays = (v) => page.evaluate((v) => document.querySelectorAll("[data-demo-overlay]").forEach((e) => (e.style.visibility = v)), v);
  rec.paused = true;
  await overlays("hidden");
  await page.screenshot({ path: path.join(IMG, `${name}.png`) });
  await overlays("");
  await sleep(100);
  rec.paused = false;
}

const dialog = page.locator('[role="dialog"]');
const option = (list, text) => page.locator(`[role="listbox"][aria-label="${list}"] [role="option"]`, { hasText: text }).first();
const row = (text) => page.locator('nav[aria-label="Sessions"]').getByText(text).first();
// A session can have two requests waiting at once (two lookups asked for together): the first is answered first.
const allow = page.getByRole("button", { name: "Allow", exact: true }).first();

/** Fills in the new-session dialog and starts the session. With `picture`, it takes that still and closes the dialog instead. */
async function newSession({ query, folder, prompt, model, mode, picture }) {
  await press("c", "C");
  await dialog.waitFor();
  await sleep(400);
  await clickOn(page.locator('button[aria-label="Choose project folder"]'));
  await page.locator('input[aria-label="Folder path"]').pressSequentially(query, { delay: 80 });
  await sleep(500);
  await clickOn(option("Folders", folder), 350);
  await sleep(300);
  await clickOn(page.locator('[role="dialog"] textarea'));
  await page.locator('[role="dialog"] textarea').pressSequentially(prompt, { delay: 14 });
  await sleep(300);
  for (const [name, value] of [
    ["Model", model],
    ["Permissions", mode],
  ]) {
    if (!value) continue;
    await clickOn(page.locator(`[role="dialog"] button[aria-label^="${name}:"]`), 350);
    await sleep(300);
    await clickOn(option(name, value), 300);
    await sleep(250);
  }
  await sleep(500);
  if (picture) {
    await sleep(2200); // the 2 s system tick delivers the memory numbers
    await shot(picture);
    await page.keyboard.press("Escape");
  } else {
    await press("Control+Enter", "Ctrl Enter");
  }
  await dialog.waitFor({ state: "detached" });
}

const planCard = page.locator("[data-plan-card]");
const questions = page.locator("[data-question-panel]");

/**
 * Answers what `dir`'s session asks until its turn ends: a request to run or edit is allowed, a
 * plan is approved into auto-accept edits, and a question gets its first option.
 */
async function approveUntilDone(dir, picture) {
  let pictured = false;
  for (let i = 0; i < 12; i++) {
    const s = await waitClaude(async () => {
      const s = await session(dir);
      return ["needs_you", "idle", "error"].includes(s?.status) && s;
    });
    if (s.status !== "needs_you") return s;
    const asks = s.pending[0]?.tool_name;
    if (asks === "ExitPlanMode") {
      // The plan from its top, then down to the choices under it.
      await planCard.waitFor();
      await planCard.evaluate((el) => el.scrollIntoView({ block: "start", behavior: "smooth" }));
      await sleep(3200);
      const choices = planCard.locator("[data-plan-choices]");
      await choices.evaluate((el) => el.scrollIntoView({ block: "end", behavior: "smooth" }));
      await sleep(1600);
      await clickOn(planCard.locator("button", { hasText: "Yes, auto-accept edits" }), 500);
    } else if (asks === "AskUserQuestion") {
      await questions.waitFor();
      await sleep(1800);
      for (let n = 0; n < 8 && (await questions.count()) > 0; n++) {
        await clickOn(questions.locator("[data-option]").first(), 400);
        await sleep(700);
        const onward = questions.locator("[data-onward]");
        if ((await onward.count()) > 0 && (await onward.isEnabled())) {
          await clickOn(onward, 300);
          await sleep(600);
        }
      }
    } else {
      await allow.waitFor();
      await sleep(1600);
      // The still shows the whole window at work: the plan limits and the context ring too.
      if (picture && !pictured && (await page.locator("[data-plan-limits]").count()) > 0 && (await page.locator("[data-context-wheel]").count()) > 0) {
        await shot(picture);
        pictured = true;
      }
      await clickOn(allow, 400);
    }
    await sleep(900);
  }
  throw new Error("too many requests");
}

// ------------------------------------------------------------ the demo

try {
  fs.mkdirSync(IMG, { recursive: true });
  await page.goto(up());
  await page.waitForSelector("text=Run all your agents from one place");
  await rec.attach(page);
  await sleep(1800);

  // Three agents, three projects, three models and permission levels. The first one plans before it builds.
  await newSession({
    query: "recipe",
    folder: "recipe-site",
    prompt: "Build a one-page recipe site in index.html for lemon pasta: a hero, an ingredient checklist and numbered steps. Plain HTML and CSS in one file.",
    model: "Sonnet",
    mode: "Plan only",
  });
  await sleep(1500);
  await newSession({
    query: "budget",
    folder: "budget-cli",
    prompt: "test_budget.py fails. Find the bug in budget.py, fix it, then run python -m unittest to confirm.",
    model: "Haiku",
  });
  await sleep(1200);
  await newSession({
    query: "react",
    folder: "react-notes",
    prompt: "Use Context7 to look up React 19's useActionState hook, then write notes.md with a short explanation and one example.",
    model: "Haiku",
    mode: "Auto-accept edits",
  });
  await sleep(2500);

  // A look at each of them: the pointer goes to a session's row and clicks it.
  await clickOn(row("Build a one-page recipe"), 800);
  await sleep(2200);

  // The bug fix asks before it edits and before it runs the tests.
  await clickOn(row("test_budget.py fails"), 800);
  await approveUntilDone(budget, "session");
  await sleep(2200);

  // The recipe site: its plan is waiting. Approved, it is built with edits accepted. Then a
  // follow-up on a faster model.
  await clickOn(row("Build a one-page recipe"), 800);
  await approveUntilDone(recipe);
  await sleep(2200);
  await clickOn(page.locator('aside[aria-label="Properties"] button[aria-label^="Model:"]'));
  await sleep(350);
  await clickOn(option("Model", "Haiku"), 300);
  await sleep(700);
  await clickOn(page.locator("main textarea[data-composer]"));
  await page.locator("main textarea[data-composer]").pressSequentially("Add a Print recipe button under the title.", { delay: 22 });
  await sleep(300);
  await press("Enter");
  await sleep(1500);
  await approveUntilDone(recipe);
  await sleep(1800);

  // Jumping anywhere with the command palette.
  await press("Control+k", "Ctrl K");
  await sleep(500);
  await page.keyboard.type("react", { delay: 90 });
  await sleep(500);
  await shot("palette");
  await press("Enter");
  await approveUntilDone(notes);
  await sleep(2600);

  // What the agents built. hangar does not open files: the page is opened in a browser tab, as a
  // double-click on the file would, and a caption says so.
  await caption(page, "Opening the page Claude built: recipe-site/index.html");
  await sleep(2400);
  const site = await context.newPage();
  await site.goto(pathToFileURL(path.join(recipe, "index.html")).href);
  await rec.attach(site);
  await caption(site, "recipe-site/index.html, in a browser tab", 3200);
  await sleep(1500);
  for (let i = 0; i < 6; i++) {
    await site.mouse.wheel(0, 160);
    await sleep(260);
  }
  await sleep(1400);
  await page.bringToFront();
  await rec.attach(page);
  await caption(page, "Back in hangar", 1600);
  await sleep(900);

  // The pool: one Playwright and one Context7 for all three sessions.
  await clickOn(page.locator('nav[aria-label="Main"]').getByText("MCP pool"));
  await page.locator('[aria-label="MCP pool"]').waitFor();
  await sleep(2500); // the 2 s system tick delivers the memory numbers
  await shot("pool");
  await sleep(1500);
  await rec.stop();

  rec.writeList();
  console.log(`${rec.frames.length} frames in ${rec.dir}`);
  for (const dir of [recipe, budget, notes]) {
    const s = await session(dir);
    console.log(`  ${path.basename(dir)}: ${s.status}, ${s.turns} turns, $${s.cost_usd.toFixed(3)}`);
  }

  // The new-session still, taken last and outside the video. By now Claude Code has reported the
  // plan limits, so the sidebar shows them, as it does in daily use. The sessions are removed
  // first: the window behind the dialog is the empty one a new user sees.
  for (const dir of [recipe, budget, notes]) {
    const s = await session(dir);
    if (s) await api("DELETE", `/sessions/${s.id}`).catch(() => {});
  }
  await until(async () => (await api("GET", "/state")).sessions.length === 0, 30_000);
  await clickOn(page.locator('nav[aria-label="Main"]').getByText("Sessions"));
  await page.waitForSelector("text=Run all your agents from one place");
  await newSession({
    query: "recipe",
    folder: "recipe-site",
    prompt: "Build a one-page recipe site in index.html for lemon pasta: a hero, an ingredient checklist and numbered steps. Plain HTML and CSS in one file.",
    model: "Sonnet",
    mode: "Plan only",
    picture: "new-session",
  });

  if (hasFfmpeg()) {
    encode(rec.dir);
    fs.rmSync(rec.dir, { recursive: true, force: true });
  } else {
    console.log(`\nffmpeg was not found, so the frames are kept. To make the video:\n  FFMPEG_PATH=/path/to/ffmpeg npm run media -- --encode "${rec.dir}"`);
  }
} finally {
  await browser.close();
  execFileSync(hangar, ["stop"], { env, stdio: "ignore" });
  await sleep(1500);
  fs.rmSync(work, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
}
