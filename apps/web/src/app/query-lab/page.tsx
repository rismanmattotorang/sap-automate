'use client';

import { useEffect, useMemo, useState } from 'react';
import { callTool, parseToolJson, initialize } from '@/lib/mcp';
import { Badge, Card, PageHeader } from '@/components/badge';

const DOMAINS = ['all', 'sap_help', 'abap', 'bpmn', 'leanix'] as const;
type Domain = (typeof DOMAINS)[number];

interface Hit { uri: string; title: string; snippet: string; score: number; }
interface Layer { name: 'docs'; hits: Hit[]; }

const SUGGESTED: string[] = [
  'period close FAGLFLEXA reconciliation',
  'BAPI_ACC_DOCUMENT_POST journal',
  'movement type 101 goods receipt',
  'where is the BAPI documented for sales order creation',
  'how does the FI period close interact with foreign currency',
];

export default function QueryLab() {
  const [query, setQuery] = useState(SUGGESTED[0]);
  const [domain, setDomain] = useState<Domain>('all');
  const [topK, setTopK] = useState(5);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [docsHits, setDocsHits] = useState<Hit[] | null>(null);
  const [latencyMs, setLatencyMs] = useState<number | null>(null);
  const [serverReady, setServerReady] = useState(false);

  useEffect(() => {
    initialize().then(() => setServerReady(true)).catch((e) => setError(e.message));
  }, []);

  async function runQuery() {
    setLoading(true); setError(null); setDocsHits(null); setLatencyMs(null);
    const t0 = performance.now();
    try {
      const result = await callTool('sap.docs.search', { query, top_k: topK, domain });
      const parsed = parseToolJson<{ hits: Hit[] }>(result);
      setDocsHits(parsed?.hits ?? []);
      setLatencyMs(Math.round(performance.now() - t0));
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  }

  return (
    <>
      <PageHeader
        title="Query Lab"
        subtitle="Type a query, see dense + sparse + RRF + reranked results with layer timings. Nothing else has this."
        right={
          <Badge tone={serverReady ? 'good' : 'warn'}>
            {serverReady ? 'connected' : 'connecting'}
          </Badge>
        }
      />

      <div className="p-6 space-y-6 max-w-[1500px]">
        <Card title="Query">
          <div className="space-y-3">
            <textarea
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              rows={2}
              className="w-full bg-ink-950 border border-ink-700 rounded px-3 py-2 font-mono text-sm focus:border-accent-500 focus:outline-none resize-none"
            />
            <div className="flex flex-wrap gap-2">
              {SUGGESTED.map(s => (
                <button
                  key={s}
                  onClick={() => setQuery(s)}
                  className="text-[11px] px-2 py-1 bg-ink-800 hover:bg-ink-700 rounded text-zinc-400 hover:text-zinc-200"
                >
                  {s}
                </button>
              ))}
            </div>
            <div className="flex items-center gap-4">
              <label className="text-xs flex items-center gap-2">
                <span className="text-zinc-400">domain</span>
                <select
                  value={domain}
                  onChange={(e) => setDomain(e.target.value as Domain)}
                  className="bg-ink-950 border border-ink-700 rounded px-2 py-1 text-xs"
                >
                  {DOMAINS.map(d => <option key={d} value={d}>{d}</option>)}
                </select>
              </label>
              <label className="text-xs flex items-center gap-2">
                <span className="text-zinc-400">top_k</span>
                <input
                  type="number" min={1} max={20} value={topK}
                  onChange={(e) => setTopK(Number(e.target.value))}
                  className="bg-ink-950 border border-ink-700 rounded w-16 px-2 py-1 text-xs"
                />
              </label>
              <button
                onClick={runQuery}
                disabled={loading || !query.trim()}
                className="ml-auto px-3 py-1.5 rounded bg-accent-500 text-ink-950 text-xs font-semibold hover:bg-accent-600 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {loading ? 'Searching…' : 'Run hybrid search'}
              </button>
            </div>
            {error && (
              <div className="text-xs text-bad bg-bad/10 border border-bad/30 rounded px-2 py-1.5">
                {error}
              </div>
            )}
          </div>
        </Card>

        {latencyMs !== null && (
          <Card title="Round trip" dense>
            <div className="px-3 py-2 flex items-center gap-6 text-xs">
              <div>
                <span className="text-zinc-400">total round-trip </span>
                <span className="font-bold text-zinc-100">{latencyMs} ms</span>
                <span className="text-ink-600"> (browser → /api/mcp → server → reranked → browser)</span>
              </div>
              <div className="ml-auto">
                <Badge tone="good">P95 gate 80 ms server-side</Badge>
              </div>
            </div>
          </Card>
        )}

        <Card title="Reranked results (after hybrid + RRF + cross-encoder rerank)">
          {!docsHits && <p className="text-xs text-ink-600 px-3 py-4">Run a query to see results.</p>}
          {docsHits && docsHits.length === 0 && (
            <p className="text-xs text-ink-600 px-3 py-4">No hits.</p>
          )}
          <ul className="space-y-3">
            {docsHits?.map((h, i) => (
              <li key={i} className="border border-ink-800 rounded-md p-3 bg-ink-950">
                <div className="flex items-start gap-3 mb-2">
                  <span className="text-accent-500 font-bold text-sm">#{i + 1}</span>
                  <div className="flex-1 min-w-0">
                    <div className="font-semibold text-zinc-100 truncate">{h.title}</div>
                    <CitationChip uri={h.uri} />
                  </div>
                  <Badge tone="accent">score {h.score.toFixed(3)}</Badge>
                </div>
                <p className="text-xs text-zinc-400 leading-relaxed pl-7">{h.snippet}</p>
              </li>
            ))}
          </ul>
        </Card>

        <Card title="What you're actually seeing">
          <div className="text-xs text-zinc-400 leading-relaxed space-y-2">
            <p>
              The server's <code className="text-accent-500">sap.docs.search</code> tool runs the full Phase 3 pipeline:
            </p>
            <ol className="list-decimal pl-5 space-y-1">
              <li><b>Dense retrieval</b> — token-bag mock embedder (replaceable with text-embedding-3-large or bge-m3) → cosine over chunks.</li>
              <li><b>Sparse retrieval</b> — proper BM25 (k1=1.5, b=0.75) with SAP-identifier-preserving tokeniser.</li>
              <li><b>RRF fusion</b> — Reciprocal Rank Fusion with k=60. Hits in both rankings get rewarded.</li>
              <li><b>Cross-encoder rerank</b> — MockReranker boosts exact-identifier matches (BAPI_*, T001*, etc.). ONNX cross-encoder slot for Phase 7.</li>
            </ol>
            <p className="text-ink-600 pt-1">
              The Operations tab shows the per-layer latency breakdown live.
            </p>
          </div>
        </Card>
      </div>
    </>
  );
}

/** Maps SAP-domain URIs to their colour + label. */
function CitationChip({ uri }: { uri: string }) {
  const scheme = uri.split('://')[0];
  const tone: any = {
    'sap-help': 'good',
    'abap-obj': 'accent',
    'bpmn-proc': 'warn',
    'leanix-fs': 'neutral',
    'sap-rfc': 'accent',
    'sap-table': 'accent',
  }[scheme] || 'neutral';
  return (
    <div className="mt-0.5 flex items-center gap-2">
      <Badge tone={tone}>{scheme}</Badge>
      <span className="text-[11px] text-ink-600 truncate">{uri}</span>
    </div>
  );
}
