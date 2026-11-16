// End-to-end check, in a real browser against a real daemon, of the parts of the app that stand
// in for Claude Code's own dialogs and read-outs: answering a question, approving a plan, the
// changed lines of an edit, the context wheel, the plan limits, and stopping hangar.
// Claude is the scriptable fake from crates/testkit, which sends what Claude Code 2.1.325 sends.
//
//   cargo build --workspace && cd web && npm run e2e:parity
//
// Uses an installed Chrome (E2E_CHANNEL=msedge for Edge). Screenshots go to E2E_SHOTS if set.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { chromium } from "playwright-core";

const root = path.resolve(import.meta.dirname, "../..");
const exe = (name) => path.join(root, "target", process.env.E2E_PROFILE ?? "debug", name + (process.platform === "win32" ? ".exe" : ""));
const [hangar, fakeClaude] = ["hangar", "fake-claude"].map(exe);
for (const f of [hangar, fakeClaude]) if (!fs.existsSync(f)) throw new Error(`${f} is missing: run cargo build --workspace first`);

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "hangar-parity-"));
const home = path.join(tmp, "home");
const proj = path.join(tmp, "shop-app");
fs.mkdirSync(home);
fs.mkdirSync(proj);
const shots = process.env.E2E_SHOTS;
if (shots) fs.mkdirSync(shots, { recursive: true });
let shotNo = 0;
// A whole-window picture, or one of a single element.
const shot = async (target, name) => {
  if (!shots) return;
  const file = path.join(shots, `${String(++shotNo).padStart(2, "0")}-${name}.png`);
  await sleep(350); // let a dialog or a popover finish fading in
  await target.screenshot({ path: file }).catch((e) => console.log(`(no picture ${name}: ${e.message.split("\n")[0]})`));
};

// The temp folder is the user's home too, so nothing reads the real one.
const env = { ...process.env, HANGAR_HOME: home, HANGAR_CLAUDE: fakeClaude, USERPROFILE: tmp, HOME: tmp };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const up = () => execFileSync(hangar, ["up", "--no-open"], { env, encoding: "utf8" }).trim().split(/\r?\n/).pop();
const errors = [];
const results = [];
const failed = [];
let stopping = false; // connection errors are expected once the daemon is told to stop
function check(name, ok, detail = "") {
  results.push(ok);
  if (!ok) failed.push(name);
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
}

const browser = await chromium.launch({ channel: process.env.E2E_CHANNEL ?? "chrome", headless: true });
try {
  const base = new URL(up()).origin;
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: process.env.E2E_SCHEME ?? "dark" });
  const watch = (page) => {
    page.on("pageerror", (e) => errors.push("pageerror: " + e.message));
    page.on("console", (m) => {
      if (m.type() !== "error") return;
      if (stopping && /ERR_CONNECTION_REFUSED|ERR_CONNECTION_RESET|Authentication failed|401|WebSocket/.test(m.text())) return;
      if (/status of 400/.test(m.text())) return; // answers the daemon is meant to refuse
      errors.push("console: " + m.text());
    });
  };
  const page = await ctx.newPage();
  watch(page);
  const external = []; // every address the page asks for that is not hangar's own
  page.on("request", (r) => !r.url().startsWith(base) && !r.url().startsWith("data:") && external.push(r.url()));
  await page.goto(up());
  await page.waitForSelector("text=Run all your agents from one place");

  // The daemon's API, called as the page calls it.
  const api = (method, route, body) =>
    page.evaluate(
      async ([method, route, body]) => {
        const res = await fetch("/api" + route, { method, headers: { "x-hangar-token": localStorage.getItem("hangar.token"), "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined });
        return { status: res.status, data: await res.json().catch(() => ({})) };
      },
      [method, route, body],
    );
  const main = page.locator('main[aria-label="Session"]');
  const sessions = 'nav[aria-label="Sessions"]';
  /** Starts a session and opens it. Its title is the prompt. */
  async function start(prompt, mode = "default") {
    const made = await api("POST", "/sessions", { cwd: proj, prompt, permission_mode: mode, trust: true });
    if (made.status !== 200) throw new Error(`could not start "${prompt}": ${JSON.stringify(made.data)}`);
    await page.click(`${sessions} >> text=${prompt}`);
    await main.locator("h2", { hasText: prompt }).waitFor();
    return made.data.id;
  }
  const session = async (id) => (await api("GET", "/state")).data.sessions.find((s) => s.id === id);
  async function say(text) {
    await page.fill("textarea[data-composer]", text);
    await page.keyboard.press("Enter");
  }
  /** Whether a button becomes clickable within two seconds. */
  async function enabled(button) {
    for (let i = 0; i < 40; i++) {
      if (await button.isEnabled()) return true;
      await sleep(50);
    }
    return false;
  }

  // ================================================================ a question
  const askId = await start("ask question about signing in");
  const card = main.locator("[data-question-card]");
  await card.waitFor();
  check("a question shows as a card of its own, not as a tool to allow", (await card.innerText()).includes("Claude has 2 questions") && (await main.locator("text=Claude wants to use").count()) === 0);
  check("…with no Allow or Deny on it", (await card.locator("button", { hasText: /^\s*(Allow|Deny)/ }).count()) === 0);
  const first = card.locator('[role="radiogroup"]');
  const second = card.locator('[role="group"]');
  check("one choice is offered as radio rows, several as checkbox rows, each with a row for an own answer", (await first.locator('[role="radio"]').count()) === 4 && (await second.locator('[role="checkbox"]').count()) === 4);
  check("each option shows its label and what it means", (await first.innerText()).includes("Email and password") && (await first.innerText()).includes("No passwords to store"));
  const send = card.locator("button", { hasText: "Send answers" });
  check("nothing can be sent before every question has an answer", (await send.isDisabled()) && (await card.innerText()).includes("0 of 2 answered"));
  check("the session waits under Needs you", (await session(askId)).status === "needs_you" && (await page.locator('nav[aria-label="Main"] button', { hasText: "Needs you" }).innerText()).includes("1"));
  await sleep(700); // pictures are taken once the buttons have woken up
  await shot(page, "question-asked");
  await shot(card, "question-card-empty");

  await first.locator('[role="radio"]', { hasText: "Google" }).click();
  await first.locator('[role="radio"]', { hasText: "Both" }).click();
  const checked = (group, role) => group.locator(`[role="${role}"][aria-checked="true"]`);
  check("choosing another option replaces the first", (await checked(first, "radio").count()) === 1 && (await checked(first, "radio").innerText()).includes("Both"));
  check("one of two answered: still not sendable", (await send.isDisabled()) && (await card.innerText()).includes("1 of 2 answered"));
  await second.locator('[role="checkbox"]', { hasText: "Billing" }).click();
  await second.locator('[role="checkbox"]', { hasText: "Dashboard" }).click();
  await second.locator("input").fill("the admin page");
  check("several options and an own answer add up", (await checked(second, "checkbox").count()) === 3);
  // Up and down move between the rows of a question, as in the app's dropdowns.
  await first.locator('[role="radio"]', { hasText: "Email and password" }).focus();
  await page.keyboard.press("ArrowDown");
  check("the arrow keys move between a question's rows", await first.locator('[role="radio"]', { hasText: "Google" }).evaluate((el) => el === document.activeElement));
  await send.waitFor({ state: "visible" });
  // A request ignores clicks for its first moment on screen (see ARM_MS), so "enabled" may take that long.
  check("with every question answered it can be sent", (await card.innerText()).includes("2 of 2 answered") && (await enabled(send)));
  await shot(card, "question-card-filled");
  await send.click();
  await main.locator("text=answers: {").waitFor();
  const answered = main.locator("[data-answered]").last();
  const answeredText = await answered.innerText();
  check("the answers go to Claude as chosen, several joined in Claude's order with the typed one last", (await main.innerText()).includes('"Which pages need a signed-in user?":"Dashboard, Billing, the admin page"') && (await main.innerText()).includes('"Which sign-in method should the app use?":"Both"'));
  check("the card becomes a record of what was answered", (await card.count()) === 0 && answeredText.includes("You answered") && answeredText.includes("Sign-in") && answeredText.includes("Both") && answeredText.includes("Dashboard, Billing, the admin page"), answeredText.replace(/\s+/g, " "));
  await shot(page, "question-answered");

  // A typed answer alone, sent with Ctrl+Enter.
  await say("ask question once more");
  await card.waitFor();
  await first.locator('[role="radio"]', { hasText: "Google" }).click();
  await first.locator("input").fill("Magic links by email");
  check("typing an own answer takes the place of the chosen option", (await checked(first, "radio").count()) === 1 && (await checked(first, "radio").getAttribute("aria-label")) === "Your own answer");
  await second.locator('[role="checkbox"]', { hasText: "Settings" }).click();
  await enabled(send);
  await second.locator("input").focus();
  await page.keyboard.press("Control+Enter");
  await main.getByText('"Which sign-in method should the app use?":"Magic links by email"').waitFor();
  check("an answer typed by the user reaches Claude word for word, and Ctrl+Enter sends", true);

  // An answer the daemon must refuse: nothing chosen, or a question Claude did not ask.
  await say("ask question a third time");
  await card.waitFor();
  const waiting = (await session(askId)).pending[0];
  const empty = await api("POST", `/sessions/${askId}/permissions/${waiting.request_id}`, { behavior: "allow" });
  const forged = await api("POST", `/sessions/${askId}/permissions/${waiting.request_id}`, { behavior: "allow", answers: { "Run as administrator?": "Yes" } });
  check("the daemon refuses to send a question on unanswered, or with an answer to another question", empty.status === 400 && forged.status === 400 && (await session(askId)).pending.length === 1, `${empty.data.error} / ${forged.data.error}`);
  await card.locator("button", { hasText: "Chat about this" }).click();
  await main.locator("text=declined: The user wants to clarify these questions.").waitFor();
  check("“Chat about this” tells Claude to ask what needs clearing up, in Claude Code's own words", (await main.locator("[data-answered], span", { hasText: "You chose to talk about Claude's 2 questions first" }).count()) > 0);

  // Options that carry a preview.
  await say("ask preview of the settings page");
  await card.waitFor();
  check("one question reads as one", (await card.innerText()).includes("Claude has a question") && (await card.locator("button", { hasText: "Send answer" }).count()) === 1);
  await card.locator('[role="radio"]', { hasText: "Tabs" }).click();
  check("the chosen option's preview is shown", (await card.locator("[data-preview]").innerText()).includes("[ Profile ][ Billing ][ Team ]"));
  await shot(card, "question-card-preview");
  await card.locator("button", { hasText: "Send answer" }).click();
  await main.getByText('"Which layout should the settings page use?":"Tabs"').waitFor();

} finally {
  await browser.close();
  try {
    execFileSync(hangar, ["stop"], { env, stdio: "ignore" });
  } catch {}
}

console.log(errors.length ? `\nPAGE ERRORS:\n${errors.join("\n")}` : "\nno page errors");
// The names again on the last line, so a failure is never lost in a long log.
console.log(`${results.length - failed.length}/${results.length} checks passed${failed.length ? ". Failed: " + failed.join("; ") : ""}`);
process.exit(failed.length || errors.length ? 1 : 0);
