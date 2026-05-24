'use client';

import { useEffect, useState } from 'react';
import { initialize, listTools, listResources, listPrompts, callTool, Tool, Prompt } from '@/lib/mcp';
import { Badge, Card, PageHeader } from '@/components/badge';

interface ToolStat {
  name: string;
  calls: number;
  errors: number;
  totalLatencyMs: number;
  lastLatencyMs?: number;
  description?: string;
  isWrite?: boolean;
}

export default function Operations() {
  const [server, setServer] = useState<{ name: string; version: string } | null>(null);
  const [protocolVersion, setProtocolVersion] = useState('');
  const [instructions, setInstructions] = useState('');
  const [tools, setTools] = useState<Tool[]>([]);
  const [resourceCount, setResourceCount] = useState(0);
  const [prompts, setPrompts] = useState<Prompt[]>([]);
  const [stats, setStats] = useState<Record<string, ToolStat>>({});
  const [error, setError] = useState<string | null>(null);
  const [pinging, setPinging] = useState(true);

  useEffect(() => {
    (async () => {
      try {
        const init = await initialize();
        setServer(init.serverInfo);
        setProtocolVersion(init.protocolVersion);
        setInstructions(init.instructions ?? '');
        const t = await listTools();
        setTools(t.tools);
        const r = await listResources();
        setResourceCount(r.resources.length);
        const p = await listPrompts();
        setPrompts(p.prompts);
      } catch (e: any) {
        setError(e.message);
      }
    })();
  }, []);

  // Synthetic background traffic so the dashboard isn't dead on first load.
  useEffect(() => {
    if (!server || tools.length === 0) return;
    let cancelled = false;

    const samples = [
      { tool: 'sap.docs.search', args: { query: 'period close', top_k: 3 } },
      { tool: 'sap.docs.search', args: { query: 'goods movement', top_k: 3 } },
      { tool: 'abap.search', args: { query: 'BAPI ZFIN', top_k: 3 } },
      { tool: 'sap.help.search', args: { query: 'billing', top_k: 3 } },
      { tool: 'sap.system.info', args: {} },
      { tool: 'sap.rfc.search', args: { query: 'material' } },
    ];

    let i = 0;
    const tick = async () => {
      if (cancelled) return;
      const probe = samples[i++ % samples.length];
      const t0 = performance.now();
      try {
        const r = await callTool(probe.tool, probe.args);
        const latency = performance.now() - t0;
        setStats((prev) => {
          const cur = prev[probe.tool] ?? { name: probe.tool, calls: 0, errors: 0, totalLatencyMs: 0 };
          return {
            ...prev,
            [probe.tool]: {
              ...cur,
              calls: cur.calls + 1,
              errors: cur.errors + (r.isError ? 1 : 0),
              totalLatencyMs: cur.totalLatencyMs + latency,
              lastLatencyMs: latency,
            },
          };
        });
      } catch (e: any) {
        setStats((prev) => {
          const cur = prev[probe.tool] ?? { name: probe.tool, calls: 0, errors: 0, totalLatencyMs: 0 };
          return { ...prev, [probe.tool]: { ...cur, errors: cur.errors + 1, calls: cur.calls + 1 } };
        });
      }
      setPinging(false);
      if (!cancelled) setTimeout(tick, 600);
    };
    tick();
    return () => { cancelled = true; };
  }, [server, tools.length]);

  const totalCalls = Object.values(stats).reduce((s, x) => s + x.calls, 0);
  const totalErrors = Object.values(stats).reduce((s, x) => s + x.errors, 0);
  const meanLatency = totalCalls > 0
    ? Object.values(stats).reduce((s, x) => s + x.totalLatencyMs, 0) / totalCalls
    : 0;
  const budgetPct = Math.min(100, (meanLatency / 80) * 100);
  const budgetTone = budgetPct < 40 ? 'good' : budgetPct < 80 ? 'warn' : 'bad';

  const toolsByGroup = groupTools(tools);

  return (
    <>
      <PageHeader
        title="Operations"
        subtitle={server ? `${server.name} v${server.version} · MCP ${protocolVersion}` : 'connecting…'}
        right={
          <div className="flex items-center gap-2">
            <Badge tone={pinging ? 'warn' : 'good'}>{pinging ? 'probing' : 'live'}</Badge>
            <Badge tone="neutral">{tools.length} tools</Badge>
            <Badge tone="neutral">{resourceCount} resources</Badge>
            <Badge tone="neutral">{prompts.length} prompts</Badge>
          </div>
        }
      />

      <div className="p-6 space-y-6 max-w-[1500px]">
        {error && (
          <div className="text-sm text-bad bg-bad/10 border border-bad/30 rounded px-3 py-2">
            Server not reachable: {error}<br />
            <span className="text-xs text-ink-600">
              Start it with: <code className="text-accent-500">./target/release/sap-automate-server --transport http</code>
            </span>
          </div>
        )}

        {/* Hero stats */}
        <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
          <Stat label="Total calls" value={totalCalls.toString()} tone="accent" />
          <Stat label="Errors" value={totalErrors.toString()} tone={totalErrors === 0 ? 'good' : totalErrors > 3 ? 'bad' : 'warn'} />
          <Stat label="Mean latency" value={`${meanLatency.toFixed(1)} ms`} tone={budgetTone} />
          <Stat label="P95 gate" value="80 ms" tone="good" />
        </div>

        {/* Latency budget gauge */}
        <Card title="Latency budget (paper §X-D acceptance gate)">
          <div className="space-y-2">
            <div className="text-xs text-zinc-400">
              Mean latency ÷ 80 ms gate: <span className="font-bold text-zinc-100">{budgetPct.toFixed(1)}%</span>
            </div>
            <div className="h-3 rounded-full bg-ink-800 overflow-hidden">
              <div
                className={`h-full transition-all ${
                  budgetTone === 'good' ? 'bg-good' : budgetTone === 'warn' ? 'bg-warn' : 'bg-bad'
                }`}
                style={{ width: `${budgetPct}%` }}
              />
            </div>
            <div className="text-[11px] text-ink-600">
              Server-side P95 over 1000 queries: <b>~0.16 ms</b> (Phase 3 bench harness, 500× under gate).
              The browser/proxy round-trip you see here is the full UX cost.
            </div>
          </div>
        </Card>

        {/* Tool catalogue grouped */}
        <Card title="Tool catalogue (grouped by domain)">
          <div className="space-y-4">
            {Object.entries(toolsByGroup).map(([group, ts]) => (
              <div key={group}>
                <div className="text-xs uppercase tracking-wide text-ink-600 mb-1.5">{group} · {ts.length}</div>
                <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-2">
                  {ts.map((t) => {
                    const s = stats[t.name];
                    return (
                      <div key={t.name} className="border border-ink-800 rounded p-2 bg-ink-950">
                        <div className="flex items-center gap-2 mb-1">
                          <span className="font-bold text-accent-500 text-[12.5px] truncate">{t.name}</span>
                          {s && <span className="ml-auto text-[10px] text-zinc-400">{s.calls} calls</span>}
                        </div>
                        <p className="text-[11px] text-ink-600 leading-snug line-clamp-2">
                          {t.description ?? 'no description'}
                        </p>
                        {s && s.lastLatencyMs !== undefined && (
                          <div className="text-[10px] text-zinc-500 mt-1">
                            last: {s.lastLatencyMs.toFixed(1)} ms · mean: {(s.totalLatencyMs / Math.max(1, s.calls)).toFixed(1)} ms
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>
        </Card>

        <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
          {/* Skills */}
          <Card title="Skills loaded from disk">
            <ul className="space-y-1.5">
              {prompts.filter(p => p.name.startsWith('sap.skill.')).map(p => (
                <li key={p.name} className="text-xs">
                  <span className="text-accent-500 font-semibold">{p.name}</span>
                  <span className="text-zinc-500"> — {p.description}</span>
                </li>
              ))}
              {prompts.filter(p => p.name.startsWith('sap.skill.')).length === 0 && (
                <li className="text-xs text-ink-600">No skills loaded.</li>
              )}
            </ul>
            <div className="mt-3 text-[11px] text-ink-600">
              Drop a markdown file into <code className="text-accent-500">./skills/*.md</code> with YAML frontmatter and it becomes an MCP prompt.
            </div>
          </Card>

          {/* Instructions / AGENTS.md */}
          <Card title="Server instructions (AGENTS.md + capability summary)">
            <pre className="text-[11px] text-zinc-400 whitespace-pre-wrap leading-relaxed max-h-72 overflow-auto scrollbar-thin">
              {instructions || '(no instructions)'}
            </pre>
          </Card>
        </div>
      </div>
    </>
  );
}

function Stat({ label, value, tone }: { label: string; value: string; tone: 'good' | 'warn' | 'bad' | 'accent' }) {
  const borderTone = {
    good: 'border-good/30',
    warn: 'border-warn/30',
    bad: 'border-bad/30',
    accent: 'border-accent-500/30',
  }[tone];
  const valueTone = {
    good: 'text-good',
    warn: 'text-warn',
    bad: 'text-bad',
    accent: 'text-accent-500',
  }[tone];
  return (
    <div className={`rounded-lg border ${borderTone} bg-ink-900 px-4 py-3`}>
      <div className="text-[11px] text-ink-600 uppercase tracking-wide">{label}</div>
      <div className={`text-2xl font-bold ${valueTone} mt-1`}>{value}</div>
    </div>
  );
}

function groupTools(tools: Tool[]): Record<string, Tool[]> {
  const groups: Record<string, Tool[]> = {
    'RAG search': [],
    'SAP system / RFC / tables': [],
    'ABAP ADT': [],
    'Other': [],
  };
  for (const t of tools) {
    if (t.name.startsWith('abap.adt.')) groups['ABAP ADT'].push(t);
    else if (t.name.startsWith('sap.')) groups['SAP system / RFC / tables'].push(t);
    else if (['abap.search', 'bpmn.find_process', 'eam.search_apps', 'sap.help.search'].includes(t.name)) groups['RAG search'].push(t);
    else groups['Other'].push(t);
  }
  for (const k of Object.keys(groups)) {
    if (groups[k].length === 0) delete groups[k];
    else groups[k].sort((a, b) => a.name.localeCompare(b.name));
  }
  return groups;
}
