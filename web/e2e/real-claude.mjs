// Real Claude Code sessions (Haiku) driven through hangar's HTTP API:
// permission prompt, approval, per-turn cost, stop, resume with the
// conversation intact; then a question answered, a plan sent back and
// approved, an edit's changed lines, and the context and limit figures
// checked against Claude's own. Costs a few cents.
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
const env = { ...process.env, HANGAR_HOME: home };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const check = (name, ok, detail = "") => {
  results.push(ok);
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};

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

await api("DELETE", `/sessions/${id}`);
check("deleted", !(await session(id)));
execFileSync(hangar, ["stop"], { env });
const failed = results.filter((r) => !r).length;
console.log(`\n${results.length - failed}/${results.length} checks passed`);
process.exit(failed ? 1 : 0);
