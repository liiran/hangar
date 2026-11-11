import { Check, CircleDollarSign, Clock, Copy, Cpu, Folder, MemoryStick, Shield } from "lucide-react";
import { useState, type ReactNode } from "react";
import { api, type Session } from "../api";
import { ago, basename, bytes, PERMISSION_MODES, STATUS, usd } from "../format";
import { report } from "../store";
import { useNow } from "../ui";
import { Select } from "./Select";
import { cx, StatusIcon } from "./ui";

const MCP_DOT: Record<string, string> = { connected: "bg-emerald-500", pending: "bg-s-working", failed: "bg-s-error" };

export function Properties({ session: s }: { session: Session }) {
  const now = useNow();
  const setMode = (mode: string) => {
    if (mode === "bypassPermissions" && !confirm("Full access lets Claude run any command and edit any file without asking. Only use it in a sandbox you trust. Continue?")) return;
    api.update(s.id, { permission_mode: mode }).catch(report);
  };
  return (
    <div className="h-full overflow-y-auto pb-6">
      <div className="flex h-12 items-center px-5 text-[12px] font-medium text-muted">Properties</div>
      <div className="space-y-0.5 px-3">
        <Row icon={<StatusIcon status={s.status} />} title={STATUS[s.status].hint}>
          {STATUS[s.status].label}
        </Row>
        <Select variant="row" icon={<Shield className="size-3.5" />} label="Permissions" value={s.permission_mode} onChange={setMode} options={PERMISSION_MODES} />
        <Row icon={<Cpu className="size-3.5" />}>{s.model ?? "Default model"}</Row>
        <Row icon={<Folder className="size-3.5" />} title={s.cwd}>
          {basename(s.cwd)}
        </Row>
      </div>

      <Section title="Usage">
        <Row icon={<MemoryStick className="size-3.5" />} title="Memory now; peak across its runs">
          {s.memory_bytes != null ? bytes(s.memory_bytes) : "Not running"}
          {s.peak_bytes > 0 && <span className="text-faint"> · peak {bytes(s.peak_bytes)}</span>}
        </Row>
        <Row icon={<CircleDollarSign className="size-3.5" />}>
          {usd(s.cost_usd)}
          <span className="text-faint">
            {" "}
            · {s.turns} {s.turns === 1 ? "turn" : "turns"}
          </span>
        </Row>
      </Section>

      <Section title="MCP servers" count={s.mcp.length}>
        {s.mcp.length === 0 && <p className="px-2 text-[12px] text-faint">{s.pid != null ? "None" : "Shown while the session runs"}</p>}
        {s.mcp.map((m) => (
          <div key={m.name} className="flex h-7 items-center gap-2.5 px-2 text-[12.5px]" title={m.status}>
            <span className={cx("size-1.5 shrink-0 rounded-full", MCP_DOT[m.status] ?? "bg-faint")} />
            <span className="min-w-0 flex-1 truncate">{m.name}</span>
            {m.status !== "connected" && <span className="text-[11px] text-faint">{m.status}</span>}
          </div>
        ))}
      </Section>

      <Section title="Session">
        <Row icon={<Clock className="size-3.5" />}>
          Started {ago(s.created_ms, now)} <span className="text-faint">· active {ago(s.updated_ms, now)}</span>
        </Row>
        {s.claude_session_id && <CopyRow label="Claude session" value={s.claude_session_id} />}
      </Section>
    </div>
  );
}

function Section({ title, count, children }: { title: string; count?: number; children: ReactNode }) {
  return (
    <section className="mt-5 px-3">
      <h3 className="mb-1 px-2 text-[12px] font-medium text-muted">
        {title}
        {count != null && count > 0 && <span className="ml-1.5 font-normal text-faint">{count}</span>}
      </h3>
      <div className="space-y-0.5">{children}</div>
    </section>
  );
}

function Row({ icon, children, title }: { icon: ReactNode; children: ReactNode; title?: string }) {
  return (
    <div className="flex min-h-8 items-center gap-2.5 rounded-md px-2 py-1 text-[13px]" title={title}>
      <span className="shrink-0 text-muted">{icon}</span>
      <span className="min-w-0 flex-1 truncate">{children}</span>
    </div>
  );
}

function CopyRow({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      onClick={() => {
        navigator.clipboard?.writeText(value).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1200);
        }, report);
      }}
      className="flex h-8 w-full items-center gap-2.5 rounded-md px-2 text-left text-[12.5px] hover:bg-panel-2"
      title={`Copy ${label.toLowerCase()} id`}
    >
      <span className="shrink-0 text-muted">{copied ? <Check className="size-3.5 text-emerald-500" /> : <Copy className="size-3.5" />}</span>
      <span className="text-muted">{label}</span>
      <span className="min-w-0 flex-1 truncate text-right font-mono text-[11.5px] text-faint">{value.slice(0, 8)}</span>
    </button>
  );
}
