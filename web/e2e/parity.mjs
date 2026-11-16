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

  // ================================================================ a plan
  const planId = await start("make plan for the auth module", "plan");
  const plan = main.locator("[data-plan-card]");
  await plan.waitFor();
  const planBox = plan.locator("[data-approval-content]");
  check("a plan shows as a card of its own, formatted", (await plan.innerText()).includes("Claude's plan") && (await planBox.locator("h1").innerText()) === "Plan: split the auth module" && (await planBox.locator("ol > li").count()) === 4 && (await planBox.locator("table tr").count()) === 4);
  const choices = (await plan.locator("button").allInnerTexts()).map((t) => t.trim());
  check("…with Claude Code's own three choices, and all of the plan showing", JSON.stringify(choices) === JSON.stringify(["Yes, auto-accept edits", "Yes, manually approve edits", "No, keep planning"]), choices.join(" | "));
  check("…and where Claude saved it", (await plan.innerText()).includes("Saved to .claude-plans") && (await plan.innerText()).includes("split-the-auth-module.md"));
  check("the session is in plan mode", (await session(planId)).permission_mode === "plan");
  await sleep(700);
  await shot(page, "plan-presented");
  await shot(plan, "plan-card");

  await plan.locator("textarea").fill("Keep the old file until the tests pass");
  await shot(plan, "plan-card-feedback");
  await plan.locator("button", { hasText: "No, keep planning" }).click();
  await main.locator("[data-plan-outcome]", { hasText: "Plan sent back" }).waitFor();
  await planBox.locator("text=Changed after your feedback").waitFor();
  check("“No, keep planning” with words sends them to Claude, which presents the plan again", (await planBox.innerText()).includes("Keep the old file until the tests pass") && (await main.locator("[data-plan-outcome]").first().innerText()).includes("“Keep the old file until the tests pass”"));
  check("…and the session is still planning", (await session(planId)).permission_mode === "plan");
  await sleep(700);
  await shot(page, "plan-sent-back");

  await plan.locator("button", { hasText: "Yes, auto-accept edits" }).click();
  await main.locator("text=plan approved (mode acceptEdits), edit made").waitFor();
  check("approving with “auto-accept edits” continues in that mode, and the edit is made without asking", (await session(planId)).permission_mode === "acceptEdits" && (await main.locator("text=Claude wants to use Edit").count()) === 0);
  check("the answered plan reads as approved, and how", (await main.locator("[data-plan-outcome]").last().innerText()).includes("Plan approved, auto-accepting edits"));
  await main.locator("[data-plan-outcome] button").last().click();
  check("…and opens to show the plan again", (await main.locator("[data-plan-outcome]").last().locator("h1").innerText()) === "Plan: split the auth module");
  await shot(page, "plan-approved");

  const manualId = await start("make plan and approve each edit", "plan");
  await plan.waitFor();
  await plan.locator("button", { hasText: "Yes, manually approve edits" }).click();
  await main.locator("text=Claude wants to use Edit").waitFor();
  check("approving with “manually approve edits” continues in ask-first mode, and the next edit asks", (await session(manualId)).permission_mode === "default" && (await main.locator("[data-plan-outcome]").innerText()).includes("asking before each edit"));
  await shot(page, "plan-approved-manual");
  await main.locator("button", { hasText: /^\s*Allow$/ }).click();
  await main.locator("text=plan approved (mode default), edit made").waitFor();

  const stopId = await start("make plan and stop to rethink", "plan");
  await plan.waitFor();
  await plan.locator("button", { hasText: "No, keep planning" }).click();
  await main.locator("[data-plan-outcome]", { hasText: "stopped, to hear what to change" }).waitFor();
  await main.locator("text=/^Stopped/").waitFor();
  const stopped = await session(stopId);
  check("“No, keep planning” with nothing typed stops the turn, still in plan mode", stopped.status === "idle" && stopped.permission_mode === "plan");
  check("a stopped turn is shown as stopped, not as failed", (await main.locator("text=The turn failed").count()) === 0 && stopped.preview === "Stopped");
  await shot(page, "plan-stopped");

  await start("long plan for every file", "plan");
  await plan.waitFor();
  const yes = plan.locator("button", { hasText: "Yes, auto-accept edits" });
  await sleep(900); // past the moment in which every new request ignores clicks
  check("a plan longer than a screen cannot be approved before all of it was opened", (await yes.isDisabled()) && (await plan.innerText()).includes("Show all of it to approve it."));
  check("…and what hides below the fold is not readable until then", !(await planBox.evaluate((el) => el.scrollHeight <= el.clientHeight + 1)));
  await shot(plan, "plan-card-long");
  await plan.locator("button", { hasText: "There is more below" }).click();
  await yes.waitFor();
  check("…after which it can", (await enabled(yes)) && (await planBox.innerText()).includes("Delete the old"));
  await yes.click();
  await main.locator("text=plan approved (mode acceptEdits), edit made").waitFor();

  // ================================================================ content that tries to mislead
  await start("ask hostile question");
  await card.waitFor();
  const pwned = () => page.evaluate(() => window.pwned);
  check("markup in a question is shown as text and never runs", (await card.locator("b, img, script, a").count()) === 0 && (await card.innerText()).includes("<b>backup</b>") && (await card.innerText()).includes("<script>window.pwned = 1</script>") && (await pwned()) === undefined);
  const marked = await card.locator("mark").allInnerTexts();
  check("invisible characters in a question and its options are written out", marked.includes("⟨U+202E⟩") && marked.includes("⟨U+200B⟩"), marked.join(" "));
  await shot(card, "question-card-hostile");
  await card.locator('[role="radio"]', { hasText: "No" }).click();
  await card.locator("button", { hasText: "Send answer" }).click();
  await main.getByText('":"No"}').waitFor();
  check("…and it can still be answered", true);

  await start("hostile plan please", "plan");
  await plan.waitFor();
  check("a plan cannot load an image, run a script or link to one", (await planBox.locator("img, script").count()) === 0 && (await planBox.locator('a[href^="javascript"]').count()) === 0 && (await pwned()) === undefined && external.length === 0, external.join(" "));
  check("a plan with invisible characters in it says so", (await plan.innerText()).includes("This plan has invisible characters in it."));
  await plan.locator("button", { hasText: "Show it as plain text" }).click();
  check("…and shows them written out on request", (await planBox.locator("mark").first().innerText()) === "⟨U+202E⟩" && (await planBox.innerText()).includes("[Approve here](javascript:window.pwned=3)"));
  await shot(plan, "plan-card-hostile");
  await plan.locator("button", { hasText: "No, keep planning" }).click();
  await main.locator("text=/^Stopped/").waitFor();

  // ================================================================ edits in the feed
  const editId = await start("edit files in the auth module");
  await main.locator("text=edited 5 files; one edit failed").waitFor();
  const diffs = main.locator("[data-diff]");
  check("every edit shows what it changed, in the feed", (await diffs.count()) === 6);
  const lines = (i, sign) => diffs.nth(i).locator(`[data-sign="${sign}"]`);
  const numbers = (i) => diffs.nth(i).evaluate((el) => [...el.querySelectorAll("[data-sign]")].map((row) => `${row.children[0].textContent.trim()}${row.dataset.sign}`.trim()));
  const firstNumbers = await numbers(0);
  check("an edit shows the file's own line numbers, with unchanged lines around the change", JSON.stringify(firstNumbers) === JSON.stringify(["14", "15", "16", "17-", "17+", "18+", "19", "20", "21"]), firstNumbers.join(" "));
  check("…the removed line and the lines that replaced it", (await lines(0, "-").innerText()).includes("const ttl = 3600;") && (await lines(0, "+").first().innerText()).includes("config.sessionTtl ?? 3600") && (await lines(0, "+").count()) === 2);
  // The color as the screen mixes it: painted on a canvas and read back as red, green, blue.
  const tint = (locator) =>
    locator.evaluate((el) => {
      const c = document.createElement("canvas").getContext("2d");
      c.fillStyle = getComputedStyle(el).backgroundColor;
      c.fillRect(0, 0, 1, 1);
      return [...c.getImageData(0, 0, 1, 1).data];
    });
  const [added, removed] = [await tint(lines(0, "+").first()), await tint(lines(0, "-"))];
  check("added lines are green and removed lines red", added[1] > added[0] && added[1] > added[2] && removed[0] > removed[1] && removed[0] > removed[2], `added rgb ${added.slice(0, 3)} removed rgb ${removed.slice(0, 3)}`);
  const stats = await main.locator("[data-diff-stat]").allInnerTexts();
  check("each row says how many lines were added and removed", JSON.stringify(stats.map((s) => s.replace(/\s+/g, " "))) === JSON.stringify(["+2 −1", "+4", "+2 −1", "+2 −2", "+60"]), stats.join(" | "));
  check("a new file is marked as one and shown from line 1", (await main.locator("text=new file").count()) === 2 && JSON.stringify(await numbers(1)) === JSON.stringify(["1+", "2+", "3+", "4+"]));
  check("a file written over shows only what changed, with the patch's own notes", (await diffs.nth(2).innerText()).includes("No newline at end of file") && (await lines(2, "+").count()) === 2);
  check("“every occurrence” shows each place, apart", (await diffs.nth(3).locator("text=⋯").count()) === 1 && (await numbers(3)).includes("89-"));
  const failedRow = main.locator("text=Not applied: String to replace not found in file.");
  check("an edit that failed says so, and shows what was asked for dimmed and without line numbers", (await failedRow.count()) === 1 && (await diffs.nth(4).evaluate((el) => getComputedStyle(el).opacity)) === "0.6" && JSON.stringify(await numbers(4)) === JSON.stringify(["−-", "++"]), (await numbers(4)).join(" "));
  check("a long change opens folded", (await diffs.nth(5).locator("[data-sign]").count()) === 12 && (await diffs.nth(5).innerText()).includes("Show all 60 lines"));
  await shot(page, "edits-in-the-feed");
  await shot(diffs.nth(0), "edit-diff");
  await shot(diffs.nth(3), "edit-diff-two-places");
  await diffs.nth(5).locator("button", { hasText: "Show all 60 lines" }).click();
  check("…and unfolds to all of it, and back", (await diffs.nth(5).locator("[data-sign]").count()) === 60);
  await diffs.nth(5).locator("button", { hasText: "Show less" }).click();
  check("…and folds again", (await diffs.nth(5).locator("[data-sign]").count()) === 12);
  await main.locator("button", { hasText: "src/auth/session.ts" }).first().click();
  check("the row still opens to the tool's raw input", (await main.locator("pre", { hasText: '"old_string": "  const ttl = 3600;"' }).count()) === 1);
  await main.locator("button", { hasText: "src/auth/session.ts" }).first().click();
  const stored = (await api("GET", `/sessions/${editId}/events`)).data.filter((e) => e.kind === "claude" && e.msg.type === "user").map((e) => e.msg.tool_use_result);
  check("the daemon keeps the changed lines only, never the whole file", stored.length === 6 && stored.every((r) => r === undefined || (!("originalFile" in r) && !("content" in r) && !("filePath" in r))));

  // ================================================================ the context wheel
  const wheel = page.locator("[data-context-wheel]");
  await wheel.waitFor();
  const reported = (await session(editId)).context;
  check("the wheel shows Claude Code's own percentage", (await wheel.innerText()).trim() === `${reported.percent}%` && reported.percent === 14, `${await wheel.innerText()} / ${JSON.stringify(reported).slice(0, 90)}`);
  const [wheelBox, inputBox] = [await wheel.boundingBox(), await page.locator("textarea[data-composer]").evaluate((el) => el.parentElement.getBoundingClientRect().toJSON())];
  check("it sits at the bottom right, under the message box", wheelBox.y >= inputBox.bottom && Math.abs(wheelBox.x + wheelBox.width - inputBox.right) < 12, `wheel right ${Math.round(wheelBox.x + wheelBox.width)} box right ${Math.round(inputBox.right)}`);
  const arc = () => wheel.locator("circle").nth(1).evaluate((el) => ({ stroke: el.getAttribute("stroke"), filled: parseFloat(el.getAttribute("stroke-dasharray")) / (2 * Math.PI * 6) }));
  check("the ring is filled as far as the percentage", Math.abs((await arc()).filled - 0.14) < 0.005 && (await arc()).stroke === "var(--accent)");
  await shot(page.locator("textarea[data-composer]").locator("xpath=ancestor::div[contains(@class,'max-w')]"), "context-wheel");
  await wheel.hover();
  const details = wheel.locator('[role="group"]');
  await details.waitFor();
  const detailsText = (await details.innerText()).replace(/\s+/g, " ");
  check("hovering shows the tokens, what fills the window, and where the numbers come from", ["14% full", "28k of 200k tokens", "System prompt 3.4k", "Messages 10k", "summarizes the conversation by itself at 167k", "Claude Code's own figures"].every((t) => detailsText.includes(t)), detailsText);
  await shot(page, "context-details");
  await page.mouse.move(400, 300);
  await details.waitFor({ state: "detached" });
  await say("context 82 now");
  await wheel.locator("text=82%").waitFor();
  check("it follows Claude as the window fills, turning orange from 75%", (await arc()).stroke === "var(--s-needs)" && (await session(editId)).context.used_tokens === 164000);
  await shot(page.locator("textarea[data-composer]").locator("xpath=ancestor::div[contains(@class,'max-w')]"), "context-wheel-82");
  await say("context 95 now");
  await wheel.locator("text=95%").waitFor();
  check("…and red from 90%", (await arc()).stroke === "var(--s-error)");
  await shot(page.locator("textarea[data-composer]").locator("xpath=ancestor::div[contains(@class,'max-w')]"), "context-wheel-95");
  check("the properties panel shows the same figures", (await page.locator('aside[aria-label="Properties"]').innerText()).replace(/\s+/g, " ").includes("95% of context · 190k / 200k"));

  // ================================================================ the plan's limits
  const footer = page.locator("[data-plan-limits]");
  check("no limits are shown before Claude has reported any", (await footer.count()) === 0);
  await say("limits 46 10 please");
  await footer.waitFor({ timeout: 8000 });
  const limitsText = async () => (await footer.innerText()).replace(/\s+/g, " ");
  check("both windows appear in the sidebar once a session reports them", /^5-hour limit 46% resets \S.* 7-day limit 10% resets \S/.test(await limitsText()), await limitsText());
  await shot(page.locator('aside[aria-label="Sidebar"]'), "limits-sidebar");
  await shot(footer, "limits");
  await say("limits 93 71 please");
  await footer.locator("text=93%").waitFor({ timeout: 8000 });
  const bars = () => footer.locator('[role="meter"] > div:first-child').evaluateAll((els) => els.map((el) => el.className));
  check("they update while the app is open, and a nearly used window turns red", (await bars())[0].includes("bg-s-error") && (await bars())[1].includes("bg-accent"), (await bars()).join(" | "));
  await shot(footer, "limits-high");
  await say("limits 100 71 please");
  await footer.locator("text=reached").waitFor({ timeout: 8000 });
  check("a used-up window says so", /^5-hour limit reached resets/.test(await limitsText()));
  await shot(page, "limits-reached");

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
