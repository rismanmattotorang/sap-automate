'use client';

import { useEffect, useState } from 'react';
import { initialize, listResources, readResource, Resource } from '@/lib/mcp';
import { Badge, Card, PageHeader } from '@/components/badge';

export default function Resources() {
  const [resources, setResources] = useState<Resource[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    initialize().then(() => listResources()).then(r => setResources(r.resources));
  }, []);

  async function open(uri: string) {
    setSelected(uri); setLoading(true); setError(null);
    try {
      const r = await readResource(uri);
      const txt = r.contents.map(c => c.text ?? '(binary)').join('\n');
      setContent(txt);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  }

  const grouped = groupByScheme(resources);

  return (
    <>
      <PageHeader
        title="Resources"
        subtitle="Read-only artefacts addressable by URI. Each tool result's citation chip points to one of these."
        right={<Badge tone="neutral">{resources.length} resources</Badge>}
      />

      <div className="flex h-[calc(100vh-73px)]">
        <div className="w-80 border-r border-ink-800 overflow-y-auto scrollbar-thin p-3 space-y-3">
          {Object.entries(grouped).map(([scheme, rs]) => (
            <div key={scheme}>
              <div className="text-[10.5px] uppercase text-ink-600 tracking-wide mb-1">{scheme} · {rs.length}</div>
              <ul className="space-y-0.5">
                {rs.map(r => (
                  <li key={r.uri}>
                    <button
                      onClick={() => open(r.uri)}
                      className={`w-full text-left text-[11.5px] rounded px-2 py-1.5 transition ${
                        r.uri === selected
                          ? 'bg-ink-800 text-accent-500'
                          : 'hover:bg-ink-900 text-zinc-300'
                      }`}
                    >
                      <div className="font-semibold truncate">{r.name}</div>
                      <div className="text-[10.5px] text-ink-600 truncate">{r.uri}</div>
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>

        <div className="flex-1 overflow-y-auto p-6 space-y-3">
          {!selected && <p className="text-xs text-ink-600">Select a resource to preview.</p>}
          {selected && (
            <>
              <div className="flex items-center gap-2 mb-2">
                <span className="text-xs font-mono text-accent-500">{selected}</span>
                {loading && <Badge tone="warn">loading</Badge>}
              </div>
              {error && <div className="text-xs text-bad">{error}</div>}
              {content && (
                <pre className="text-[11.5px] leading-relaxed bg-ink-900 border border-ink-800 rounded p-3 overflow-auto scrollbar-thin max-h-[80vh]">
                  {content}
                </pre>
              )}
            </>
          )}
        </div>
      </div>
    </>
  );
}

function groupByScheme(rs: Resource[]): Record<string, Resource[]> {
  const out: Record<string, Resource[]> = {};
  for (const r of rs) {
    const scheme = r.uri.split('://')[0];
    (out[scheme] ??= []).push(r);
  }
  for (const k of Object.keys(out)) out[k].sort((a, b) => a.uri.localeCompare(b.uri));
  return out;
}
