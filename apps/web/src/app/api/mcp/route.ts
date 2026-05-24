// Same-origin proxy to the SAP-Automate MCP server's HTTP transport.
// Keeps the browser away from CORS friction and lets us inject the bearer
// token from the server environment without leaking it to the client.

import { NextRequest, NextResponse } from 'next/server';

export const dynamic = 'force-dynamic';

const TARGET = process.env.MCP_SERVER_URL ?? 'http://127.0.0.1:3030/mcp';
const TOKEN = process.env.MCP_BEARER_TOKEN;

export async function POST(req: NextRequest) {
  const body = await req.text();
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  if (TOKEN) headers['authorization'] = `Bearer ${TOKEN}`;
  try {
    const res = await fetch(TARGET, { method: 'POST', headers, body, cache: 'no-store' });
    const text = await res.text();
    return new NextResponse(text, {
      status: res.status,
      headers: { 'content-type': res.headers.get('content-type') ?? 'application/json' },
    });
  } catch (e: any) {
    return NextResponse.json(
      { jsonrpc: '2.0', id: null, error: { code: -32001, message: `proxy failed: ${e?.message ?? e}` } },
      { status: 502 }
    );
  }
}

export async function GET() {
  return NextResponse.json({
    ok: true,
    target: TARGET,
    hint: 'POST a JSON-RPC 2.0 message to this endpoint to call the SAP-Automate MCP server.',
  });
}
