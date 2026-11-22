// End-to-end check, in a real browser against a real daemon, of the parts of the app that stand
// in for Claude Code's own dialogs and read-outs: answering a question, approving a plan, the
// changed lines of an edit, the context wheel, the plan limits, helper agents, rewind, and stopping hangar.
// Claude is the scriptable fake from crates/testkit, which sends what Claude Code sends (recorded
// from 2.1.325, the helper agents from 2.1.329).
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
// A dozen sessions are open at once here. The fake ones are tiny, so the budget is told so:
// otherwise, on a PC that is short of memory, it makes the later ones wait in the queue.
fs.writeFileSync(path.join(home, "config.toml"), ["[budget]", "reserve_mb = 64", "initial_estimate_mb = 8", ""].join("\n"));
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
  const card = page.locator("[data-question-panel]");
  await card.waitFor();
  const option = (text) => card.locator("[data-option]", { hasText: text });
  const onward = card.locator("[data-onward]");
  const own = card.locator("input");
  const marked = (text) => option(text).getAttribute("aria-checked");
  const boxNow = () => page.locator("textarea[data-composer]").evaluate((el) => el.parentElement.getBoundingClientRect().toJSON());
  await sleep(300); // it slides up into place
  const [panelAt, under] = [await card.boundingBox(), await boxNow()];
  const gap = under.top - (panelAt.y + panelAt.height);
  check("a question comes up as a panel on top of the message box, as wide as it", gap >= 0 && gap < 12 && Math.abs(panelAt.x - under.left) < 2 && Math.abs(panelAt.width - under.width) < 2, `gap ${Math.round(gap)} px`);
  check("the feed says where to answer it, and nothing offers Allow or Deny", (await main.locator("[data-asking]").innerText()).includes("Claude is asking 2 questions") && (await page.locator("button", { hasText: /^\s*(Allow|Deny)/ }).count()) === 0 && (await main.locator("text=Claude wants to use").count()) === 0);
  const firstText = await card.innerText();
  check("it shows one question at a time", firstText.includes("1 of 2") && firstText.includes("Which sign-in method should the app use?") && !firstText.includes("Which pages need"));
  check("…with lettered options, what each means, and a row for an own answer", /^A\s+Email and password\s+Simple to build/.test(await option("Email and password").innerText()) && (await card.locator("[data-option]").count()) === 3 && (await own.getAttribute("placeholder")) === "Something else…");
  check("nothing can be sent before the question has an answer", await onward.isDisabled());
  check("the session waits under Needs you", (await session(askId)).status === "needs_you" && (await page.locator('nav[aria-label="Main"] button', { hasText: "Needs you" }).innerText()).includes("1"));
  await shot(page, "question-asked");
  await shot(card, "question-panel-first");

  await option("Google").click();
  await card.getByText("2 of 2").waitFor();
  check("choosing an option answers the question and moves on to the next", (await card.innerText()).includes("Which pages need a signed-in user?") && (await card.innerText()).includes("Choose any that apply"));
  await option("Billing").click();
  await option("Dashboard").click();
  await own.fill("the admin page");
  check("a question that takes several stays until you move on; options and an own answer add up", (await card.innerText()).includes("2 of 2") && (await marked("Billing")) === "true" && (await marked("Dashboard")) === "true" && (await marked("Settings")) === "false" && (await card.locator('[aria-label="Your own answer"]').getAttribute("aria-checked")) === "true");
  check("with every question answered, the button sends", (await onward.innerText()).includes("Send answers") && (await enabled(onward)));
  await shot(card, "question-panel-second");
  // Any question can be opened again, in any order, and its answer changed.
  await card.locator('[role="tab"]').first().click();
  await card.getByText("1 of 2").waitFor();
  check("an earlier question opens again with its answer still marked", (await marked("Google")) === "true" && (await marked("Both")) === "false");
  await shot(card, "question-panel-back");
  await option("Both").click();
  await main.getByText("Which pages need a signed-in user? Dashboard, Billing, the admin page").waitFor();
  check("once no question is left open, the answers go to Claude: as chosen, several joined in Claude's order, the typed one last", (await main.innerText()).includes("Which sign-in method should the app use? Both"));
  const answered = main.locator("[data-answered]").last();
  const answeredText = (await answered.innerText()).replace(/\s+/g, " ");
  check("the feed keeps a record of each question and its answer, and the panel is gone", (await card.count()) === 0 && (await main.locator("[data-asking]").count()) === 0 && answeredText === "You answered Which sign-in method should the app use? Both Which pages need a signed-in user? Dashboard, Billing, the admin page", answeredText);
  check("the short tag Claude sends with each question is shown nowhere", !/Sign-in|Protected/.test(firstText) && !/Sign-in|Protected/.test(await main.innerText()));
  await shot(page, "question-answered");

  // Out of order, by keyboard: the second question first, then an own answer to the first.
  await say("ask question once more");
  await card.waitFor();
  await card.locator('button[aria-label="Next question"]').click();
  await card.getByText("2 of 2").waitFor();
  await page.keyboard.press("b");
  check("a letter key chooses its option", (await marked("Settings")) === "true");
  await page.keyboard.press("Enter");
  await card.getByText("1 of 2").waitFor();
  check("Enter moves on to the question still open", (await card.innerText()).includes("Which sign-in method"));
  await page.keyboard.press("d");
  // The box takes the keyboard once the key has been handled, which on a busy PC is not at once.
  for (let i = 0; i < 40 && !(await own.evaluate((el) => el === document.activeElement)); i++) await sleep(50);
  check("the letter after the last option goes to the own-answer row", await own.evaluate((el) => el === document.activeElement));
  await page.keyboard.type("Magic links by email");
  await page.keyboard.press("Enter");
  await main.getByText("Which sign-in method should the app use? Magic links by email").waitFor();
  check("an answer typed by the user reaches Claude word for word, and questions can be answered in any order", (await main.innerText()).includes("Which pages need a signed-in user? Settings"));

  // An answer the daemon must refuse: nothing chosen, or a question Claude did not ask.
  await say("ask question a third time");
  await card.waitFor();
  const waiting = (await session(askId)).pending[0];
  const empty = await api("POST", `/sessions/${askId}/permissions/${waiting.request_id}`, { behavior: "allow" });
  const forged = await api("POST", `/sessions/${askId}/permissions/${waiting.request_id}`, { behavior: "allow", answers: { "Run as administrator?": "Yes" } });
  check("the daemon refuses to send a question on unanswered, or with an answer to another question", empty.status === 400 && forged.status === 400 && (await session(askId)).pending.length === 1, `${empty.data.error} / ${forged.data.error}`);
  await card.locator("button", { hasText: "Chat about this" }).click();
  await main.locator("text=declined: The user wants to clarify these questions.").waitFor();
  check("“Chat about this” tells Claude to ask what needs clearing up, in Claude Code's own words", (await main.getByText("You chose to talk about Claude's 2 questions first").count()) === 1);

  // One question, whose options carry a preview.
  await say("ask preview of the settings page");
  await card.waitFor();
  check("one question reads as one, with no pager", (await card.innerText()).includes("Claude has a question") && (await card.locator('[role="tab"]').count()) === 0);
  const seen = () => card.locator("[data-preview] pre:not(.invisible)").innerText();
  await sleep(300); // it slides up into place
  const heightBefore = (await card.boundingBox()).height;
  check("an option's preview shows beside the options: the first one's to begin with", (await seen()).includes("| Profile |"));
  await option("Tabs").hover();
  check("…and the one under the pointer while it is there, without the panel changing size", (await seen()).includes("[ Profile ][ Billing ][ Team ]") && (await card.boundingBox()).height === heightBefore);
  await shot(card, "question-panel-preview");
  await option("Tabs").click();
  await main.getByText("Which layout should the settings page use? Tabs").waitFor();
  check("with a single question, choosing an option sends it", (await card.count()) === 0);

  // ================================================================ a plan
  const planId = await start("make plan for the auth module", "plan");
  const plan = main.locator("[data-plan-card]");
  await plan.waitFor();
  const planBox = plan.locator("[data-approval-content]");
  check("a plan shows as a card of its own, formatted", (await planBox.locator("h1").innerText()) === "Plan: split the auth module" && (await planBox.locator("ol > li").count()) === 4 && (await planBox.locator("table tr").count()) === 4);
  const planLines = (await plan.innerText()).split("\n").map((l) => l.trim());
  check("the card is worded as Claude Code's terminal is, line for line", planLines[0] === "Ready to code?" && planLines.includes("Here is Claude's plan:") && planLines.includes("Claude has written up a plan and is ready to execute. Would you like to proceed?"), planLines.slice(0, 2).join(" | "));
  const choices = (await plan.locator("button").allInnerTexts()).map((t) => t.replace(/\s+/g, " ").trim());
  check("…with its three numbered choices, and all of the plan showing", JSON.stringify(choices) === JSON.stringify(["1 Yes, auto-accept edits", "2 Yes, manually approve edits", "3 No, keep planning"]), choices.join(" | "));
  const rows = await plan.locator("[data-plan-choices] > *").evaluateAll((els) => els.map((el) => Math.round(el.getBoundingClientRect().width)));
  check("the choices are one list of equal rows, the last with room to say what to change", new Set(rows).size === 1 && (await plan.locator("input").getAttribute("placeholder")) === "Tell Claude what to change", rows.join(" "));
  check("…and, under the choices, where Claude saved it", /\.claude-plans.split-the-auth-module\.md$/.test(planLines.at(-1)), planLines.at(-1));
  check("the session is in plan mode", (await session(planId)).permission_mode === "plan");
  // The mode sits under the message box, bottom left, where the key hints used to be.
  const mode = (label) => page.locator(`button[aria-label="Mode: ${label}"]`);
  const boxOf = () => page.locator("textarea[data-composer]").evaluate((el) => el.parentElement.getBoundingClientRect().toJSON());
  const [modeAt, messageBox] = [await mode("Plan only").boundingBox(), await boxOf()];
  check("the mode shows under the message box, bottom left", modeAt && modeAt.y >= messageBox.bottom && Math.abs(modeAt.x - messageBox.left) < 4, JSON.stringify(modeAt));
  check("…and the Enter and Shift+Enter hints are gone", (await page.locator("text=new line").count()) === 0);
  await shot(page, "plan-presented");
  await shot(plan, "plan-card");

  // As in the terminal: the third choice does nothing until something is typed.
  await plan.locator("button", { hasText: "No, keep planning" }).click();
  await plan.locator("input").press("Enter");
  await sleep(400);
  check(
    "“No, keep planning” with nothing typed only puts the cursor in its box: nothing is sent, and the turn goes on waiting",
    (await plan.locator("input").evaluate((el) => el === document.activeElement)) && (await session(planId)).pending.length === 1 && (await main.locator("[data-plan-outcome]").count()) === 0,
  );
  const approveWith = plan.locator("[data-plan-approve-with]");
  check("…and nothing offers to approve with a note yet", (await approveWith.count()) === 0);
  await plan.locator("input").fill("Keep the old file until the tests pass");
  check("once something is typed, the row says what Claude Code's says under it", (await approveWith.innerText()) === "shift+tab to approve with this feedback");
  await shot(plan, "plan-card-feedback");
  await plan.locator("input").press("Enter");
  await main.locator("[data-plan-outcome]", { hasText: "Plan sent back" }).waitFor();
  await planBox.locator("text=Changed after your feedback").waitFor();
  check("“No, keep planning” with words sends them to Claude, which presents the plan again", (await planBox.innerText()).includes("Keep the old file until the tests pass") && (await main.locator("[data-plan-outcome]").first().innerText()).includes("“Keep the old file until the tests pass”"));
  check("…and the session is still planning", (await session(planId)).permission_mode === "plan");
  await shot(page, "plan-sent-back");

  await plan.locator("button", { hasText: "Yes, auto-accept edits" }).click();
  await main.locator("text=plan approved (mode acceptEdits), edit made").waitFor();
  check("approving with “auto-accept edits” continues in that mode, and the edit is made without asking", (await session(planId)).permission_mode === "acceptEdits" && (await main.locator("text=Claude wants to use Edit").count()) === 0);
  check("the mode under the message box follows", (await mode("Auto-accept edits").count()) === 1);
  check("the answered plan reads as approved, and how", (await main.locator("[data-plan-outcome]").last().innerText()).includes("Plan approved, auto-accepting edits"));
  await main.locator("[data-plan-outcome] button").last().click();
  check("…and opens to show the plan again", (await main.locator("[data-plan-outcome]").last().locator("h1").innerText()) === "Plan: split the auth module");
  await shot(page, "plan-approved");

  const manualId = await start("make plan and approve each edit", "plan");
  await plan.waitFor();
  // By keyboard, as in Claude Code's terminal: 2 is the second choice.
  await plan.locator("button", { hasText: "Yes, auto-accept edits" }).focus();
  await page.keyboard.press("2");
  await main.locator("text=Claude wants to use Edit").waitFor();
  check("approving with “manually approve edits” continues in ask-first mode, and the next edit asks", (await session(manualId)).permission_mode === "default" && (await main.locator("[data-plan-outcome]").innerText()).includes("asking before each edit"));
  await shot(page, "plan-approved-manual");
  await main.locator("button", { hasText: /^\s*Allow$/ }).click();
  await main.locator("text=plan approved (mode default), edit made").waitFor();
  await mode("Ask first").click();
  const modes = page.locator('[role="listbox"][aria-label="Mode"]');
  const [listAt, pillAt] = [await modes.boundingBox(), await mode("Ask first").boundingBox()];
  check("the mode opens upward into a list of the four modes", listAt.y + listAt.height <= pillAt.y && (await modes.locator('[role="option"]').count()) === 4);
  await shot(page, "mode-menu");
  await modes.locator('[role="option"]', { hasText: "Auto-accept edits" }).click();
  await mode("Auto-accept edits").waitFor();
  check("…and choosing one switches the session", (await session(manualId)).permission_mode === "acceptEdits");

  // Approved with a note. In the box, Shift+Tab approves with what was typed (auto-accept edits).
  const noteId = await start("make plan and approve with a note", "plan");
  await plan.waitFor();
  await plan.locator("input").fill("Skip step 4");
  await plan.locator("input").press("Shift+Tab");
  await main.locator("text=plan approved (mode acceptEdits), edit made, with your note: Skip step 4").waitFor();
  const noted = (await main.locator("[data-plan-outcome]").innerText()).replace(/\s+/g, " ");
  check("Shift+Tab in the box approves the plan with what was typed, and Claude reads it with the approval", (await session(noteId)).permission_mode === "acceptEdits" && noted.includes("Plan approved, auto-accepting edits, with your note: “Skip step 4”"), noted);
  const said = (await api("GET", `/sessions/${noteId}/events`)).data.filter((e) => e.kind === "user").map((e) => e.text);
  check("…the note is kept with the approval, not shown as a message of its own", said.length === 1, said.join(" | "));
  await shot(page, "plan-approved-with-note");

  // What is typed goes with whichever "Yes" is chosen, as in the terminal.
  const note2Id = await start("make plan and approve each edit with a note", "plan");
  await plan.waitFor();
  await plan.locator("input").fill("Use pnpm, not npm");
  await plan.locator("button", { hasText: "Yes, manually approve edits" }).click();
  await main.locator("text=Claude wants to use Edit").waitFor();
  await main.locator("button", { hasText: /^\s*Allow$/ }).click();
  await main.locator("text=plan approved (mode default), edit made, with your note: Use pnpm, not npm").waitFor();
  check("a “Yes” chosen with something typed carries it along too", (await session(note2Id)).permission_mode === "default" && (await main.locator("[data-plan-outcome]").innerText()).includes("asking before each edit, with your note"));

  // Stopping while a plan waits: the Stop button, as Esc in the terminal.
  const stopId = await start("make plan and stop to rethink", "plan");
  await plan.waitFor();
  const stop = page.locator("header button", { hasText: "Stop" });
  check("a session whose plan is waiting has the Stop button", (await stop.count()) === 1 && (await page.locator("text=stop the turn").count()) === 1);
  await stop.click();
  await main.locator("[data-plan-outcome]", { hasText: "Plan not approved: stopped" }).waitFor();
  await main.locator("text=/^Stopped/").waitFor();
  const stopped = await session(stopId);
  check("Stop while a plan waits ends the turn there, still in plan mode", stopped.status === "idle" && stopped.permission_mode === "plan" && (await stop.count()) === 0);
  check("a stopped turn is shown as stopped, not as failed", (await main.locator("text=The turn failed").count()) === 0 && stopped.preview === "Stopped");
  await shot(page, "plan-stopped");

  await start("long plan for every file", "plan");
  await plan.waitFor();
  const yes = plan.locator("button", { hasText: "Yes, auto-accept edits" });
  const cut = () => planBox.evaluate((el) => el.scrollHeight > el.clientHeight + 1);
  check("a plan longer than a screen is cut at a visible line, with a button to show the rest", (await cut()) && (await plan.locator("button", { hasText: "There is more below" }).count()) === 1);
  check("…and can be answered without opening the rest: the choices are live", (await enabled(yes)) && !(await plan.innerText()).includes("to approve it"));
  await shot(plan, "plan-card-long");
  await plan.locator("button", { hasText: "There is more below" }).click();
  check("…the button shows all of it", !(await cut()) && (await planBox.innerText()).includes("Delete the old"));
  await yes.click();
  await main.locator("text=plan approved (mode acceptEdits), edit made").waitFor();

  await start("long plan, approved unread", "plan");
  await plan.waitFor();
  await plan.locator("button", { hasText: "Yes, auto-accept edits" }).focus();
  await page.keyboard.press("1");
  await main.locator("text=plan approved (mode acceptEdits), edit made").waitFor();
  check("a long plan is approved by its number key without being opened", true);

  // ================================================================ content that tries to mislead
  await start("ask hostile question");
  await card.waitFor();
  const pwned = () => page.evaluate(() => window.pwned);
  check("markup in a question is shown as text and never runs", (await card.locator("b, img, script, a").count()) === 0 && (await card.innerText()).includes("<b>backup</b>") && (await card.innerText()).includes("<script>window.pwned = 1</script>") && (await pwned()) === undefined);
  const written = await card.locator("mark").allInnerTexts();
  check("invisible characters in a question and its options are written out", written.includes("⟨U+202E⟩") && written.includes("⟨U+200B⟩"), written.join(" "));
  await shot(card, "question-panel-hostile");
  await option("No").click();
  await main.getByText("Answer: No").waitFor();
  check("…and it can still be answered", true);

  await start("hostile plan please", "plan");
  await plan.waitFor();
  check("a plan cannot load an image, run a script or link to one", (await planBox.locator("img, script").count()) === 0 && (await planBox.locator('a[href^="javascript"]').count()) === 0 && (await pwned()) === undefined && external.length === 0, external.join(" "));
  check("a plan with invisible characters in it says so", (await plan.innerText()).includes("This plan has invisible characters in it."));
  check("a field of the request that the card has no place for is still shown", (await plan.innerText()).includes('Also set: allowedPrompts: [{"tool":"Bash","prompt":"delete the backups"}]'));
  await plan.locator("button", { hasText: "Show it as plain text" }).click();
  check("…and shows them written out on request", (await planBox.locator("mark").first().innerText()) === "⟨U+202E⟩" && (await planBox.innerText()).includes("[Approve here](javascript:window.pwned=3)"));
  await shot(plan, "plan-card-hostile");
  await plan.locator("button", { hasText: "Yes, auto-accept edits" }).focus();
  await page.keyboard.press("Escape");
  await main.locator("text=/^Stopped/").waitFor();
  check("Esc stops a turn whose plan is waiting, as in the terminal", (await main.locator("[data-plan-outcome]").innerText()).includes("Plan not approved: stopped"));

  // Claude going into plan mode by itself: Claude Code does not ask about it, and the mode follows.
  const enterId = await start("enter plan mode first");
  await plan.waitFor();
  const entered = main.getByText("Entered plan mode", { exact: true });
  check("Claude entering plan mode by itself shows in the feed in Claude Code's words, and is not asked about", (await entered.count()) === 1 && (await main.locator("text=Claude wants to use").count()) === 0);
  check("…and the session's mode follows", (await session(enterId)).permission_mode === "plan" && (await mode("Plan only").count()) === 1);
  await shot(page, "plan-entered");
  await page.keyboard.press("Escape");
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
  await say("context 55 now");
  await wheel.locator("text=55%").waitFor();
  check("the ring turns yellow once the window is half full", (await arc()).stroke === "var(--fill-1)");
  await shot(page.locator("textarea[data-composer]").locator("xpath=ancestor::div[contains(@class,'max-w')]"), "context-wheel-55");
  await say("context 82 now");
  await wheel.locator("text=82%").waitFor();
  check("it follows Claude as the window fills: orange from 70%", (await arc()).stroke === "var(--fill-2)" && (await session(editId)).context.used_tokens === 164000);
  await shot(page.locator("textarea[data-composer]").locator("xpath=ancestor::div[contains(@class,'max-w')]"), "context-wheel-82");
  await say("context 95 now");
  await wheel.locator("text=95%").waitFor();
  check("…and red at the end, from 95%", (await arc()).stroke === "var(--fill-4)");
  await shot(page.locator("textarea[data-composer]").locator("xpath=ancestor::div[contains(@class,'max-w')]"), "context-wheel-95");
  check("the properties panel shows the same figures", (await page.locator('aside[aria-label="Properties"]').innerText()).replace(/\s+/g, " ").includes("95% of context · 190k / 200k"));

  // ================================================================ the plan's limits
  const footer = page.locator("[data-plan-limits]");
  check("no limits are shown before Claude has reported any", (await footer.count()) === 0);
  await say("limits 46 10 please");
  await footer.waitFor({ timeout: 8000 });
  const limitsText = async () => (await footer.innerText()).replace(/\s+/g, " ");
  check("both windows appear in the sidebar once a session reports them, each on one line above its bar", /^5-hour limit resets \S.* 46% 7-day limit resets \S.* 10%$/.test(await limitsText()) && (await footer.boundingBox()).height < 64, `${await limitsText()} (${Math.round((await footer.boundingBox()).height)} px tall)`);
  await shot(page.locator('aside[aria-label="Sidebar"]'), "limits-sidebar");
  await shot(footer, "limits");
  await say("limits 93 71 please");
  await footer.locator("text=93%").waitFor({ timeout: 8000 });
  const bars = () => footer.locator('[role="meter"] > div:first-child').evaluateAll((els) => els.map((el) => el.style.background));
  check("they update while the app is open, and warm as they fill, in the wheel's steps", (await bars())[0] === "var(--fill-3)" && (await bars())[1] === "var(--fill-2)", (await bars()).join(" | "));
  await shot(footer, "limits-high");
  await say("limits 100 71 please");
  await footer.locator("text=100%").waitFor({ timeout: 8000 });
  check("a used-up window reads 100%, in red", /^5-hour limit resets \S.* 100% /.test(await limitsText()) && (await bars())[0] === "var(--fill-4)");
  await shot(page, "limits-reached");

  // ================================================================ helper agents
  // Claude hands work to a helper agent in the background. Its turn ends; the agent works on.
  const helpId = await start("helper start, then wait", "acceptEdits");
  const agent = main.locator("[data-agent]").first();
  const liveLines = agent.locator("[data-agent-live]");
  const flat = async (el) => (await el.innerText()).replace(/\s+/g, " ").trim();
  await main.locator('[data-agent-state="running"]').waitFor();
  await main.locator("[data-working]", { hasText: "agent" }).waitFor();
  check("with an agent at work after Claude's turn, the session is still working", (await session(helpId)).status === "working");
  check("…and the feed says what is still running", (await main.locator("[data-working]").innerText()) === "1 agent running in the background…");
  check("before its first step the row reads as in Claude Code: Initializing…", (await flat(liveLines)) === "Initializing…");
  check("…and is marked as in the background", (await agent.innerText()).includes("in background"));
  check("the launch text meant for Claude is not shown", !(await main.innerText()).includes("Async agent launched"));
  await shot(page, "agent-initializing");

  // Its steps show as they happen.
  await say("helper step");
  await liveLines.locator("text=In progress… · 1 tool use").waitFor();
  check("a step shows as it happens, with Claude Code's own count: a read is counted, as in Claude Code", (await flat(liveLines)) === "Reading 1 file… In progress… · 1 tool use · 19.5k tokens", await flat(liveLines));
  await say("helper step 2");
  await liveLines.locator("text=In progress… · 3 tool uses").waitFor();
  check("reads and searches in a row are one line, as in Claude Code", (await flat(liveLines)) === "Searching for 1 pattern, reading 2 files… In progress… · 3 tool uses · 22.5k tokens", await flat(liveLines));
  await say("helper step 2");
  await liveLines.locator("text=In progress… · 5 tool uses").waitFor();
  check(
    "an edit and a command are lines of their own",
    (await flat(liveLines)) === "Searched for 1 pattern, read 2 files Edit notes.txt Bash npm test In progress… · 5 tool uses · 25.5k tokens",
    await flat(liveLines),
  );
  await say("helper step 3");
  await liveLines.locator("text=In progress… · 8 tool uses").waitFor();
  check(
    "only the newest three lines show; the lines above them are counted",
    (await flat(liveLines)) === "Edit notes.txt Bash npm test Searching for 1 pattern, reading 2 files… +1 more tool use In progress… · 8 tool uses · 30k tokens",
    await flat(liveLines),
  );
  // By now the row is far up the feed. A strip at the foot keeps saying what the agent does.
  const strip = main.locator("[data-running-agent]");
  check(
    "at the foot of the feed, each agent in the background shows what it is doing now",
    (await strip.count()) === 1 && (await flat(strip)) === "Tidy the notes Searching for 1 pattern, reading 2 files… 8 tool uses · 30k tokens",
    await flat(strip.first()),
  );
  check("with only an agent at work, a message would be read at once, and the box says so", (await page.getAttribute("textarea[data-composer]", "placeholder")) === "Reply to Claude…");
  check("through all of it the session stayed working, with Claude's own turns in between", (await session(helpId)).status === "working" && (await main.locator("text=Noted.").count()) === 4);
  await shot(page, "agent-live");

  // Opened, the row shows everything the agent did.
  await agent.locator("button[aria-expanded]").first().click();
  const inside = agent.locator("[data-agent-steps]");
  await inside.waitFor();
  check("opened, the row shows the prompt", (await inside.innerText()).includes("Prompt") && (await inside.innerText()).includes("Tidy the notes. Report what you did."));
  check("…and every step, each of which opens like a step of Claude's own", (await inside.locator("button[aria-expanded]").count()) === 8);
  check("an edit the agent made shows its changed lines", (await inside.locator("[data-diff]").count()) === 1 && (await inside.locator('[data-diff] [data-sign="+"]').innerText()).includes("final"));
  await shot(page, "agent-opened");
  await agent.locator("button[aria-expanded]").first().click();

  // The agent reports. Claude Code starts a turn by itself in which Claude reads the report.
  await say("helper finish");
  await main.locator("text=The helper is done: Tidy the notes.").waitFor();
  await main.locator('[data-agent-state="completed"]').waitFor();
  check("an agent that has reported reads Done, with Claude Code's totals", (await agent.locator("[data-agent-outcome]").innerText()) === "Done (8 tool uses · 30k tokens · 10s)", await agent.locator("[data-agent-outcome]").innerText());
  const afterAgent = await session(helpId);
  check("…and only now is the session idle", afterAgent.status === "idle" && afterAgent.agents.length === 0 && (await main.locator("[data-running-agents]").count()) === 0, afterAgent.status);
  await agent.locator("button[aria-expanded]").first().click();
  check("opened, its report is under Response", (await flat(inside)).includes("Response Done: 8 steps. The notes are tidy."));
  await shot(page, "agent-done");

  // Two agents at once; one is stopped by itself, then Stop ends the other.
  const twoId = await start("two helpers at once");
  await main.locator('[data-agent-state="running"]').nth(1).waitFor();
  await main.locator("[data-working]", { hasText: "agents" }).waitFor();
  check("two agents at work are both counted", (await main.locator("[data-working]").innerText()) === "2 agents running in the background…");
  await say("helper step");
  await main.locator("[data-agent-live] >> text=In progress… · 1 tool use").nth(1).waitFor();
  await shot(page, "agents-two");
  await main.locator("[data-agent]").first().locator("[data-agent-stop]").click();
  await main.locator('[data-agent-state="stopped"]').waitFor();
  const oneLeft = await session(twoId);
  check(
    "one agent can be stopped by itself: it reads Stopped, the other works on",
    (await main.locator("[data-agent]").first().locator("[data-agent-outcome]").innerText()) === "Stopped" && oneLeft.status === "working" && oneLeft.agents.length === 1 && oneLeft.agents[0].description === "Check the tests",
  );
  check("a stopped agent has no Stop button left", (await main.locator("[data-agent-stop]").count()) === 1);
  await page.locator("header button", { hasText: "Stop" }).click();
  await main.locator('[data-agent-state="stopped"]').nth(1).waitFor();
  for (let i = 0; i < 40 && (await session(twoId)).status !== "idle"; i++) await sleep(50);
  check("Stop, with only agents running, stops them; the session is idle at once", (await session(twoId)).status === "idle" && (await main.locator('[data-agent-state="running"]').count()) === 0);

  // An agent in the foreground asks before its edit. The request says who asks.
  const fgId = await start("helper fg, asking first");
  await main.locator("[data-asking-agent]").waitFor();
  const asker = main.locator("div", { has: page.locator("[data-asking-agent]") }).last();
  check("a request from an agent says that an agent asks, and which one", (await flat(asker)).includes("An agent wants to use Edit") && (await main.locator("[data-asking-agent]").innerText()).includes("Tidy the notes"));
  check("the agent's row shows the step that waits", (await flat(main.locator("[data-agent-live]"))) === "Searched for 1 pattern, read 2 files Edit notes.txt In progress… · 4 tool uses · 24k tokens", await flat(main.locator("[data-agent-live]")));
  check("an agent in the foreground is not marked as in the background", !(await main.locator("[data-agent]").innerText()).includes("in background") && (await main.locator("[data-running-agents]").count()) === 0);
  await shot(page, "agent-asks");
  await main.locator("button", { hasText: /^\s*Allow$/ }).click();
  await main.locator("text=The helper tidied the notes.").waitFor();
  check("allowed, it runs to its end", (await main.locator("[data-agent-outcome]").innerText()) === "Done (5 tool uses · 25.5k tokens · 6s)" && (await session(fgId)).status === "idle");
  check("the report's frame, written for Claude, is not shown", !(await main.innerText()).includes("Subagent hand-back"));

  // An agent that fails says so.
  const failId = await start("helper start, to fail", "acceptEdits");
  await main.locator('[data-agent-state="running"]').waitFor();
  await say("helper fail");
  await main.locator('[data-agent-state="failed"]').waitFor();
  check("an agent that failed reads Failed", (await main.locator("[data-agent-outcome]").innerText()) === "Failed");
  // Claude then says so in a turn of its own; the session is idle after it.
  for (let i = 0; i < 200 && (await session(failId)).status !== "idle"; i++) await sleep(50);
  check("…and the session is idle once Claude has said so", (await session(failId)).status === "idle");

  // An Agent call that fails outright says why, in Claude Code's words.
  await start("helper refuse, no such agent");
  await main.locator('[data-agent-state="failed"]').waitFor();
  check("an Agent call that fails says why", (await main.locator("[data-agent-outcome]").innerText()) === "Failed: Agent type 'auditor' not found. Available agents: general-purpose, Explore, Plan", await main.locator("[data-agent-outcome]").innerText());
  check("…and names the kind of agent that was asked for", (await main.locator("[data-agent]").innerText()).includes("auditor"));

  // One that is at work when hangar stops (see "stopping hangar" below).
  await start("helper start, for the restart");
  await main.locator('[data-agent-state="running"]').waitFor();
  await say("helper step");
  await main.locator("[data-agent-live] >> text=In progress… · 1 tool use").waitFor();

  // ================================================================ rewind
  // Claude Code's /rewind: go back to before one of your messages. The words are its terminal's.
  const idle = async (id) => {
    for (let i = 0; i < 200 && (await session(id)).status !== "idle"; i++) await sleep(50);
    return (await session(id)).status === "idle";
  };
  const rwId = await start("change cart.ts first", "acceptEdits");
  await main.locator("text=changed cart.ts").waitFor();
  await say("change notes.md too");
  await main.locator("text=changed notes.md").waitFor();
  await say("recall, what do you remember?");
  await main.locator("text=/I remember: change cart/").waitFor();
  await idle(rwId);
  const dialog = page.locator("[data-rewind]");
  const composer = page.locator("textarea[data-composer]");
  const points = dialog.locator("[data-rewind-point]");
  const choiceTexts = async () => (await dialog.locator("[data-rewind-choice]").allInnerTexts()).map((t) => t.replace(/\s+/g, " ").trim());
  const chosenChoice = () => dialog.locator('[data-rewind-choice][aria-selected="true"]').getAttribute("data-rewind-choice");
  const rwLines = async () => flat(dialog.locator("[data-rewind-lines]"));
  const userMessages = () => main.locator("[data-rewind-to]").count();

  // Esc twice, with nothing typed, opens it on the list of your messages, as in Claude Code.
  await composer.focus();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await dialog.waitFor();
  check("Esc twice opens Rewind on the list of your messages", (await dialog.getAttribute("data-rewind")) === "list" && (await dialog.innerText()).includes("Restore the code and/or conversation to the point before…"));
  check(
    "the list has your messages oldest first, with what Claude edited for each, and ends in (current)",
    JSON.stringify((await points.allInnerTexts()).map((t) => t.replace(/\s+/g, " ").trim())) === JSON.stringify(["change cart.ts first Edited cart.ts", "change notes.md too Edited notes.md", "recall, what do you remember? No edits"]) &&
      (await dialog.locator("[data-rewind-current]").getAttribute("aria-selected")) === "true",
    (await points.allInnerTexts()).join(" / "),
  );
  await shot(dialog, "rewind-list");
  // Choosing "(current)" changes nothing.
  await page.keyboard.press("Enter");
  await dialog.waitFor({ state: "detached" });
  check("choosing (current) closes it with nothing changed", (await userMessages()) === 3 && (await composer.inputValue()) === "");

  // By keyboard to the second message. Picking it asks Claude Code what restoring the code would change.
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await dialog.waitFor();
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  check("the arrow keys move through the list", (await points.nth(1).getAttribute("aria-selected")) === "true");
  await page.keyboard.press("Enter");
  await dialog.locator("[data-rewind-choice]").first().waitFor();
  check(
    "a picked message shows with Claude Code's question",
    (await dialog.innerText()).includes("Restore to the point before you sent this message:") && (await dialog.locator("[data-rewind-message]").innerText()) === "change notes.md too",
  );
  check(
    "with code to restore, the choices are Claude Code's, in its order, starting on both",
    JSON.stringify(await choiceTexts()) === JSON.stringify(["1 Restore code and conversation", "2 Restore conversation", "3 Restore code", "4 Never mind"]) && (await chosenChoice()) === "both",
    (await choiceTexts()).join(" / "),
  );
  check("it says what the highlighted choice does, with Claude Code's own count", (await rwLines()) === "The conversation goes back to before this message. The code will be restored +1 -1 in notes.md.", await rwLines());
  check("…and warns as Claude Code does", (await dialog.innerText()).includes("Restored files go back whole, losing later changes by anyone. Bash changes to other files are not undone."));
  await shot(dialog, "rewind-choices");
  await page.keyboard.press("ArrowDown");
  check("Restore conversation leaves the code", (await chosenChoice()) === "conversation" && (await rwLines()) === "The conversation goes back to before this message. The code will be unchanged.", await rwLines());
  await page.keyboard.press("ArrowDown");
  check("Restore code leaves the conversation", (await chosenChoice()) === "code" && (await rwLines()) === "The conversation will be unchanged. The code will be restored +1 -1 in notes.md.", await rwLines());
  // Esc goes back to the list; picking again lands on the same choices.
  await page.keyboard.press("Escape");
  check("Esc on the choices goes back to the list", (await dialog.getAttribute("data-rewind")) === "list");
  await points.nth(1).click();
  await dialog.locator('[data-rewind-choice="code"]').waitFor();

  // Restore code, by its number: the files go back, the conversation stays.
  await page.keyboard.press("3");
  await dialog.waitFor({ state: "detached" });
  await main.locator("[data-rewound]").waitFor();
  check(
    "Restore code puts the files back and says so in the feed; the conversation is as it was",
    (await flat(main.locator("[data-rewound]"))) === "Code restored to before “change notes.md too” +1 -1 in notes.md" && (await userMessages()) === 3 && (await composer.inputValue()) === "",
    await flat(main.locator("[data-rewound]")),
  );

  // The same message again, from its own Rewind button. There is no code left to restore.
  await main.locator("[data-rewind-to]").nth(1).click();
  await dialog.locator("[data-rewind-choice]").first().waitFor();
  check(
    "with nothing to restore in the files, only the conversation is offered",
    JSON.stringify(await choiceTexts()) === JSON.stringify(["1 Restore conversation", "2 Never mind"]) &&
      (await dialog.innerText()).includes("Restore the conversation to the point before you sent this message:") &&
      (await rwLines()) === "The conversation goes back to before this message. The code has not changed (nothing will be restored).",
    `${(await choiceTexts()).join(" / ")} | ${await rwLines()}`,
  );
  await shot(dialog, "rewind-conversation-only");
  // Restore conversation: the message and all after it are gone, and its text is back in the box.
  await page.keyboard.press("Enter");
  await dialog.waitFor({ state: "detached" });
  for (let i = 0; i < 40 && (await composer.inputValue()) === ""; i++) await sleep(50);
  check("Restore conversation puts the message back in the message box, to change and send again", (await composer.inputValue()) === "change notes.md too");
  check(
    "…and removes it and all after it from the feed",
    (await userMessages()) === 1 && (await main.locator("text=/I remember: change cart/").count()) === 0 && (await flat(main.locator("[data-rewound]"))) === "Conversation restored to before “change notes.md too”",
    await flat(main.locator("[data-rewound]").first()),
  );
  await shot(page, "rewind-done");
  await say("recall again");
  await main.locator("text=/I remember: /").waitFor();
  check("Claude no longer has the messages that were taken back", (await main.locator("text=I remember: change cart.ts first | recall again").count()) === 1);
  await idle(rwId);

  // The feed is the same after a reload: what was taken back stays gone.
  await page.reload();
  await page.click(`${sessions} >> text=change cart.ts first`);
  await main.locator("[data-rewound]").waitFor();
  check("after a reload, what was taken back is still gone", (await userMessages()) === 2 && (await main.locator("text=changed notes.md").count()) === 0);

  // A restore Claude Code cannot do fully: its own words are shown, and what did work is kept.
  await say("change locked.txt please");
  await main.locator("text=changed locked.txt").waitFor();
  await idle(rwId);
  await main.locator("[data-rewind-to]").last().click();
  await dialog.locator('[data-rewind-choice="both"]').waitFor();
  await dialog.locator('[data-rewind-choice="both"]').click();
  await dialog.locator("[data-rewind-problems]").waitFor();
  check(
    "when the code cannot be restored, Claude Code's reason is shown",
    (await flat(dialog.locator("[data-rewind-problems]"))) === "Failed to restore the code: Failed to rewind: No files were restored: 1 file(s) failed (backup missing, or the file could not be updated)",
    await flat(dialog.locator("[data-rewind-problems]")),
  );
  await shot(dialog, "rewind-problem");
  await dialog.locator("button", { hasText: "Close" }).click();
  check("…and the conversation still went back, as in Claude Code", (await composer.inputValue()) === "change locked.txt please" && (await main.locator("text=changed locked.txt").count()) === 0);
  // With text typed, Esc twice clears the box instead of opening Rewind, as in Claude Code.
  await composer.focus();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await sleep(200);
  check("with text typed, Esc twice clears it and does not open Rewind", (await composer.inputValue()) === "" && (await page.locator("[data-rewind]").count()) === 0);

  // From the command palette too, under Claude Code's description of it.
  await page.keyboard.press("Control+k");
  await page.keyboard.type("rewind");
  const rewindItem = page.locator('[role="option"]', { hasText: "Rewind" });
  check("Rewind is in the command palette", (await rewindItem.count()) === 1 && (await rewindItem.innerText()).includes("restore earlier code, conversation, or both"));
  await page.keyboard.press("Enter");
  await dialog.waitFor();
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "detached" });
  check("Esc on the list closes it", (await page.locator("[data-rewind]").count()) === 0);

  // A stopped session: Claude Code is started on the same conversation to be asked. Nothing is sent.
  const pausedId = await start("change paused.ts here", "acceptEdits");
  await main.locator("text=changed paused.ts").waitFor();
  await idle(pausedId);
  await api("POST", `/sessions/${pausedId}/stop`);
  for (let i = 0; i < 100 && (await session(pausedId)).status !== "stopped"; i++) await sleep(50);
  await main.locator("[data-rewind-to]").first().click();
  await dialog.locator('[data-rewind-choice="both"]').waitFor();
  const woken = await session(pausedId);
  check(
    "rewinding a stopped session starts it again to ask Claude Code, without sending anything",
    (await rwLines()) === "The conversation goes back to before this message. The code will be restored +1 -1 in paused.ts." && woken.pid != null && (await userMessages()) === 1,
    `${await rwLines()} | ${woken.status}`,
  );
  // The dialog keeps the keyboard: Ctrl K does not open the palette over it.
  await page.keyboard.press("Control+k");
  check("while Rewind is open, Ctrl K does not take over", (await page.locator('[role="option"]', { hasText: "Stop hangar" }).count()) === 0 && (await dialog.count()) === 1);
  // Never mind reached with Tab while "both" is highlighted: Enter does what has the focus.
  const rewindsBefore = await main.locator("[data-rewound]").count();
  check("…and \"both\" is the highlighted line", (await dialog.locator('[data-rewind-choice][aria-selected="true"]').getAttribute("data-rewind-choice")) === "both");
  await dialog.locator('[data-rewind-choice="nevermind"]').focus();
  await page.keyboard.press("Enter");
  await dialog.waitFor({ state: "detached" });
  check(
    "Enter on Never mind, reached with Tab, closes it and restores nothing; the session is idle",
    (await idle(pausedId)) && (await composer.inputValue()) === "" && (await main.locator("[data-rewound]").count()) === rewindsBefore && (await main.locator("text=changed paused.ts").count()) === 1,
  );

  // A turn is running (its request waits). A restore stops it first, as Claude Code does.
  await say("use tool and then rewind");
  await main.locator("text=Claude wants to use Bash").waitFor();
  await main.locator("[data-rewind-to]").last().click();
  await dialog.locator('[data-rewind-choice="conversation"]').waitFor();
  check("with a turn running, the dialog says that it is stopped first", (await dialog.locator("[data-rewind-stops]").innerText()) === "The turn that is running is stopped first.");
  await dialog.locator('[data-rewind-choice="conversation"]').click();
  await dialog.waitFor({ state: "detached" });
  for (let i = 0; i < 40 && (await composer.inputValue()) === ""; i++) await sleep(50);
  const stoppedTurn = await session(pausedId);
  check(
    "…the turn is stopped, its request is over, and the message is back in the box",
    stoppedTurn.pending.length === 0 && (await idle(pausedId)) && (await composer.inputValue()) === "use tool and then rewind" && (await main.locator("button", { hasText: /^\s*Allow$/ }).count()) === 0,
    `${stoppedTurn.status}, ${stoppedTurn.pending.length} pending`,
  );
  await composer.fill("");

  // A message typed while a turn runs is taken into that turn by Claude Code, which keeps no file
  // checkpoint for it. The list marks it as Claude Code's does; its conversation can still go back.
  const midId = await start("use tool, then type more", "default");
  await main.locator("text=Claude wants to use Bash").waitFor();
  await say("hello, one more thing");
  await main.locator("button", { hasText: /^\s*Allow$/ }).first().click();
  await idle(midId);
  await composer.focus();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await dialog.waitFor();
  check("a message typed during a turn is marked as Claude Code marks it", (await flat(dialog.locator("[data-rewind-point]").last())) === "hello, one more thing ⚠ No code restore", await flat(dialog.locator("[data-rewind-point]").last()));
  await dialog.locator("[data-rewind-point]").last().click();
  await dialog.locator("[data-rewind-choice]").first().waitFor();
  check(
    "picked, it offers only the conversation, with Claude Code's reason for the code",
    JSON.stringify((await dialog.locator("[data-rewind-choice]").allInnerTexts()).map((x) => x.replace(/\s+/g, " ").trim())) === JSON.stringify(["1 Restore conversation", "2 Never mind"]) &&
      (await flat(dialog.locator("[data-rewind-no-code]"))) === "The code cannot be restored: No file checkpoint found for this message.",
  );
  const quick = Date.now();
  await page.keyboard.press("Enter");
  await dialog.waitFor({ state: "detached" });
  for (let i = 0; i < 40 && (await composer.inputValue()) === ""; i++) await sleep(50);
  check("…and going back to before it is quick and puts it back in the box", (await composer.inputValue()) === "hello, one more thing" && Date.now() - quick < 3000, `${Date.now() - quick} ms`);
  await composer.fill("");

  // ================================================================ stopping hangar
  const other = await ctx.newPage(); // a second window on the same daemon
  watch(other);
  await other.goto(up());
  await other.waitForSelector(`${sessions} >> text=edit files in the auth module`);
  await start("use tool and wait for me");
  await main.locator("text=Claude wants to use Bash").waitFor();
  await page.keyboard.press("Control+k");
  await page.fill('input[aria-label="Command or session"]', "stop hangar");
  const item = page.locator('[role="option"]', { hasText: "Stop hangar" });
  check("“Stop hangar” is in the command palette", (await item.count()) === 1);
  await shot(page, "stop-in-palette");
  await page.keyboard.press("Enter");
  const confirm = page.locator('[role="dialog"]', { hasText: "Stop hangar?" });
  await confirm.waitFor();
  const confirmText = (await confirm.innerText()).replace(/\s+/g, " ");
  // Two sessions are at work: the one whose request waits, and the one whose agent is running.
  check("it asks first, and says what stopping does", confirmText.includes("2 sessions are in the middle of a turn") && confirmText.includes("keeps its history") && confirmText.includes("hangar up"), (await api("GET", "/state")).data.sessions.filter((s) => ["working", "starting", "needs_you"].includes(s.status)).map((s) => `${s.title}: ${s.status}`).join("; "));
  await shot(page, "stop-confirm");
  await page.keyboard.press("Escape");
  await confirm.waitFor({ state: "detached" });
  check("Escape cancels, and hangar keeps running", (await api("GET", "/state")).status === 200);
  await page.keyboard.press("Control+k");
  await page.fill('input[aria-label="Command or session"]', "stop hangar");
  await page.keyboard.press("Enter");
  await confirm.waitFor();
  stopping = true;
  await confirm.locator("button", { hasText: "Stop hangar" }).click();
  await page.waitForSelector("[data-stopped]");
  check("the window says hangar is stopped, and how to start it again", (await page.locator("[data-stopped]").innerText()).includes("hangar is stopped") && (await page.locator("[data-stopped]").innerText()).includes("hangar up"));
  await shot(page, "stopped");
  await other.waitForSelector("[data-stopped]", { timeout: 8000 });
  check("every other open window is told too, instead of “lost the daemon”", (await other.locator("text=Lost the daemon").count()) === 0);
  for (let i = 0; i < 50 && fs.existsSync(path.join(home, "daemon.json")); i++) await sleep(100);
  check("the daemon really stopped", !fs.existsSync(path.join(home, "daemon.json")));

  // Starting again: the same sessions, paused, with what was answered and measured still there.
  const again = up();
  await page.waitForSelector("text=Open hangar from your terminal", { timeout: 20000 });
  await page.goto(again);
  await page.waitForSelector(`${sessions} >> text=ask question about signing in`);
  const back = (await api("GET", "/state")).data;
  const paused = back.sessions.find((s) => s.id === editId);
  check("after `hangar up` the sessions are back, paused", ["hibernated", "stopped"].includes(paused.status), paused.status);
  check("…with the context figure and the limits Claude last reported", paused.context.percent === 95 && back.limits.windows[0].used === 1);
  await page.click(`${sessions} >> text=ask question about signing in`);
  await main.locator("[data-answered]").first().waitFor();
  check("…and the answers given before", (await main.locator("[data-answered]").first().innerText()).includes("Dashboard, Billing, the admin page"));
  await page.click(`${sessions} >> text=use tool and wait for me`);
  await main.locator("text=/^(Expired|Withdrawn) Bash/").waitFor();
  check("a request that was waiting when hangar stopped is over, not left answerable", (await main.locator("button", { hasText: /^\s*Allow$/ }).count()) === 0);
  await shot(page, "after-restart");
  await page.click(`${sessions} >> text=helper start, for the restart`);
  await main.locator("[data-agent]").waitFor();
  check(
    "an agent that was at work when hangar stopped reads as not finished, and not as running",
    (await main.locator("[data-agent]").getAttribute("data-agent-state")) === "unfinished" && (await main.locator("[data-agent-outcome]").innerText()) === "Did not finish" && (await main.locator("[data-agent-stop]").count()) === 0,
  );

  // A daemon that dies without saying so is still "lost", not "stopped".
  const pid = JSON.parse(fs.readFileSync(path.join(home, "daemon.json"), "utf8")).pid;
  process.kill(pid);
  await page.waitForSelector("text=Lost the daemon", { timeout: 15000 });
  check("a daemon that dies by itself still shows as lost, not as stopped", (await page.locator("[data-stopped]").count()) === 0);
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
