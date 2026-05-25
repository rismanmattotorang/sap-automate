'use client';

import { useEffect, useState } from 'react';
import { initialize, listPrompts, getPrompt, Prompt } from '@/lib/mcp';
import { Badge, Card, PageHeader } from '@/components/badge';

export default function SkillLab() {
  const [prompts, setPrompts] = useState<Prompt[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [args, setArgs] = useState<Record<string, string>>({});
  const [rendered, setRendered] = useState<string | null>(null);
  const [description, setDescription] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    initialize().then(() => listPrompts()).then(p => {
      setPrompts(p.prompts);
      if (p.prompts.length > 0) {
        const first = p.prompts.find(x => x.name.startsWith('sap.skill.')) ?? p.prompts[0];
        setSelected(first.name);
      }
    });
  }, []);

  const prompt = prompts.find(p => p.name === selected);

  // Auto-render when arguments are filled.
  useEffect(() => {
    if (!prompt) return;
    const required = (prompt.arguments ?? []).filter(a => a.required);
    const haveAllRequired = required.every(a => args[a.name] && args[a.name].trim() !== '');
    if (!haveAllRequired) return;
    let cancelled = false;
    setLoading(true); setError(null);
    getPrompt(prompt.name, args)
      .then(r => {
        if (cancelled) return;
        setDescription(r.description ?? null);
        const text = r.messages?.[0]?.content?.text ?? '';
        setRendered(text);
      })
      .catch(e => !cancelled && setError(e.message))
      .finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, [prompt?.name, JSON.stringify(args)]);

  // Reset args on prompt switch.
  useEffect(() => {
    setArgs({});
    setRendered(null);
    setDescription(null);
    setError(null);
  }, [prompt?.name]);

  return (
    <>
      <PageHeader
        title="Skill Lab"
        subtitle="Skills are markdown files with YAML frontmatter, auto-loaded from ./skills/. Each becomes an MCP prompt with typed arguments."
        right={<Badge tone="neutral">{prompts.length} prompts</Badge>}
      />

      <div className="flex h-[calc(100vh-73px)]">
        <div className="w-72 border-r border-ink-800 overflow-y-auto scrollbar-thin">
          <ul>
            {prompts.map(p => (
              <li key={p.name}>
                <button
                  onClick={() => setSelected(p.name)}
                  className={`w-full text-left px-3 py-2 text-xs transition border-l-2 ${
                    p.name === selected
                      ? 'bg-ink-800 border-accent-500 text-accent-500'
                      : 'border-transparent hover:bg-ink-900 text-zinc-300'
                  }`}
                >
                  <div className="flex items-center gap-2">
                    {p.name.startsWith('sap.skill.') && <Badge tone="accent">skill</Badge>}
                    {!p.name.startsWith('sap.skill.') && <Badge tone="neutral">built-in</Badge>}
                  </div>
                  <div className="font-semibold mt-1">{p.name}</div>
                  <div className="text-[10.5px] text-ink-600 line-clamp-2 mt-0.5 leading-tight">
                    {p.description}
                  </div>
                </button>
              </li>
            ))}
          </ul>
        </div>

        <div className="flex-1 overflow-y-auto p-6 space-y-4">
          {prompt ? (
            <>
              <div>
                <h2 className="text-xl font-bold text-zinc-100">{prompt.name}</h2>
                <p className="text-sm text-zinc-400 mt-1">{prompt.description}</p>
              </div>

              {prompt.arguments && prompt.arguments.length > 0 && (
                <Card title="Arguments">
                  <div className="space-y-3">
                    {prompt.arguments.map(a => (
                      <label key={a.name} className="block">
                        <div className="text-xs mb-1 flex items-baseline gap-2">
                          <span className="font-bold text-zinc-200">{a.name}</span>
                          {a.required && <Badge tone="warn">required</Badge>}
                        </div>
                        {a.description && <div className="text-[11px] text-ink-600 mb-1.5">{a.description}</div>}
                        <input
                          type="text"
                          value={args[a.name] ?? ''}
                          onChange={e => setArgs({ ...args, [a.name]: e.target.value })}
                          className="w-full bg-ink-950 border border-ink-700 rounded px-2 py-1.5 text-xs focus:outline-none focus:border-accent-500"
                        />
                      </label>
                    ))}
                  </div>
                </Card>
              )}

              <Card title="Rendered skill body" right={loading ? <Badge tone="warn">rendering</Badge> : rendered && <Badge tone="good">live preview</Badge>}>
                {error ? (
                  <div className="text-xs text-bad">{error}</div>
                ) : rendered ? (
                  <pre className="text-[12px] whitespace-pre-wrap leading-relaxed text-zinc-300 max-h-[28rem] overflow-auto scrollbar-thin">
                    {rendered}
                  </pre>
                ) : (
                  <div className="text-xs text-ink-600">
                    Fill required arguments to render the skill.
                  </div>
                )}
              </Card>

              <Card title="Why this matters">
                <div className="text-xs text-zinc-400 leading-relaxed space-y-2">
                  <p>
                    The convergent pattern across <code>SAP/mdk-mcp-server</code> (AGENTS.md),
                    <code>fr0ster/mcp-abap-adt</code> (handler dedup), and the
                    <code>marianfoo/sap-ai-mcp-servers</code> registry (CAP Agentic Engineered Skills,
                    RAP Skills, ARC-1 SAP Skills): <b>agents invoke skills, not raw tools</b>.
                  </p>
                  <p>
                    A skill bundles tool composition + prompt engineering for one SAP workflow.
                    Authors ship them as <code>./skills/*.md</code> with YAML frontmatter; the server
                    auto-loads them and exposes them via MCP <code>prompts/get</code>.
                    No protocol extension required.
                  </p>
                  <p>
                    <b>Behavioural skills</b> ride the same surface.
                    {' '}<code>sap.skill.karpathy_guidelines</code> (ported from {' '}
                    <code>multica-ai/andrej-karpathy-skills</code>, MIT)
                    captures the four pre-flight principles — think-before, simplicity, surgical
                    changes, goal-driven execution — and {' '}
                    <code>sap.skill.aipnv_ai_pairing</code> the five-question anti-autopilot
                    checklist from <code>fr0ster/mcp-abap-adt</code>. The gateway routes user
                    intents directly to these skills via <code>prompts/get</code>.
                  </p>
                </div>
              </Card>
            </>
          ) : (
            <p className="text-xs text-ink-600">Select a skill.</p>
          )}
        </div>
      </div>
    </>
  );
}
