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
  await sleep(700); // a new request ignores clicks for its first moment (and pictures show it awake)
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
  await main.getByText("Protected: Dashboard, Billing, the admin page").waitFor();
  check("once no question is left open, the answers go to Claude: as chosen, several joined in Claude's order, the typed one last", (await main.innerText()).includes("Sign-in: Both"));
  const answered = main.locator("[data-answered]").last();
  const answeredText = await answered.innerText();
  check("the feed keeps a record of what was answered, and the panel is gone", (await card.count()) === 0 && (await main.locator("[data-asking]").count()) === 0 && answeredText.includes("You answered") && answeredText.includes("Sign-in") && answeredText.includes("Both") && answeredText.includes("Dashboard, Billing, the admin page"), answeredText.replace(/\s+/g, " "));
  await shot(page, "question-answered");

  // Out of order, by keyboard: the second question first, then an own answer to the first.
  await say("ask question once more");
  await card.waitFor();
  await sleep(700);
  await card.locator('button[aria-label="Next question"]').click();
  await card.getByText("2 of 2").waitFor();
  await page.keyboard.press("b");
  check("a letter key chooses its option", (await marked("Settings")) === "true");
  await page.keyboard.press("Enter");
  await card.getByText("1 of 2").waitFor();
  check("Enter moves on to the question still open", (await card.innerText()).includes("Which sign-in method"));
  await page.keyboard.press("d");
  check("the letter after the last option goes to the own-answer row", await own.evaluate((el) => el === document.activeElement));
  await page.keyboard.type("Magic links by email");
  await page.keyboard.press("Enter");
  await main.getByText("Sign-in: Magic links by email").waitFor();
  check("an answer typed by the user reaches Claude word for word, and questions can be answered in any order", (await main.innerText()).includes("Protected: Settings"));

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
  await sleep(700);
  await shot(card, "question-panel-preview");
  await option("Tabs").click();
  await main.getByText("Layout: Tabs").waitFor();
  check("with a single question, choosing an option sends it", (await card.count()) === 0);

  // ================================================================ a plan
  const planId = await start("make plan for the auth module", "plan");
  const plan = main.locator("[data-plan-card]");
  await plan.waitFor();
  const planBox = plan.locator("[data-approval-content]");
  check("a plan shows as a card of its own, formatted", (await plan.innerText()).includes("Claude's plan") && (await planBox.locator("h1").innerText()) === "Plan: split the auth module" && (await planBox.locator("ol > li").count()) === 4 && (await planBox.locator("table tr").count()) === 4);
  const choices = (await plan.locator("button").allInnerTexts()).map((t) => t.replace(/\s+/g, " ").trim());
  check("…and under it Claude Code's own question and three numbered choices, with all of the plan showing", (await plan.innerText()).includes("Would you like to proceed?") && JSON.stringify(choices) === JSON.stringify(["1 Yes, auto-accept edits", "2 Yes, manually approve edits", "3 No, keep planning"]), choices.join(" | "));
  const rows = await plan.locator("[data-plan-choices] > *").evaluateAll((els) => els.map((el) => Math.round(el.getBoundingClientRect().width)));
  check("the choices are one list of equal rows, the last with room to say what to change", new Set(rows).size === 1 && (await plan.locator("input").getAttribute("placeholder")) === "Tell Claude what to change", rows.join(" "));
  check("…and where Claude saved it", (await plan.innerText()).includes("Saved to .claude-plans") && (await plan.innerText()).includes("split-the-auth-module.md"));
  check("the session is in plan mode", (await session(planId)).permission_mode === "plan");
  // The mode sits under the message box, bottom left, where the key hints used to be.
  const mode = (label) => page.locator(`button[aria-label="Mode: ${label}"]`);
  const boxOf = () => page.locator("textarea[data-composer]").evaluate((el) => el.parentElement.getBoundingClientRect().toJSON());
  const [modeAt, messageBox] = [await mode("Plan only").boundingBox(), await boxOf()];
  check("the mode shows under the message box, bottom left", modeAt && modeAt.y >= messageBox.bottom && Math.abs(modeAt.x - messageBox.left) < 4, JSON.stringify(modeAt));
  check("…and the Enter and Shift+Enter hints are gone", (await page.locator("text=new line").count()) === 0);
  await sleep(700);
  await shot(page, "plan-presented");
  await shot(plan, "plan-card");

  await plan.locator("input").fill("Keep the old file until the tests pass");
  await shot(plan, "plan-card-feedback");
  await plan.locator("input").press("Enter");
  await main.locator("[data-plan-outcome]", { hasText: "Plan sent back" }).waitFor();
  await planBox.locator("text=Changed after your feedback").waitFor();
  check("“No, keep planning” with words sends them to Claude, which presents the plan again", (await planBox.innerText()).includes("Keep the old file until the tests pass") && (await main.locator("[data-plan-outcome]").first().innerText()).includes("“Keep the old file until the tests pass”"));
  check("…and the session is still planning", (await session(planId)).permission_mode === "plan");
  await sleep(700);
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
  await enabled(plan.locator("button", { hasText: "Yes, manually approve edits" }));
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
  const written = await card.locator("mark").allInnerTexts();
  check("invisible characters in a question and its options are written out", written.includes("⟨U+202E⟩") && written.includes("⟨U+200B⟩"), written.join(" "));
  await sleep(700);
  await shot(card, "question-panel-hostile");
  await option("No").click();
  await main.getByText("Answer: No").waitFor();
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
  check("it asks first, and says what stopping does", confirmText.includes("1 session is in the middle of a turn") && confirmText.includes("keeps its history") && confirmText.includes("hangar up"), confirmText);
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
