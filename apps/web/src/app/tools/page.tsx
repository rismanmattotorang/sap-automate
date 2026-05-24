'use client';

import { useEffect, useMemo, useState } from 'react';
import { initialize, listTools, callTool, Tool } from '@/lib/mcp';
import { Badge, Card, PageHeader } from '@/components/badge';

export default function ToolExplorer() {
  const [tools, setTools] = useState<Tool[]>([]);
  const [filter, setFilter] = useState('');
  const [selected, setSelected] = useState<string | null>(null);

  useEffect(() => {
    initialize().then(() => listTools()).then(t => {
      setTools(t.tools);
      if (t.tools.length > 0) setSelected(t.tools.find(x => x.name === 'sap.docs.search')?.name ?? t.tools[0].name);
    });
  }, []);

  const filtered = useMemo(
    () => tools.filter(t => t.name.toLowerCase().includes(filter.toLowerCase())),
    [tools, filter]
  );
  const tool = tools.find(t => t.name === selected);

  return (
    <>
      <PageHeader
        title="Tool Explorer"
        subtitle="Schema-driven forms auto-generated from each tool's JSON Schema. Read-only flag surfaced prominently."
        right={<Badge tone="neutral">{tools.length} tools</Badge>}
      />

      <div className="flex h-[calc(100vh-73px)]">
        <div className="w-72 border-r border-ink-800 overflow-y-auto scrollbar-thin">
          <input
            type="text"
            placeholder="filter…"
            value={filter}
            onChange={e => setFilter(e.target.value)}
            className="m-3 w-[calc(100%-1.5rem)] bg-ink-950 border border-ink-700 rounded px-2 py-1.5 text-xs focus:outline-none focus:border-accent-500"
          />
          <ul>
            {filtered.map(t => (
              <li key={t.name}>
                <button
                  onClick={() => setSelected(t.name)}
                  className={`w-full text-left px-3 py-2 text-xs transition border-l-2 ${
                    t.name === selected
                      ? 'bg-ink-800 border-accent-500 text-accent-500'
                      : 'border-transparent hover:bg-ink-900 text-zinc-300'
                  }`}
                >
                  <div className="font-semibold">{t.name}</div>
                  <div className="text-[10.5px] text-ink-600 line-clamp-2 mt-0.5 leading-tight">
                    {t.description}
                  </div>
                </button>
              </li>
            ))}
          </ul>
        </div>

        <div className="flex-1 overflow-y-auto p-6">
          {tool ? <ToolForm tool={tool} /> : <p className="text-xs text-ink-600">Select a tool.</p>}
        </div>
      </div>
    </>
  );
}

function ToolForm({ tool }: { tool: Tool }) {
  const [values, setValues] = useState<Record<string, any>>({});
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<any>(null);
  const [latencyMs, setLatencyMs] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Reset state on tool change.
  useEffect(() => {
    setValues({});
    setResult(null);
    setLatencyMs(null);
    setError(null);
  }, [tool.name]);

  const schema = tool.inputSchema;
  const properties = (schema.properties ?? {}) as Record<string, any>;
  const required = new Set(schema.required ?? []);
  // Marker: write-side tools follow the naming convention we use server-side
  // (abap.adt.activate; sap.rfc.call with require_read_only_safe=false).
  const isWriteTool = tool.name === 'abap.adt.activate' || tool.name === 'sap.rfc.call';

  async function run() {
    setRunning(true); setError(null); setResult(null);
    const t0 = performance.now();
    try {
      const args = clean(values);
      const r = await callTool(tool.name, args);
      setResult(r);
      setLatencyMs(Math.round(performance.now() - t0));
    } catch (e: any) {
      setError(e.message);
    } finally {
      setRunning(false);
    }
  }

  return (
    <div className="space-y-4 max-w-3xl">
      <div>
        <div className="flex items-start gap-2 mb-2">
          <h2 className="text-xl font-bold text-zinc-100 flex-1">{tool.name}</h2>
          {isWriteTool && <Badge tone="bad">writes state · gated</Badge>}
          {!isWriteTool && <Badge tone="good">read-only safe</Badge>}
        </div>
        <p className="text-sm text-zinc-400">{tool.description}</p>
      </div>

      <Card title="Arguments">
        {Object.keys(properties).length === 0 ? (
          <p className="text-xs text-ink-600">This tool takes no arguments.</p>
        ) : (
          <div className="space-y-3">
            {Object.entries(properties).map(([name, schema]) => (
              <Field
                key={name}
                name={name}
                schema={schema as any}
                required={required.has(name)}
                value={values[name]}
                onChange={(v) => setValues(prev => ({ ...prev, [name]: v }))}
              />
            ))}
          </div>
        )}
        <div className="flex items-center mt-4 gap-3">
          <button
            onClick={run}
            disabled={running}
            className="px-3 py-1.5 rounded bg-accent-500 text-ink-950 text-xs font-semibold hover:bg-accent-600 disabled:opacity-50"
          >
            {running ? 'Calling…' : 'Call tool'}
          </button>
          {latencyMs !== null && <Badge tone="neutral">{latencyMs} ms round trip</Badge>}
        </div>
      </Card>

      {error && (
        <Card title="Error">
          <div className="text-xs text-bad font-mono">{error}</div>
          <div className="text-[11px] text-ink-600 mt-2">
            Errors carry a structured JSON-RPC error code (see paper §IV-I).
            Codes in <code>-32100..-32199</code> are transient (retry); <code>-32200..-32299</code> are permanent.
          </div>
        </Card>
      )}

      {result && (
        <Card title="Result" right={result.isError && <Badge tone="bad">isError=true</Badge>}>
          <ResultView content={result.content ?? []} />
        </Card>
      )}

      <Card title="JSON Schema (verbatim from tools/list)" dense>
        <pre className="text-[10.5px] leading-snug p-3 text-zinc-400 overflow-auto scrollbar-thin max-h-80">
          {JSON.stringify(tool.inputSchema, null, 2)}
        </pre>
      </Card>
    </div>
  );
}

function Field({ name, schema, required, value, onChange }: {
  name: string; schema: any; required: boolean; value: any;
  onChange: (v: any) => void;
}) {
  const type = schema.type;
  const description = schema.description as string | undefined;
  const def = schema.default;
  const enumValues = schema.enum as string[] | undefined;

  return (
    <label className="block">
      <div className="text-xs mb-1 flex items-baseline gap-2">
        <span className="font-bold text-zinc-200">{name}</span>
        <span className="text-[10.5px] text-ink-600 lowercase">{type}{enumValues ? ` · enum` : ''}</span>
        {required && <Badge tone="warn">required</Badge>}
        {def !== undefined && <span className="text-[10.5px] text-ink-600">default {JSON.stringify(def)}</span>}
      </div>
      {description && <div className="text-[11px] text-ink-600 mb-1.5">{description}</div>}
      {enumValues ? (
        <select
          value={value ?? def ?? ''}
          onChange={e => onChange(e.target.value)}
          className="w-full bg-ink-950 border border-ink-700 rounded px-2 py-1.5 text-xs focus:outline-none focus:border-accent-500"
        >
          <option value="">—</option>
          {enumValues.map(v => <option key={v} value={v}>{v}</option>)}
        </select>
      ) : type === 'integer' || type === 'number' ? (
        <input
          type="number"
          min={schema.minimum}
          max={schema.maximum}
          value={value ?? def ?? ''}
          onChange={e => onChange(e.target.value === '' ? undefined : Number(e.target.value))}
          className="w-full bg-ink-950 border border-ink-700 rounded px-2 py-1.5 text-xs focus:outline-none focus:border-accent-500"
        />
      ) : type === 'boolean' ? (
        <select
          value={String(value ?? def ?? false)}
          onChange={e => onChange(e.target.value === 'true')}
          className="w-full bg-ink-950 border border-ink-700 rounded px-2 py-1.5 text-xs focus:outline-none focus:border-accent-500"
        >
          <option value="true">true</option>
          <option value="false">false</option>
        </select>
      ) : type === 'array' ? (
        <textarea
          value={value ? JSON.stringify(value) : ''}
          placeholder='["FIELD1","FIELD2"]  // JSON array'
          onChange={e => {
            try { onChange(JSON.parse(e.target.value)); }
            catch { onChange(e.target.value as any); }
          }}
          rows={2}
          className="w-full bg-ink-950 border border-ink-700 rounded px-2 py-1.5 text-xs font-mono focus:outline-none focus:border-accent-500 resize-none"
        />
      ) : type === 'object' ? (
        <textarea
          value={value ? JSON.stringify(value, null, 2) : ''}
          placeholder='{"KEY": "value"}'
          onChange={e => {
            try { onChange(JSON.parse(e.target.value)); }
            catch { onChange(undefined); }
          }}
          rows={4}
          className="w-full bg-ink-950 border border-ink-700 rounded px-2 py-1.5 text-xs font-mono focus:outline-none focus:border-accent-500 resize-none"
        />
      ) : (
        <input
          type="text"
          value={value ?? ''}
          onChange={e => onChange(e.target.value)}
          className="w-full bg-ink-950 border border-ink-700 rounded px-2 py-1.5 text-xs focus:outline-none focus:border-accent-500"
        />
      )}
    </label>
  );
}

function ResultView({ content }: { content: Array<{ type: string; text?: string }> }) {
  return (
    <div className="space-y-2">
      {content.map((c, i) => {
        if (c.type !== 'text' || !c.text) {
          return <pre key={i} className="text-[11px]">{JSON.stringify(c)}</pre>;
        }
        // Try to JSON-pretty-print the text payload.  All RAG/RFC/ADT tools
        // emit JSON; raw text falls through.
        let pretty: string;
        try { pretty = JSON.stringify(JSON.parse(c.text), null, 2); }
        catch { pretty = c.text; }
        return (
          <pre key={i} className="text-[11px] leading-snug p-3 bg-ink-950 border border-ink-800 rounded overflow-auto scrollbar-thin max-h-[26rem]">
            {pretty}
          </pre>
        );
      })}
    </div>
  );
}

function clean(values: Record<string, any>): any {
  const out: any = {};
  for (const [k, v] of Object.entries(values)) {
    if (v === undefined || v === '' || v === null) continue;
    out[k] = v;
  }
  return out;
}
