// End-to-end check of the web app in a real browser against a real daemon.
// Claude is the scriptable fake from crates/testkit, so it costs nothing and
// needs no network.
//
//   cargo build --workspace && cd web && npm run e2e
//
// Uses an installed Chrome (E2E_CHANNEL=msedge for Edge). Screenshots go to
// E2E_SHOTS if set.
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { chromium } from "playwright-core";

const root = path.resolve(import.meta.dirname, "../..");
const exe = (name) => path.join(root, "target", process.env.E2E_PROFILE ?? "debug", name + (process.platform === "win32" ? ".exe" : ""));
const [hangar, shim, fakeClaude, fakeMcp] = ["hangar", "hangar-shim", "fake-claude", "fake-mcp"].map(exe);
for (const f of [hangar, shim, fakeClaude, fakeMcp]) if (!fs.existsSync(f)) throw new Error(`${f} is missing: run cargo build --workspace first`);

// A fresh state dir, and a project folder with settings of its own.
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "hangar-e2e-"));
const home = path.join(tmp, "home");
const proj = path.join(tmp, "proj");
fs.mkdirSync(home);
fs.mkdirSync(path.join(proj, ".claude"), { recursive: true });
fs.writeFileSync(path.join(home, "config.toml"), '[servers.fake]\nshare = "global"\n');
fs.writeFileSync(
  path.join(proj, ".claude", "settings.json"),
  JSON.stringify({ hooks: { PreToolUse: [{ matcher: "Bash", hooks: [{ type: "command", command: "./scripts/check.sh" }] }] }, permissions: { allow: ["Bash(npm test)"] } }),
);
fs.writeFileSync(path.join(proj, ".mcp.json"), JSON.stringify({ mcpServers: { docs: { command: "node", args: ["docs-server.js"] } } }));
const shots = process.env.E2E_SHOTS;
if (shots) fs.mkdirSync(shots, { recursive: true });
const shot = (page, name) => (shots ? page.screenshot({ path: path.join(shots, `${name}.png`) }) : Promise.resolve());

const env = { ...process.env, HANGAR_HOME: home, HANGAR_CLAUDE: fakeClaude };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const up = () => execFileSync(hangar, ["up", "--no-open"], { env, encoding: "utf8" }).trim().split(/\r?\n/).pop();
const errors = [];
const results = [];
let restarting = false; // connection errors are expected while the daemon restarts
function check(name, ok, detail = "") {
  results.push(ok);
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
}

const clients = [];
const browser = await chromium.launch({ channel: process.env.E2E_CHANNEL ?? "chrome", headless: true });
try {
  const pairUrl = up();
  const base = new URL(pairUrl).origin;
  const info = JSON.parse(fs.readFileSync(path.join(home, "daemon.json"), "utf8"));
  check("hangar up prints a one-time pairing URL", /\/#pair=[0-9a-f]{32}$/.test(pairUrl));
  check("the URL does not contain the daemon token", !pairUrl.includes(info.token));

  // Three agent-like clients sharing one server through the shim.
  for (let i = 0; i < 3; i++) {
    const p = spawn(shim, ["--name", "fake", "--", fakeMcp], { env: { ...env, HANGAR_NO_AUTOSTART: "1" }, stdio: ["pipe", "pipe", "ignore"] });
    p.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "e2e", version: "1" } } }) + "\n");
    clients.push(p);
  }

  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: process.env.E2E_SCHEME ?? "dark" });
  const page = await ctx.newPage();
  const external = [];
  page.on("pageerror", (e) => errors.push("pageerror: " + e.message));
  page.on("console", (m) => {
    if (m.type() !== "error") return;
    if (restarting && /ERR_CONNECTION_REFUSED|Authentication failed|401/.test(m.text())) return;
    if (/status of 401/.test(m.text())) return; // the signed-out probe
    errors.push("console: " + m.text());
  });
  page.on("request", (r) => !r.url().startsWith(base) && !r.url().startsWith("data:") && external.push(r.url()));

  // ---- sign-in
  await page.goto(pairUrl);
  await page.waitForSelector("text=Run all your agents from one place");
  check("pairing signs the window in", true);
  check("the code is removed from the address bar", !page.url().includes("pair="), page.url());
  await shot(page, "1-welcome");

  // ---- the folder picker: browse, create, choose
  const dialog = page.locator('[role="dialog"]');
  const folderButton = 'button[aria-label="Choose project folder"]';
  const folderPath = 'input[aria-label="Folder path"]';
  const option = (text) => page.locator('[role="listbox"][aria-label="Folders"] [role="option"]', { hasText: text });
  async function pickFolder(path) {
    await page.click(folderButton);
    await page.fill(folderPath, path);
    await option("Use this folder").waitFor();
    await page.keyboard.press("Enter");
    await page.locator(folderPath).waitFor({ state: "detached" });
  }
  await page.keyboard.press("Control+n");
  await dialog.waitFor();
  await page.click(folderButton);
  await page.fill(folderPath, tmp + path.sep);
  await option("proj").waitFor();
  check("typing a path lists its folders", (await option("home").count()) === 1 && (await option("proj").count()) === 1);
  await page.locator(folderPath).press("ArrowDown"); // past "Use this folder" and "Parent folder", onto "home"
  await page.locator(folderPath).press("ArrowDown");
  await page.locator(folderPath).press("Tab");
  await option("Use this folder").waitFor();
  check("Tab goes into a folder", (await page.inputValue(folderPath)).endsWith(path.sep) && (await page.inputValue(folderPath)).includes(path.sep + "home"), await page.inputValue(folderPath));
  await page.fill(folderPath, path.join(tmp, "made-by-picker"));
  await option("Create folder").waitFor();
  await page.keyboard.press("Enter");
  await page.locator(folderPath).waitFor({ state: "detached" });
  check("the picker creates a new folder and selects it", fs.existsSync(path.join(tmp, "made-by-picker")) && (await page.locator(folderButton).innerText()).includes("made-by-picker"));
  await shot(page, "0-picker");
  await pickFolder(proj);
  check("choosing a folder shows it", (await page.locator(folderButton).innerText()).includes("proj"));

  // ---- custom dropdowns
  await page.click('button[aria-label="Model: Default model"]');
  await page.locator('[role="listbox"][aria-label="Model"] [role="option"]', { hasText: "Haiku" }).click();
  check("the model dropdown is the app's own and picks Haiku", (await page.locator('button[aria-label="Model: Haiku"]').count()) === 1);

  // ---- a new session in an untrusted folder
  await page.fill('[role="dialog"] textarea', "Refactor the auth module into smaller functions and add tests");
  await page.waitForSelector("text=New folder");
  const startBtn = dialog.locator('button:has-text("Start session"), button:has-text("Queue session")');
  check("an untrusted folder must be confirmed first", await startBtn.isDisabled());
  const notice = await dialog.innerText();
  check("the notice lists the folder's hooks, allowed tools and MCP servers", ["./scripts/check.sh", "Bash(npm test)", "docs: node"].every((s) => notice.includes(s)));
  await shot(page, "0-trust");
  await page.check("text=I trust this folder");
  check("confirming enables Start", await startBtn.isEnabled());
  await page.keyboard.press("Control+Enter");
  await dialog.waitFor({ state: "detached" });
  await page.waitForSelector("text=echo: Refactor the auth module");

  async function newSession(prompt) {
    await page.keyboard.press("Control+n");
    await dialog.waitFor();
    if (!(await page.locator(folderButton).innerText()).includes("proj")) await pickFolder(proj);
    await page.fill('[role="dialog"] textarea', prompt);
    await sleep(500); // the trust lookup
    check(`a trusted folder asks nothing again (${prompt.slice(0, 16)}…)`, (await page.locator("text=New folder").count()) === 0);
    await page.keyboard.press("Control+Enter");
    await dialog.waitFor({ state: "detached" });
  }
  await newSession("Audit dependencies, then use tool to list the repo");
  await page.waitForSelector("text=Claude wants to use Bash");
  check("the session-scoped approval is offered", (await page.locator("button:has-text('Allow for session')").count()) === 1);
  await newSession("Write release notes for v0.4 from the changelog");
  await sleep(800);

  // ---- permissions from the properties panel, with the app's dropdown
  await page.click('button[aria-label="Permissions: Ask first"]');
  await page.locator('[role="listbox"][aria-label="Permissions"] [role="option"]', { hasText: "Plan only" }).click();
  await page.locator('button[aria-label="Permissions: Plan only"]').waitFor();
  check("permissions change from the properties panel", true);
  await page.click('nav[aria-label="Sessions"] >> text=Refactor the auth');
  await sleep(200);
  check("the model chosen in the dialog is the session's", (await page.locator('aside[aria-label="Properties"]').innerText()).includes("haiku"));
  await page.click('nav[aria-label="Sessions"] >> text=Write release notes');
  await sleep(200);

  // ---- drafts survive switching sessions
  const composer = "main textarea[data-composer]";
  await page.fill(composer, "half-written thought");
  await page.click('nav[aria-label="Sessions"] >> text=Audit dependencies');
  await page.waitForSelector("text=Claude wants to use Bash");
  await page.click('nav[aria-label="Sessions"] >> text=Write release notes');
  await sleep(200);
  check("a draft survives switching sessions", (await page.inputValue(composer)) === "half-written thought");
  await page.fill(composer, "");

  // ---- the menu closes on Escape without side effects
  await page.click('nav[aria-label="Sessions"] >> text=Audit dependencies');
  await page.waitForSelector("text=Claude wants to use Bash");
  await sleep(2500); // the 2 s system tick delivers pool and memory numbers
  await shot(page, "2-needs-you");
  await page.click('button[aria-label="More"]');
  await page.waitForSelector('[role="menu"]');
  await page.keyboard.press("Escape");
  await sleep(200);
  check("Escape closes the menu", (await page.locator('[role="menu"]').count()) === 0);
  check("…and leaves the permission request alone", (await page.locator("text=Claude wants to use Bash").count()) === 1);

  await page.click("button:has-text('Allow') >> nth=0");
  await page.waitForSelector("text=allow: ls");
  await shot(page, "3-allowed");

  // ---- follow-ups and per-turn cost
  await page.fill(composer, "Great. Now summarize what you found in two bullets.");
  await page.keyboard.press("Enter");
  await page.waitForSelector("text=echo: Great. Now summarize");
  await sleep(300);
  const costs = await page.locator("main >> text=/Done in .*\\$/").allInnerTexts();
  check("each turn shows its own cost, not a running total", costs.length >= 2 && costs.every((c) => c.includes("$0.010")), costs.join(" / "));

  // ---- model output cannot load remote images
  await page.fill(composer, "look ![pixel](http://example.invalid/track.png)");
  await page.keyboard.press("Enter");
  await page.waitForSelector("text=[image: pixel]");
  check("images in replies become links", (await page.locator("main img").count()) === 0);
  check("no request left the machine", external.length === 0, external.join(", "));

  // ---- a crash says why, once
  await page.fill(composer, "crash");
  await page.keyboard.press("Enter");
  await page.waitForSelector("text=Stopped with an error");
  const mainText = await page.locator("main").innerText();
  check("an error shows its reason and how to continue", /exited with code 5/.test(mainText) && /Send a message to resume/.test(mainText));
  check("…once, not twice", (mainText.match(/exited with code 5/g) ?? []).length === 1);
  check("the window title counts sessions that need you", (await page.title()).startsWith("("), await page.title());
  await shot(page, "5-error");

  // ---- the pool view
  await page.click('nav[aria-label="Main"] >> text=MCP pool');
  const pool = page.locator('[aria-label="MCP pool"]');
  await pool.waitFor();
  await sleep(2200);
  const poolText = await pool.innerText();
  check("one shared server serves all three clients", /fake[\s\S]*Shared[\s\S]*3 sessions/.test(poolText), poolText.replace(/\n+/g, " | ").slice(0, 160));
  await shot(page, "4-pool");
  await page.keyboard.press("Escape");
  await sleep(200);
  check("Escape leaves the pool view", (await pool.count()) === 0 && (await page.locator('nav[aria-label="Sessions"]').count()) === 1);

  // ---- keyboard: command palette, J/K, C
  const current = async () => (await page.locator('nav[aria-label="Sessions"] [aria-current="true"]').innerText().catch(() => "")).split("\n")[0];
  await page.locator("body").click({ position: { x: 5, y: 5 } }); // focus out of the composer
  await page.keyboard.press("Control+k");
  await page.waitForSelector('[aria-label="Command palette"]');
  await page.keyboard.type("release notes");
  await page.keyboard.press("Enter");
  await sleep(200);
  check("the command palette finds and opens a session", (await current()).startsWith("Write release notes"), await current());
  await page.locator("body").click({ position: { x: 5, y: 5 } });
  const before = await current();
  await page.keyboard.press("j");
  await sleep(150);
  const after = await current();
  await page.keyboard.press("k");
  await sleep(150);
  check("J and K move between sessions", after !== before && (await current()) === before, `${before} → ${after}`);
  await page.keyboard.press("c");
  await dialog.waitFor();
  check("C opens a new session", true);
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "detached" });

  // ---- views: Needs you, and a project
  await page.click('nav[aria-label="Main"] >> text=Needs you');
  await sleep(200);
  const inbox = await page.locator('nav[aria-label="Sessions"]').innerText();
  check("Needs you lists only sessions that need you", inbox.includes("Audit dependencies") && !inbox.includes("Write release notes"), inbox.replace(/\n+/g, " | ").slice(0, 120));
  await page.click('section[aria-label="Projects"] >> text="proj"');
  await sleep(200);
  check("a project lists its sessions", (await page.locator('nav[aria-label="Sessions"] li').count()) === 3);
  await page.click('nav[aria-label="Main"] >> text=Sessions');

  // ---- a phone-sized window shows the list, then one session at a time
  const phone = await browser.newContext({ viewport: { width: 400, height: 800 } });
  const mp = await phone.newPage();
  mp.on("pageerror", (e) => errors.push("pageerror: " + e.message));
  await mp.goto(up());
  await mp.waitForSelector('nav[aria-label="Sessions"]');
  check("on a phone, the list comes first", (await mp.locator('main[aria-label="Session"]').count()) === 0);
  await mp.click('nav[aria-label="Sessions"] >> text=Write release notes');
  await mp.waitForSelector('main[aria-label="Session"]');
  check("…a session then fills the screen", !(await mp.locator('nav[aria-label="Sessions"]').isVisible()));
  await mp.click('button[aria-label="Back to the list"]');
  await mp.waitForSelector('nav[aria-label="Sessions"]');
  check("…and Back returns to the list", true);
  await mp.click('button[aria-label="Toggle sidebar"]');
  await mp.waitForSelector('[aria-label="Navigation"]');
  await mp.keyboard.press("Escape");
  await sleep(250);
  check("the sidebar drawer opens and Escape closes it", (await mp.locator('[aria-label="Navigation"]').count()) === 0);
  await shot(mp, "6-phone");
  await phone.close();

  // ---- reload keeps the sign-in; the used link signs in nobody else
  await page.reload();
  await page.waitForSelector('nav[aria-label="Sessions"] >> text=Audit dependencies');
  check("reloading stays signed in", true);
  const other = await browser.newContext();
  const p2 = await other.newPage();
  await p2.goto(pairUrl);
  await p2.waitForSelector("text=Open hangar from your terminal");
  check("a used pairing link signs nobody else in", (await p2.innerText("body")).includes("expired or was already used"));
  check("the API refuses requests without the token", (await p2.evaluate(async () => (await fetch("/api/state")).status)) === 401);
  await other.close();

  // ---- a restarted daemon has a new token: the window asks to sign in again
  restarting = true;
  execFileSync(hangar, ["stop"], { env });
  await page.waitForSelector("text=Lost the daemon", { timeout: 10000 });
  check("losing the daemon is visible", true);
  const fresh = up();
  await page.waitForSelector("text=Open hangar from your terminal", { timeout: 20000 });
  check("a restarted daemon asks this window to sign in again", true);
  await page.goto(fresh); // same page, new hash
  await page.waitForSelector('nav[aria-label="Sessions"] >> text=Audit dependencies');
  check("the new link signs it back in, sessions intact", true);
} finally {
  for (const c of clients) c.kill();
  await browser.close();
  try {
    execFileSync(hangar, ["stop"], { env, stdio: "ignore" });
  } catch {}
}

const failed = results.filter((ok) => !ok).length;
console.log(errors.length ? `\nPAGE ERRORS:\n${errors.join("\n")}` : "\nno page errors");
console.log(`${results.length - failed}/${results.length} checks passed`);
process.exit(failed || errors.length ? 1 : 0);
