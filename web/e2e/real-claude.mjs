// Real Claude Code sessions (Haiku) driven through hangar's HTTP API:
// permission prompt, approval, per-turn cost, stop, resume with the
// conversation intact; then a question answered, a plan sent back and
// approved with a note, Claude entering plan mode by itself, an edit's
// changed lines, the context and limit figures checked against Claude's
// own, a helper agent in the background and in the foreground, Claude
// Code's rewind (preview, code, conversation, both), and three harder
// cases: after a summary, an agent inside an agent, and a restore while an
// agent edits. Costs a few cents.
//
//   cargo build --release && cd web && npm run e2e:claude
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "../..");
const hangar = path.join(root, "target", process.env.E2E_PROFILE ?? "release", "hangar" + (process.platform === "win32" ? ".exe" : ""));
if (!fs.existsSync(hangar)) throw new Error(`${hangar} is missing: run cargo build --release first`);
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "hangar-claude-e2e-"));
const home = path.join(tmp, "home");
const proj = path.join(tmp, "proj");
fs.mkdirSync(home);
fs.mkdirSync(proj);
fs.writeFileSync(path.join(proj, "README.md"), "# scratch project for hangar's end-to-end check\n");
// HANGAR_UI_PORT=0: any free port. Without it this hangar would take 7317, the usual hangar's
// port, whenever that one is stopped.
const env = { ...process.env, HANGAR_HOME: home, HANGAR_UI_PORT: "0" };
// Real sessions take about half a gigabyte each. On a PC that is short of free memory the default
// reserve would hold them in the queue; the memory budget itself is tested elsewhere.
fs.writeFileSync(path.join(home, "config.toml"), ["[budget]", "reserve_mb = 512", "initial_estimate_mb = 400", ""].join(String.fromCharCode(10)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push(ok);
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};

// However this script ends (a check that throws, a timeout), its daemon is stopped: one left
// running keeps its sessions alive and its program file locked for the next build.
process.on("exit", () => {
  try {
    execFileSync(hangar, ["stop"], { env, stdio: "ignore" });
  } catch {}
});
const url = execFileSync(hangar, ["up", "--no-open"], { env, encoding: "utf8" }).trim().split(/\r?\n/).pop();
const base = new URL(url).origin;
const code = url.split("#pair=")[1];
const { token } = await (await fetch(`${base}/api/pair`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ code }) })).json();
const api = async (method, path, body) => {
  const res = await fetch(`${base}/api${path}`, { method, headers: { "x-hangar-token": token, "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${JSON.stringify(data)}`);
  return data;
};
const session = async (id) => (await api("GET", "/state")).sessions.find((s) => s.id === id);
async function waitFor(id, pred, what, ms = 180000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const s = await session(id);
    if (s && pred(s)) return s;
    await sleep(500);
  }
  throw new Error(`timed out waiting for ${what}: ${JSON.stringify(await session(id))}`);
}

const created = await api("POST", "/sessions", {
  cwd: proj,
  prompt: "Use the Bash tool to run exactly this command: echo hangar-ok-4217 > hangar-check.txt  Then tell me in one short sentence what you did.",
  model: "haiku",
  permission_mode: "default",
  trust: true,
});
const id = created.id;
check("session created", !!id);

const asking = await waitFor(id, (s) => s.status === "needs_you" && s.pending.length > 0, "a permission request");
const req = asking.pending[0];
check("Claude asks before running Bash", req.tool_name === "Bash", `${req.tool_name}: ${JSON.stringify(req.input).slice(0, 80)}`);
check("the command shown is the one Claude will run", String(req.input.command ?? "").includes("hangar-check.txt"), String(req.input.command));
await api("POST", `/sessions/${id}/permissions/${encodeURIComponent(req.request_id)}`, { behavior: "allow" });

const done = await waitFor(id, (s) => s.status === "idle", "the turn to finish");
const events = await api("GET", `/sessions/${id}/events`);
const written = fs.existsSync(path.join(proj, "hangar-check.txt")) ? fs.readFileSync(path.join(proj, "hangar-check.txt"), "utf8") : "";
check("the approved command really ran", written.includes("hangar-ok-4217"), JSON.stringify(written.trim()));
const result = events.find((e) => e.kind === "claude" && e.msg.type === "result");
check("the turn's cost is recorded", result?.msg.turn_cost_usd > 0 && Math.abs(done.cost_usd - result.msg.turn_cost_usd) < 1e-9, `$${result?.msg.turn_cost_usd}`);
check("Claude reports what it did", /hangar-check|hangar-ok/i.test(result?.msg.result ?? ""), (result?.msg.result ?? "").slice(0, 80));
check("MCP status is tracked", Array.isArray(done.mcp));
check("memory is measured", done.memory_bytes > 0, `${Math.round(done.memory_bytes / 1048576)} MB`);

await api("POST", `/sessions/${id}/stop`);
const stopped = await waitFor(id, (s) => s.pid == null && (s.status === "stopped" || s.status === "hibernated"), "the process to stop", 30000);
check("stopping ends the process but keeps the conversation", !!stopped.claude_session_id, stopped.status);

await api("POST", `/sessions/${id}/messages`, { text: "What exact text did you write into the file? Reply with only that text, nothing else." });
let after = [];
let results2 = [];
for (const end = Date.now() + 180000; Date.now() < end; await sleep(500)) {
  after = await api("GET", `/sessions/${id}/events`);
  results2 = after.filter((e) => e.kind === "claude" && e.msg.type === "result");
  if (results2.length >= 2 && (await session(id)).status === "idle") break;
}
check("a stopped session starts again with --resume", after.some((e) => e.kind === "claude" && e.msg.type === "system" && e.msg.subtype === "init" && e.msg.session_id === stopped.claude_session_id) && results2.length >= 2);
const last = results2.at(-1)?.msg;
check("a resumed session remembers the earlier turn", (last?.result ?? "").includes("hangar-ok-4217"), (last?.result ?? "").slice(0, 80));
check("the resumed turn is billed on its own", last?.turn_cost_usd > 0 && last.turn_cost_usd < last.total_cost_usd + 1e-9, `$${last?.turn_cost_usd} of $${last?.total_cost_usd} process total`);
const final = await session(id);
check("the session total adds the turns up", Math.abs(final.cost_usd - results2.reduce((n, r) => n + r.msg.turn_cost_usd, 0)) < 1e-9, `$${final.cost_usd.toFixed(4)}`);

// ---------------------------------------------------------------- a question
const decide = (sid, request, body) => api("POST", `/sessions/${sid}/permissions/${encodeURIComponent(request)}`, body);
const eventsOf = (sid) => api("GET", `/sessions/${sid}/events`);
/** What Claude was told about its tool calls, oldest first. */
const toolResults = (events) =>
  events
    .filter((e) => e.kind === "claude" && e.msg.type === "user" && Array.isArray(e.msg.message?.content))
    .flatMap((e) => e.msg.message.content.filter((b) => b.type === "tool_result").map((b) => (typeof b.content === "string" ? b.content : JSON.stringify(b.content))));
const lastResult = (events) => events.filter((e) => e.kind === "claude" && e.msg.type === "result").at(-1)?.msg;

const asked = await api("POST", "/sessions", {
  cwd: proj,
  prompt:
    "Use the AskUserQuestion tool once, with two questions. Question 1 (single choice, header 'Colour'): 'Which colour do you prefer?' with options Red, Green, Blue. " +
    "Question 2 (multiSelect true, header 'Fruit'): 'Which fruits do you like?' with options Apple, Banana, Cherry. Do not ask anything in plain text. " +
    "After I answer, reply with one line: ANSWERS: followed by exactly what the tool told you I answered.",
  model: "haiku",
  trust: true,
});
const question = await waitFor(asked.id, (s) => s.status === "needs_you" && s.pending[0]?.tool_name === "AskUserQuestion", "Claude's question");
const qs = question.pending[0].input.questions;
check("Claude's question arrives as a request with its questions and options", qs.length === 2 && qs[0].options.length === 3 && qs[1].multiSelect === true, JSON.stringify(qs.map((q) => q.question)));
const unanswered = await fetch(`${base}/api/sessions/${asked.id}/permissions/${encodeURIComponent(question.pending[0].request_id)}`, {
  method: "POST",
  headers: { "x-hangar-token": token, "content-type": "application/json" },
  body: JSON.stringify({ behavior: "allow" }),
});
check("hangar will not send a question on unanswered", unanswered.status === 400);
await decide(asked.id, question.pending[0].request_id, { behavior: "allow", answers: { [qs[0].question]: "Green", [qs[1].question]: "Apple, Cherry, and a mango I typed" } });
await waitFor(asked.id, (s) => s.status === "idle", "the answer to be used");
const askedEvents = await eventsOf(asked.id);
check("Claude is told the answers, the typed one word for word", toolResults(askedEvents).some((t) => t.includes('="Green"') && t.includes("and a mango I typed")), toolResults(askedEvents).at(-1)?.slice(0, 160));
check("…and goes on with them", /Green/.test(lastResult(askedEvents)?.result ?? "") && /mango I typed/.test(lastResult(askedEvents)?.result ?? ""), (lastResult(askedEvents)?.result ?? "").slice(0, 140));
check("the answers are kept in the session's history", askedEvents.find((e) => e.kind === "decision")?.detail?.answers?.[qs[0].question] === "Green");
await api("DELETE", `/sessions/${asked.id}`);

// ---------------------------------------------------------------- a plan
const work = path.join(tmp, "plan-proj");
fs.mkdirSync(work);
const planned = await api("POST", "/sessions", {
  cwd: work,
  prompt: "Make a short plan (3 steps at most) to create a file named hello.txt containing the word hi in this folder. Do not ask me questions. Present the plan with ExitPlanMode.",
  model: "haiku",
  permission_mode: "plan",
  trust: true,
});
const isPlan = (s) => s.status === "needs_you" && s.pending[0]?.tool_name === "ExitPlanMode";
const firstPlan = await waitFor(planned.id, isPlan, "Claude's plan");
check("Claude's plan arrives as a request that carries the plan", typeof firstPlan.pending[0].input.plan === "string" && /hello\.txt/.test(firstPlan.pending[0].input.plan), firstPlan.pending[0].input.plan.split("\n")[0]);
await decide(planned.id, firstPlan.pending[0].request_id, { behavior: "deny", feedback: "Also create a second file named bye.txt containing the word bye." });
const secondPlan = await waitFor(planned.id, (s) => isPlan(s) && s.pending[0].request_id !== firstPlan.pending[0].request_id, "the plan again");
check("sent back with what to change, Claude plans again with it", /bye\.txt/.test(secondPlan.pending[0].input.plan) && secondPlan.permission_mode === "plan", secondPlan.pending[0].input.plan.split("\n")[0]);
check("…and nothing was written while planning", !fs.existsSync(path.join(work, "hello.txt")));
// Approved with a note, which Claude must read with the approval: before its first step, not after it.
await decide(planned.id, secondPlan.pending[0].request_id, { behavior: "allow", mode: "acceptEdits", feedback: "Name the first file hello-b.txt, not hello.txt." });
// Auto-accept covers edits, not commands: Claude sometimes looks around with one first (a
// listing, say). That is allowed here; being asked about an edit is what must not happen.
const askedWhileBuilding = [];
let built;
for (const end = Date.now() + 180000; Date.now() < end; await sleep(500)) {
  built = await session(planned.id);
  if (built.status === "idle" || isPlan(built)) break;
  for (const p of built.pending) {
    askedWhileBuilding.push(p.tool_name);
    await decide(planned.id, p.request_id, { behavior: "allow" });
  }
}
const builtEvents = await eventsOf(planned.id);
const wrote = (name) => fs.existsSync(path.join(work, name)) && fs.readFileSync(path.join(work, name), "utf8").trim();
check(
  "approved with auto-accept, Claude carries the plan out without asking about the edits",
  built.status === "idle" && !askedWhileBuilding.some((tool) => ["Write", "Edit", "MultiEdit"].includes(tool)) && wrote("hello-b.txt") === "hi" && wrote("bye.txt") === "bye",
  `${built.status}; asked about: ${askedWhileBuilding.join(", ") || "nothing"}; hello-b.txt=${wrote("hello-b.txt")}; bye.txt=${wrote("bye.txt")}`,
);
const approvedAt = builtEvents.find((e) => e.kind === "decision" && e.behavior === "allow")?.seq ?? Infinity;
// What Claude did with the old name after the approval. Writing or creating hello.txt would mean
// the note was read too late. A command that only looks (a listing that names it) is not that.
const afterApproval = builtEvents
  .filter((e) => e.seq > approvedAt && e.kind === "claude" && e.msg.type === "assistant")
  .flatMap((e) => (e.msg.message?.content ?? []).filter((b) => b.type === "tool_use" && JSON.stringify(b.input).includes("hello.txt")));
const wroteOld = afterApproval.filter(
  (b) =>
    (["Write", "Edit", "MultiEdit"].includes(b.name) && /(^|[\\/])hello\.txt$/.test(String(b.input.file_path ?? ""))) ||
    (["Bash", "PowerShell"].includes(b.name) && /(>\s*|touch\s+|tee\s+|Set-Content\s+|Out-File\s+|New-Item\s+)["']?(\.[\\/])?hello\.txt/i.test(String(b.input.command ?? ""))),
);
check(
  "a note typed with the approval is read with it: Claude never writes the old name",
  wroteOld.length === 0 && !fs.existsSync(path.join(work, "hello.txt")),
  afterApproval.map((b) => `${b.name}: ${String(b.input.command ?? b.input.file_path ?? "").slice(0, 120)}`).join(" | ") || "the old name never came up",
);
check("…and the note is kept with the approval, not as a message", builtEvents.find((e) => e.seq === approvedAt)?.detail?.feedback === "Name the first file hello-b.txt, not hello.txt." && builtEvents.filter((e) => e.kind === "user").length === 1);
check("…and the session is in the mode that was chosen", built.permission_mode === "acceptEdits", built.permission_mode);
check("Claude is told the plan was approved, in its own terminal's words", toolResults(builtEvents).some((t) => t.startsWith("User has approved your plan")));

// ---------------------------------------------------------------- an edit, and the numbers
await api("POST", `/sessions/${planned.id}/messages`, { text: "Use the Edit tool to change the word hi in hello-b.txt to: hello there. Then reply with the single word DONE." });
await waitFor(planned.id, (s) => s.status === "idle" && wrote("hello-b.txt") === "hello there", "the edit");
const editEvents = await eventsOf(planned.id);
const patches = editEvents.filter((e) => e.kind === "claude" && e.msg.type === "user" && e.msg.tool_use_result?.structuredPatch?.length).map((e) => e.msg.tool_use_result);
const patch = patches.at(-1);
const patchLines = patch?.structuredPatch[0].lines ?? [];
check("an edit's changed lines are kept, with the line they start at", patch?.structuredPatch[0].oldStart === 1 && patchLines.includes("-hi") && patchLines.includes("+hello there"), JSON.stringify(patch).slice(0, 160));
check("…and nothing else of the file", patch && Object.keys(patch).every((k) => ["structuredPatch", "type", "truncated"].includes(k)), patch && Object.keys(patch).join(","));

// The context figure is Claude Code's own: it must equal the size of the prompt Claude last read.
const own = editEvents.filter((e) => e.kind === "claude" && e.msg.type === "assistant" && !e.msg.parent_tool_use_id && e.msg.message?.usage).at(-1).msg.message.usage;
const read = own.input_tokens + (own.cache_read_input_tokens ?? 0) + (own.cache_creation_input_tokens ?? 0);
let context;
for (const end = Date.now() + 10000; Date.now() < end; await sleep(250)) {
  context = (await session(planned.id)).context;
  if (context?.used_tokens === read) break;
}
check("the context figure is the size of the prompt Claude last read, to the token", context?.used_tokens === read, `hangar shows ${context?.used_tokens}; Claude's last reply read ${read}`);
check("…with Claude Code's own percentage and window", context?.max_tokens > 0 && Math.abs(context.percent - (100 * context.used_tokens) / context.max_tokens) <= 1 && context.parts.length > 0, `${context?.percent}% of ${context?.max_tokens}; ${context?.parts.map((p) => p.name).join(", ")}`);

const limits = (await api("GET", "/state")).limits;
const windows = Object.fromEntries((limits?.windows ?? []).map((w) => [w.name, w]));
const sane = (w) => w && w.used >= 0 && w.used <= 10 && w.resets_at * 1000 > Date.now();
check("the plan's 5-hour and 7-day limits are reported with their reset times", sane(windows.five_hour) && sane(windows.seven_day), JSON.stringify(limits?.windows.map((w) => `${w.name} ${Math.round(w.used * 100)}%`)));
await api("DELETE", `/sessions/${planned.id}`);

// ---------------------------------------------------------------- Claude enters plan mode by itself
const entering = await api("POST", "/sessions", {
  cwd: work,
  prompt: "Call the EnterPlanMode tool now, before anything else. Then make a one-step plan to create a file named later.txt, and present it with ExitPlanMode. Do not ask me questions.",
  model: "haiku",
  permission_mode: "default",
  trust: true,
});
const enteredPlan = await waitFor(entering.id, (s) => s.status === "needs_you" || s.status === "idle", "Claude to enter plan mode and present a plan");
const enteringEvents = await eventsOf(entering.id);
const askedAbout = enteringEvents.filter((e) => e.kind === "permission").map((e) => e.tool_name);
check("Claude Code does not ask before entering plan mode: the first request is the plan", askedAbout.length === 1 && askedAbout[0] === "ExitPlanMode", askedAbout.join(", "));
check("…and the session's mode follows by itself", enteredPlan.permission_mode === "plan" && toolResults(enteringEvents).some((t) => t.startsWith("Entered plan mode")), enteredPlan.permission_mode);
// Stopped while the plan waits, as the Stop button does.
await decide(entering.id, enteredPlan.pending[0].request_id, { behavior: "deny", stop: true });
const afterStop = await waitFor(entering.id, (s) => s.status === "idle", "the turn to stop", 60000);
check("stopping while a plan waits ends the turn, still in plan mode, with nothing written", afterStop.permission_mode === "plan" && afterStop.preview === "Stopped" && !fs.existsSync(path.join(work, "later.txt")), `${afterStop.permission_mode}; ${afterStop.preview}`);
await api("DELETE", `/sessions/${entering.id}`);

// ---------------------------------------------------------------- helper agents
// Claude hands work to a helper agent (its Agent tool) in the background. Its own turn ends
// while the agent works on: the session must stay "working", say which agent runs, name the
// agent on the request it makes, answer a message meanwhile, and let the agent be stopped.
const agentsDir = path.join(proj, "agents");
fs.mkdirSync(agentsDir);
const helped = await api("POST", "/sessions", {
  cwd: agentsDir,
  prompt:
    "Use the Agent tool once (general-purpose subagent, run_in_background: true). Tell the subagent to do exactly two steps in the current folder: " +
    "first create a file named made.txt containing the word hi with the Write tool, then run this one Bash command, which takes 40 seconds: node -e \"setTimeout(() => console.log('slept'), 40000)\" . " +
    "Do not do the work yourself. As soon as the agent is started, reply with one short sentence.",
  model: "haiku",
  permission_mode: "default",
  trust: true,
});
const agentAsks = await waitFor(helped.id, (s) => s.status === "needs_you" && s.pending.some((p) => p.agent_id), "the agent's request");
const agentRequest = agentAsks.pending.find((p) => p.agent_id);
const running = agentAsks.agents[0];
check("the agent Claude started is listed as running, in the background", agentAsks.agents.length === 1 && running?.background === true, JSON.stringify(agentAsks.agents));
check("a request made by the agent names the agent", agentRequest.tool_name === "Write" && agentRequest.agent_id === running?.id, `${agentRequest.tool_name} by ${agentRequest.agent_id}`);
await decide(helped.id, agentRequest.request_id, { behavior: "allow" });
// The second step may ask too (a command). Allow whatever the agent asks until it is asleep in it.
const turnsOf = (events) => events.filter((e) => e.kind === "claude" && e.msg.type === "result").length;
let atWork;
for (const end = Date.now() + 120000; Date.now() < end; await sleep(500)) {
  const s = await session(helped.id);
  for (const p of s.pending) await decide(helped.id, p.request_id, { behavior: "allow" });
  const steps = (await eventsOf(helped.id)).filter((e) => e.kind === "claude" && e.msg.type === "system" && e.msg.subtype === "task_progress");
  if (s.pending.length === 0 && !s.in_turn && s.agents.length === 1 && steps.some((e) => e.msg.last_tool_name === "Bash")) {
    atWork = s;
    break;
  }
}
const midEvents = await eventsOf(helped.id);
check("with Claude's turn over and the agent still at work, the session is working, not idle", atWork?.status === "working" && turnsOf(midEvents) >= 1, `${atWork?.status}, ${turnsOf(midEvents)} turn(s) ended`);
for (let i = 0; i < 20 && !fs.existsSync(path.join(agentsDir, "made.txt")); i++) await sleep(250);
check("the agent really did its first step", fs.existsSync(path.join(agentsDir, "made.txt")));
const insideAgent = midEvents.filter((e) => e.kind === "claude" && e.msg.parent_tool_use_id === running.tool_use_id);
const agentTools = insideAgent.flatMap((e) => (e.msg.type === "assistant" ? e.msg.message.content.filter((b) => b.type === "tool_use").map((b) => b.name) : []));
check("the agent's own steps are kept with the call that started it", agentTools.includes("Write") && agentTools.includes("Bash"), agentTools.join(", "));
const started = midEvents.find((e) => e.kind === "claude" && e.msg.subtype === "task_started")?.msg;
const progress = midEvents.filter((e) => e.kind === "claude" && e.msg.subtype === "task_progress").at(-1)?.msg;
check("Claude Code's account of the agent is kept, without the prompt", started?.task_id === running.id && started.tool_use_id === running.tool_use_id && !("prompt" in started), JSON.stringify(started));
check("…with its own count of tool uses and tokens", progress?.usage?.tool_uses >= 2 && progress.usage.total_tokens > 0 && typeof progress.description === "string", JSON.stringify(progress?.usage));
const launch = midEvents.find((e) => e.kind === "claude" && e.msg.type === "user" && e.msg.tool_use_result?.agentId)?.msg.tool_use_result;
check("of the Agent tool's result, only the launch facts are kept", launch?.agentId === running.id && launch.status === "async_launched" && Object.keys(launch).every((k) => ["agentId", "status"].includes(k)), JSON.stringify(launch));

// A message while only the agent works: Claude answers at once, and the agent works on.
const before = turnsOf(midEvents);
await api("POST", `/sessions/${helped.id}/messages`, { text: "Reply with exactly these two words and nothing else: still here" });
let answered;
for (const end = Date.now() + 60000; Date.now() < end; await sleep(400)) {
  const events = await eventsOf(helped.id);
  if (turnsOf(events) > before) {
    answered = { events, s: await session(helped.id) };
    break;
  }
}
check("a message sent while only the agent works is answered at once", /still here/i.test(lastResult(answered?.events ?? [])?.result ?? ""), (lastResult(answered?.events ?? [])?.result ?? "").slice(0, 60));
// (The agent may be asking for something at this moment: then the session needs you, with the agent still listed.)
check("…and the agent works on behind it: the session is not idle", ["working", "needs_you"].includes(answered?.s.status) && answered.s.agents.length === 1, `${answered?.s.status}, ${answered?.s.agents.length} agent(s)`);
for (const p of (await session(helped.id)).pending) await decide(helped.id, p.request_id, { behavior: "allow" });

// The agent is stopped by itself.
const stopAt = Date.now();
await api("POST", `/sessions/${helped.id}/agents/${encodeURIComponent(running.id)}/stop`);
// Claude Code then starts a turn by itself, in which Claude hears that the agent was stopped.
// The session is idle after that turn, and not for a moment before it.
const stoppedAgent = await waitFor(helped.id, (s) => s.agents.length === 0 && s.status === "idle", "the agent to stop", 60000);
const stopTook = Date.now() - stopAt;
const endEvents = await eventsOf(helped.id);
check("the session is idle only after the turn in which Claude hears of the stop", turnsOf(endEvents) === turnsOf(answered?.events ?? []) + 1, `${turnsOf(endEvents)} turns ended when it went idle`);
await sleep(3000);
const settled = await session(helped.id);
const ends = endEvents.filter((e) => e.kind === "claude" && (e.msg.subtype === "task_updated" || e.msg.subtype === "task_notification")).map((e) => `${e.msg.subtype}:${e.msg.status}`);
check("one agent can be stopped by itself; Claude Code reports it killed", stoppedAgent.agents.length === 0 && ends.includes("task_updated:killed"), `${ends.join(", ")} after ${stopTook} ms`);
check("…and the session is idle with nothing listed as running", settled.status === "idle" && settled.agents.length === 0 && !settled.in_turn, `${turnsOf(endEvents)} turns in all`);

// An agent in the foreground runs inside Claude's turn and reports its totals.
await api("POST", `/sessions/${helped.id}/messages`, {
  text: "Use the Agent tool once (general-purpose subagent, run_in_background: false) to read made.txt in the current folder and report its content. Do not read it yourself. Then tell me the content in one short sentence.",
});
await waitFor(helped.id, (s) => s.agents.length === 1, "the foreground agent to start", 60000).catch(() => null);
const fgDone = await waitFor(helped.id, (s) => s.status === "idle" && s.agents.length === 0, "the foreground agent's turn", 120000);
const fgEvents = await eventsOf(helped.id);
const totals = fgEvents.filter((e) => e.kind === "claude" && e.msg.type === "user" && e.msg.tool_use_result?.status === "completed" && e.msg.tool_use_result.agentId).at(-1)?.msg.tool_use_result;
const fgStart = fgEvents.filter((e) => e.kind === "claude" && e.msg.subtype === "task_started").at(-1)?.msg;
check("an agent in the foreground is announced as such", fgStart?.is_backgrounded === false, JSON.stringify(fgStart));
check(
  "when it has reported, Claude Code's totals for it are kept",
  totals?.totalToolUseCount >= 1 && totals.totalTokens > 0 && totals.totalDurationMs > 0 && Object.keys(totals).every((k) => ["agentId", "status", "totalDurationMs", "totalTokens", "totalToolUseCount"].includes(k)),
  JSON.stringify(totals),
);
check("…and Claude goes on with the agent's report", /hi/i.test(lastResult(fgEvents)?.result ?? "") && fgDone.status === "idle", (lastResult(fgEvents)?.result ?? "").slice(0, 80));
await api("DELETE", `/sessions/${helped.id}`);

// ---------------------------------------------------------------- rewind
// Claude Code's own rewind, asked for through hangar: a preview of what restoring the code would
// change, the code alone, the conversation alone, and both on a session that was stopped.
const rewindDir = path.join(proj, "rewind");
fs.mkdirSync(rewindDir);
fs.writeFileSync(path.join(rewindDir, "a.txt"), "alpha\n");
const files = () => Object.fromEntries(["a.txt", "b.txt", "c.txt"].map((n) => [n, fs.existsSync(path.join(rewindDir, n)) ? fs.readFileSync(path.join(rewindDir, n), "utf8").trim() : null]));
const rewind = (sid, seq, action) => api("POST", `/sessions/${sid}/rewind`, { seq, action });
const files_in = (dir) => (fs.existsSync(path.join(dir, "a.txt")) ? fs.readFileSync(path.join(dir, "a.txt"), "utf8").trim() : null);
const names = (list) => list.map((f) => path.basename(f)).sort();
const SECOND = "Use the Write tool to create b.txt containing the single line bee. Then run this exact Bash command: echo sea > c.txt . Then reply DONE.";
const rewound = await api("POST", "/sessions", {
  cwd: rewindDir,
  prompt: "Use the Edit tool to change the word alpha to ALPHA in a.txt. Then reply DONE.",
  model: "haiku",
  permission_mode: "bypassPermissions",
  trust: true,
});
const turnEnds = async (n) => {
  for (const end = Date.now() + 180000; Date.now() < end; await sleep(500)) {
    const events = await eventsOf(rewound.id);
    if (events.filter((e) => e.kind === "claude" && e.msg.type === "result").length >= n && (await session(rewound.id)).status === "idle") return events;
  }
  throw new Error(`turn ${n} of the rewind session did not end`);
};
await turnEnds(1);
await api("POST", `/sessions/${rewound.id}/messages`, { text: SECOND });
await turnEnds(2);
await api("POST", `/sessions/${rewound.id}/messages`, { text: "Remember the word CHERRY. Reply with the single word OK." });
const threeTurns = await turnEnds(3);
const sent = threeTurns.filter((e) => e.kind === "user");
check("every message went to Claude with an id it can be rewound to", sent.length === 3 && sent.every((e) => /^[0-9a-f-]{36}$/.test(e.uuid ?? "")));
check("three turns left the files as expected", JSON.stringify(files()) === JSON.stringify({ "a.txt": "ALPHA", "b.txt": "bee", "c.txt": "sea" }), JSON.stringify(files()));

// Previews: what Claude Code says restoring the code would change. Nothing changes.
const [seq1, seq2, seq3] = sent.map((e) => e.seq);
const previewNewest = await rewind(rewound.id, seq3, "preview");
const previewSecond = await rewind(rewound.id, seq2, "preview");
const previewFirst = await rewind(rewound.id, seq1, "preview");
check("a preview of the newest message finds nothing to restore", previewNewest.can_restore === true && previewNewest.files.length === 0, JSON.stringify(previewNewest));
check(
  "a preview names the files Claude's own edits changed since the message, with Claude Code's line counts",
  JSON.stringify(names(previewSecond.files)) === '["b.txt"]' && previewSecond.insertions === 0 && previewSecond.deletions === 1 && JSON.stringify(names(previewFirst.files)) === '["a.txt","b.txt"]',
  `${JSON.stringify(names(previewSecond.files))} +${previewSecond.insertions} -${previewSecond.deletions}; ${JSON.stringify(names(previewFirst.files))} +${previewFirst.insertions} -${previewFirst.deletions}`,
);
check("a preview changes nothing", JSON.stringify(files()) === JSON.stringify({ "a.txt": "ALPHA", "b.txt": "bee", "c.txt": "sea" }) && !(await eventsOf(rewound.id)).some((e) => e.kind === "rewind"));

// The code alone, to before the second message.
await rewind(rewound.id, seq2, "preview");
const codeOnly = await rewind(rewound.id, seq2, "code");
check("restoring the code answers that it went through", codeOnly.code?.ok === true && codeOnly.conversation === undefined, JSON.stringify(codeOnly));
check(
  "the file Claude created is gone, its edit from before stays, and what a command made is not undone",
  JSON.stringify(files()) === JSON.stringify({ "a.txt": "ALPHA", "b.txt": null, "c.txt": "sea" }),
  JSON.stringify(files()),
);
const codeRecord = (await eventsOf(rewound.id)).filter((e) => e.kind === "rewind").at(-1);
check("the restore is in the history, with what the preview said it would change", codeRecord?.to === seq2 && codeRecord.conversation === false && JSON.stringify(names(codeRecord.code?.files ?? [])) === '["b.txt"]', JSON.stringify(codeRecord));

// The conversation alone, to before the second message. A third came after it.
const convOnly = await rewind(rewound.id, seq2, "conversation");
check("restoring the conversation hands back the message's text for the message box", convOnly.conversation?.ok === true && convOnly.conversation.prefill === SECOND, JSON.stringify(convOnly).slice(0, 200));
await api("POST", `/sessions/${rewound.id}/messages`, {
  text: "Two short lines, from memory. Line 1: every word I asked you to remember so far, or NONE. Line 2: every file you created or edited so far in this conversation.",
});
const remembered = lastResult(await turnEnds(4))?.result ?? "";
check("Claude no longer has the messages that were taken back", /NONE/.test(remembered) && !/CHERRY/i.test(remembered) && !/b\.txt/.test(remembered) && /a\.txt/.test(remembered), remembered.slice(0, 160));
let goneRefused = false;
try {
  await rewind(rewound.id, seq3, "preview");
} catch (e) {
  goneRefused = /not in the conversation any more/.test(e.message);
}
check("a message that was taken back cannot be gone back to", goneRefused);

// Both, on a stopped session: Claude Code is started on the same conversation to be asked.
await api("POST", `/sessions/${rewound.id}/stop`);
await waitFor(rewound.id, (s) => s.status === "stopped", "the rewind session to stop", 30000);
const messagesBefore = (await eventsOf(rewound.id)).filter((e) => e.kind === "user").length;
const wakePreview = await rewind(rewound.id, seq1, "preview");
const both = await rewind(rewound.id, seq1, "both");
const woke = await waitFor(rewound.id, (s) => s.status === "idle", "the rewound session to be idle", 60000);
check(
  "a stopped session is started again to be asked, and nothing is sent to it",
  JSON.stringify(names(wakePreview.files)) === '["a.txt"]' && woke.pid != null && (await eventsOf(rewound.id)).filter((e) => e.kind === "user").length === messagesBefore,
  `${JSON.stringify(names(wakePreview.files))}, pid ${woke.pid}`,
);
check("both at once: the code and the conversation go back to before the first message", both.code?.ok === true && both.conversation?.ok === true && files()["a.txt"] === "alpha", `${JSON.stringify(both).slice(0, 200)}; ${JSON.stringify(files())}`);
await api("POST", `/sessions/${rewound.id}/messages`, { text: "How many messages did I send you before this one? Answer with a number only." });
const fresh = lastResult(await turnEnds(5))?.result ?? "";
check("after that, the conversation starts over for Claude", /\b(0|zero|none)\b/i.test(fresh), fresh.slice(0, 80));

// While a turn runs: Claude has edited a file and is in the middle of a slow command. hangar
// stops the turn first, as Claude Code's terminal does, then restores both.
await api("POST", `/sessions/${rewound.id}/messages`, {
  text: "Use the Edit tool to change the word alpha to OMEGA in a.txt. Then run this exact Bash command, which takes a while: node -e \"setTimeout(() => console.log('slow'), 40000)\" . Then reply DONE.",
});
let midTurn;
for (const end = Date.now() + 120000; Date.now() < end; await sleep(400)) {
  const s = await session(rewound.id);
  const bash = (await eventsOf(rewound.id)).some((e) => e.kind === "claude" && e.msg.type === "assistant" && e.msg.message.content.some((b) => b.type === "tool_use" && b.name === "Bash"));
  if (s.in_turn && bash && files()["a.txt"] === "OMEGA") {
    midTurn = s;
    break;
  }
}
const busyAt = (await eventsOf(rewound.id)).filter((e) => e.kind === "user").at(-1).seq;
const startedAt = Date.now();
const mid = await rewind(rewound.id, busyAt, "both");
const afterMid = await session(rewound.id);
check(
  "while a turn runs, a restore stops it first and then puts back the code and the conversation",
  !!midTurn && mid.code?.ok === true && mid.conversation?.ok === true && files()["a.txt"] === "alpha" && !afterMid.in_turn,
  `${midTurn ? "the turn was running" : "the turn never got to its command"}; ${JSON.stringify(mid).slice(0, 160)}; a.txt=${files()["a.txt"]}; took ${Date.now() - startedAt} ms`,
);
check("…and the session is idle afterwards, not still working", (await waitFor(rewound.id, (s) => s.status === "idle", "idle after the restore", 30000)).status === "idle");
await api("DELETE", `/sessions/${rewound.id}`);

// ---------------------------------------------------------------- three harder cases
const resultsOf = (events) => events.filter((e) => e.kind === "claude" && e.msg.type === "result").length;
/** Waits until the session has ended `n` turns and is idle; returns its history. */
async function turnsDone(sid, n, ms = 240000) {
  for (const end = Date.now() + ms; Date.now() < end; await sleep(500)) {
    const events = await eventsOf(sid);
    if (resultsOf(events) >= n && (await session(sid)).status === "idle") return events;
  }
  throw new Error(`turn ${n} of ${sid} did not end: ${JSON.stringify(await session(sid))}`);
}
const newFolder = (name, text) => {
  const dir = path.join(proj, name);
  fs.mkdirSync(dir);
  fs.writeFileSync(path.join(dir, "a.txt"), text);
  return dir;
};
const refusal = async (run) => {
  try {
    await run();
    return "";
  } catch (e) {
    return e.message;
  }
};

// 1. After Claude has summarized the conversation (/compact here; it also happens by itself in
// a long session), nothing from before the summary is offered. Claude Code goes back past a
// summary only until its process is restarted (seen on 2.1.330), and hangar restarts processes.
const sumDir = newFolder("summary", "alpha\n");
const summed = await api("POST", "/sessions", { cwd: sumDir, prompt: "Use the Edit tool to change the word alpha to ALPHA in a.txt. Then reply DONE.", model: "haiku", permission_mode: "bypassPermissions", trust: true });
await turnsDone(summed.id, 1);
await api("POST", `/sessions/${summed.id}/messages`, { text: "/compact" });
// A message sent while the summary is being made: Claude begins it after the summary.
await sleep(1500);
await api("POST", `/sessions/${summed.id}/messages`, { text: "Remember the word KIWI. Reply with the single word OK." });
const afterCompact = await turnsDone(summed.id, 3);
const boundary = afterCompact.find((e) => e.kind === "claude" && e.msg.type === "system" && e.msg.subtype === "compact_boundary");
check("summarizing ends like a turn, and Claude Code's note of it is kept", !!boundary && !(await session(summed.id)).in_turn, JSON.stringify(boundary?.msg.compact_metadata ?? null).slice(0, 160));
await api("POST", `/sessions/${summed.id}/messages`, { text: "Remember the word PLUM. Reply with the single word OK." });
const sumEvents = await turnsDone(summed.id, 4);
const sumSeqs = sumEvents.filter((e) => e.kind === "user").map((e) => e.seq);
const pastSummary = await refusal(() => rewind(summed.id, sumSeqs[0], "preview"));
const pastSummaryBoth = await refusal(() => rewind(summed.id, sumSeqs[0], "both"));
check("a message from before the summary is not offered, for a preview or a restore", /before Claude summarized the conversation/.test(pastSummary) && /before Claude summarized the conversation/.test(pastSummaryBoth) && files_in(sumDir) === "ALPHA", pastSummary.slice(0, 160));
const afterSummary = await rewind(summed.id, sumSeqs[3], "conversation");
check("a message from after the summary goes back as ever", afterSummary.conversation?.ok === true && /PLUM/.test(afterSummary.conversation.prefill ?? ""), JSON.stringify(afterSummary).slice(0, 160));
// The message sent during the summary is written before the summary's mark, yet Claude began it after.
const boundarySeq = sumEvents.find((e) => e.kind === "claude" && e.msg.subtype === "compact_boundary").seq;
const duringSummary = await refusal(() => rewind(summed.id, sumSeqs[2], "conversation").then((r) => (r.conversation?.ok ? "" : JSON.stringify(r))));
check(
  "a message sent while Claude was summarizing can be gone back to: Claude began it after the summary",
  sumSeqs[2] < boundarySeq && duringSummary === "",
  `sent at ${sumSeqs[2]}, summary's mark at ${boundarySeq}; ${duringSummary || "went back"}; unstarted then: ${JSON.stringify(boundary?.msg.hangar_unstarted)}`,
);
await api("DELETE", `/sessions/${summed.id}`);

// 2. An agent started by an agent. Claude Code announces the inner one (started, doing, ended)
// without sending its steps.
const nestDir = newFolder("nested", "alpha\n");
const nested = await api("POST", "/sessions", {
  cwd: nestDir,
  prompt:
    "Use the Agent tool once (general-purpose, run_in_background: false). Tell that agent exactly this: 'Use the Agent tool yourself (general-purpose, run_in_background: false) " +
    "to read the file a.txt in the current folder and report its content. If you have no Agent tool, say exactly NO AGENT TOOL. Report what happened.' " +
    "Do not do any of it yourself. Then tell me in one short sentence what the agent reported.",
  model: "haiku",
  permission_mode: "bypassPermissions",
  trust: true,
});
let most = 0;
for (const end = Date.now() + 240000; Date.now() < end; await sleep(120)) {
  const s = await session(nested.id);
  most = Math.max(most, s.agents.length);
  if (s.status === "idle" && resultsOf(await eventsOf(nested.id)) >= 1) break;
}
const nestEvents = await eventsOf(nested.id);
const innerCalls = nestEvents
  .filter((e) => e.kind === "claude" && e.msg.type === "assistant" && e.msg.parent_tool_use_id)
  .flatMap((e) => e.msg.message.content.filter((b) => b.type === "tool_use" && b.name === "Agent").map((b) => b.id));
const innerStart = nestEvents.find((e) => e.kind === "claude" && e.msg.subtype === "task_started" && innerCalls.includes(e.msg.tool_use_id))?.msg;
const innerEnd = nestEvents.find((e) => e.kind === "claude" && e.msg.subtype === "task_notification" && e.msg.task_id === innerStart?.task_id)?.msg;
const innerSteps = nestEvents.filter((e) => e.kind === "claude" && innerCalls.includes(e.msg.parent_tool_use_id)).length;
check("an agent started by an agent is announced by Claude Code, with the call it belongs to", !!innerStart && innerEnd?.status === "completed", JSON.stringify(innerStart ?? lastResult(nestEvents)?.result).slice(0, 200));
check("…Claude Code sends none of its steps, only its account of it", !!innerStart && innerSteps === 0, `${innerSteps} messages from inside it`);
check("both agents were listed while they ran, and none is left when they are done", most === 2 && (await session(nested.id)).agents.length === 0 && (await session(nested.id)).status === "idle", `most at once: ${most}`);
await api("DELETE", `/sessions/${nested.id}`);

// 3. "Restore code" while an agent is still at work on the same file. Claude Code does not stop
// agents for it, and neither does hangar: the restore goes through, the agent goes on, and what
// it then writes is its own.
const editDir = newFolder("agent-edits", "alpha\n");
const editing = await api("POST", "/sessions", {
  cwd: editDir,
  prompt:
    "First use the Edit tool yourself to change the word alpha to ALPHA in a.txt. Then start ONE helper agent with the Agent tool (general-purpose, run_in_background: true). " +
    "Its job, in this order: run this one command, which takes a while: node -e \"setTimeout(() => console.log('waited'), 25000)\" ; when it has finished, read a.txt and use the Edit tool to change " +
    "the one word in it to AGENT. Do not do the agent's job yourself. Once the agent is started, reply with one short sentence.",
  model: "haiku",
  permission_mode: "bypassPermissions",
  trust: true,
});
let waiting;
for (const end = Date.now() + 180000; Date.now() < end; await sleep(400)) {
  const s = await session(editing.id);
  const atCommand = (await eventsOf(editing.id)).some((e) => e.kind === "claude" && e.msg.subtype === "task_progress" && ["Bash", "PowerShell"].includes(e.msg.last_tool_name));
  if (!s.in_turn && s.agents.length === 1 && atCommand && files_in(editDir) === "ALPHA") {
    waiting = s;
    break;
  }
}
const editSeq = (await eventsOf(editing.id)).find((e) => e.kind === "user").seq;
const editPreview = await rewind(editing.id, editSeq, "preview");
const editRestore = await rewind(editing.id, editSeq, "code");
const rightAfter = await session(editing.id);
check(
  "with an agent at work, Restore code goes through: the file is back",
  !!waiting && JSON.stringify(names(editPreview.files)) === '["a.txt"]' && editRestore.code?.ok === true && files_in(editDir) === "alpha",
  `${waiting ? "agent was at its command" : "the agent never got to its command"}; preview ${JSON.stringify(names(editPreview.files))}; a.txt=${files_in(editDir)}`,
);
check("…the agent is not stopped, and the session still counts as working", rightAfter.agents.length === 1 && rightAfter.status === "working", `${rightAfter.status}, ${rightAfter.agents.length} agent(s)`);
const editDone = await waitFor(editing.id, (s) => s.status === "idle" && s.agents.length === 0, "the agent to finish after the restore", 180000);
check("…and it goes on to change the file again, as the dialog warns", files_in(editDir) === "AGENT" && editDone.status === "idle", `a.txt=${files_in(editDir)}`);
await api("DELETE", `/sessions/${editing.id}`);

await api("DELETE", `/sessions/${id}`);
check("deleted", !(await session(id)));
execFileSync(hangar, ["stop"], { env });
const failed = results.filter((r) => !r).length;
console.log(`\n${results.length - failed}/${results.length} checks passed`);
process.exit(failed ? 1 : 0);
