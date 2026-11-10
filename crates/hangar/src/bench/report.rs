//! Turning run results into numbers people can trust.

use super::{BenchOpts, Mode, RunResult, presets::ServerSet};
use crate::procs;
use crate::util::fmt_bytes;

/// Nearest-rank percentile (`p` in 0..=100) of the values.
pub fn percentile(values: &[u64], p: f64) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let rank = ((p / 100.0) * v.len() as f64).ceil() as usize;
    Some(v[rank.clamp(1, v.len()) - 1])
}

/// Ready times of the sessions that succeeded.
pub fn ready(values: &[Option<u64>]) -> Vec<u64> {
    values.iter().flatten().copied().collect()
}

pub fn failed(values: &[Option<u64>]) -> usize {
    values.iter().filter(|x| x.is_none()).count()
}

pub fn fmt_ms(v: Option<u64>) -> String {
    match v {
        Some(ms) if ms >= 1000 => format!("{:.1} s", ms as f64 / 1000.0),
        Some(ms) => format!("{ms} ms"),
        None => "-".into(),
    }
}

/// "−78%" for a saving, "+12%" for a regression.
pub fn change(before: u64, after: u64) -> String {
    if before == 0 {
        return "-".into();
    }
    let pct = 100.0 * (after as f64 - before as f64) / before as f64;
    if pct <= 0.0 { format!("−{:.0}%", -pct) } else { format!("+{pct:.0}%") }
}

pub fn summary_line(r: &RunResult) -> String {
    let rd = ready(&r.session_ready_ms);
    format!(
        "{:>6} x{:<3} mcp {:>9} in {:>3} procs | cpu {:>6.1} s | ready p50 {:>7} p95 {:>7} | failed {} | leftovers {}",
        r.mode.to_string(),
        r.sessions,
        fmt_bytes(r.mcp.private_bytes),
        r.mcp.processes,
        r.cpu_ms as f64 / 1000.0,
        fmt_ms(percentile(&rd, 50.0)),
        fmt_ms(percentile(&rd, 95.0)),
        failed(&r.session_ready_ms),
        r.leftovers.processes
    )
}

pub fn render_markdown(results: &[RunResult], opts: &BenchOpts, servers: &ServerSet) -> String {
    let mut s = String::new();
    let (total, _) = procs::system_memory();
    let notes = [
        opts.browse.then_some(", every session opens a page in its browser"),
        (!opts.fast_npx).then_some(", npx fast path off"),
        opts.kill_agents.then_some(", sessions end by killing the agent"),
    ];
    s.push_str(&format!(
        "## hangar bench: {} agent, preset `{}`{}\n\nServers: {}. Machine: {} RAM, {} logical CPUs, {}.\n\n",
        opts.agent,
        opts.preset,
        notes.iter().flatten().copied().collect::<String>(),
        servers.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "),
        fmt_bytes(total),
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        std::env::consts::OS
    ));
    s.push_str("| Sessions | Mode | MCP processes | MCP memory (private) | Agent memory | CPU to start | Ready p50 | Ready p95 | Ready max | Failed | Leftovers after close |\n");
    s.push_str("|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for r in results {
        let rd = ready(&r.session_ready_ms);
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {:.1} s | {} | {} | {} | {} | {} ({}) |\n",
            r.sessions,
            r.mode,
            r.mcp.processes,
            fmt_bytes(r.mcp.private_bytes),
            if r.agents.processes > 0 { fmt_bytes(r.agents.private_bytes) } else { "-".into() },
            r.cpu_ms as f64 / 1000.0,
            fmt_ms(percentile(&rd, 50.0)),
            fmt_ms(percentile(&rd, 95.0)),
            fmt_ms(rd.iter().max().copied()),
            failed(&r.session_ready_ms),
            r.leftovers.processes,
            fmt_bytes(r.leftovers.private_bytes),
        ));
    }
    let mut lines = Vec::new();
    for n in results.iter().map(|r| r.sessions).collect::<std::collections::BTreeSet<_>>() {
        let d = results.iter().find(|r| r.sessions == n && r.mode == Mode::Direct);
        let p = results.iter().find(|r| r.sessions == n && r.mode == Mode::Pooled);
        let (Some(d), Some(p)) = (d, p) else { continue };
        let broken = failed(&d.session_ready_ms) + failed(&p.session_ready_ms);
        if broken > 0 {
            // Fewer servers came up, so the totals would flatter one side.
            lines.push(format!("- **{n} sessions:** not compared: {broken} session(s) failed to start (see the table)."));
            continue;
        }
        let p95 = |r: &RunResult| percentile(&ready(&r.session_ready_ms), 95.0);
        let mut line = format!(
            "- **{n} sessions:** MCP memory {} → {} (**{}**), processes {} → {}, startup CPU {:.1} s → {:.1} s (**{}**), ready p95 {} → {}",
            fmt_bytes(d.mcp.private_bytes),
            fmt_bytes(p.mcp.private_bytes),
            change(d.mcp.private_bytes, p.mcp.private_bytes),
            d.mcp.processes,
            p.mcp.processes,
            d.cpu_ms as f64 / 1000.0,
            p.cpu_ms as f64 / 1000.0,
            change(d.cpu_ms, p.cpu_ms),
            fmt_ms(p95(d)),
            fmt_ms(p95(p)),
        );
        if d.agents.processes > 0 {
            let dt = d.mcp.private_bytes + d.agents.private_bytes;
            let pt = p.mcp.private_bytes + p.agents.private_bytes;
            line.push_str(&format!(", whole session footprint {} → {} ({})", fmt_bytes(dt), fmt_bytes(pt), change(dt, pt)));
        }
        if !p.warm_ready_ms.is_empty() {
            let w = ready(&p.warm_ready_ms);
            line.push_str(&format!(", joining warm servers p50 {} / p95 {}", fmt_ms(percentile(&w, 50.0)), fmt_ms(percentile(&w, 95.0))));
        }
        lines.push(line);
    }
    if !lines.is_empty() {
        s.push('\n');
        s.push_str(&lines.join("\n"));
        s.push('\n');
    }
    s.push_str(
        "\nEach configuration ran once, direct before pooled. \
         Memory is private (committed) bytes summed over every process, so shared DLL pages are not double counted. \
         CPU comes from job-object accounting and includes processes that exited during startup. \
         Ready is the slowest server of each session; p95 uses the nearest-rank method. \
         Leftovers are processes still alive 5 s after every session closed, excluding servers the pool keeps warm on purpose.\n",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles() {
        let v: Vec<u64> = (1..=10).collect();
        assert_eq!(percentile(&v, 50.0), Some(5));
        assert_eq!(percentile(&v, 95.0), Some(10));
        assert_eq!(percentile(&v, 90.0), Some(9));
        assert_eq!(percentile(&[7], 95.0), Some(7));
        assert_eq!(percentile(&[], 50.0), None);
    }

    #[test]
    fn changes_carry_their_sign() {
        assert_eq!(change(100, 22), "−78%");
        assert_eq!(change(100, 112), "+12%");
        assert_eq!(change(0, 5), "-");
    }
}
