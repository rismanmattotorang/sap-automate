// Lightweight MCP client used by the browser-side components.
// Wraps the same-origin /api/mcp proxy in a typed surface.

export type JsonValue = string | number | boolean | null | JsonValue[] | { [k: string]: JsonValue };

export interface ToolInputSchema {
  type: string;
  properties?: Record<string, any>;
  required?: string[];
  additionalProperties?: boolean | object;
  [k: string]: any;
}

export interface Tool {
  name: string;
  description?: string;
  inputSchema: ToolInputSchema;
}

export interface Resource {
  uri: string;
  name: string;
  description?: string;
  mimeType?: string;
}

export interface PromptArgument {
  name: string;
  description?: string;
  required?: boolean;
}

export interface Prompt {
  name: string;
  description?: string;
  arguments?: PromptArgument[];
}

export interface CallToolResult {
  content: Array<{ type: string; text?: string; data?: string; mimeType?: string }>;
  isError?: boolean;
}

export interface InitializeResult {
  protocolVersion: string;
  capabilities: any;
  serverInfo: { name: string; version: string };
  instructions?: string;
}

let nextId = 1;
function newId() { return nextId++; }

async function rpc<T = any>(method: string, params?: any): Promise<T> {
  const res = await fetch('/api/mcp', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: newId(), method, params }),
  });
  const body = await res.json();
  if (body.error) throw new McpError(body.error.code, body.error.message, body.error.data);
  return body.result as T;
}

export class McpError extends Error {
  constructor(public code: number, message: string, public data?: any) {
    super(message);
    this.name = 'McpError';
  }
  /** Maps to the named codes from mcp_core::error::ErrorCode + rfc/adt ranges. */
  category(): 'transient' | 'permanent' | 'degraded' | 'protocol' {
    if (this.code <= -32100 && this.code >= -32199) return 'transient';
    if (this.code <= -32200 && this.code >= -32299) return 'permanent';
    if (this.code <= -32300 && this.code >= -32399) return 'degraded';
    return 'protocol';
  }
}

export async function initialize(): Promise<InitializeResult> {
  return rpc<InitializeResult>('initialize', {
    protocolVersion: '2025-06-18',
    capabilities: {},
    clientInfo: { name: 'sap-automate-web', version: '0.1.0' },
  });
}

export async function listTools(): Promise<{ tools: Tool[] }> {
  return rpc('tools/list');
}

export async function callTool(name: string, args: any): Promise<CallToolResult> {
  return rpc('tools/call', { name, arguments: args });
}

export async function listResources(): Promise<{ resources: Resource[] }> {
  return rpc('resources/list');
}

export async function readResource(uri: string): Promise<{ contents: Array<{ uri: string; text?: string; mimeType?: string }> }> {
  return rpc('resources/read', { uri });
}

export async function listPrompts(): Promise<{ prompts: Prompt[] }> {
  return rpc('prompts/list');
}

export async function getPrompt(name: string, args?: Record<string, string>) {
  return rpc('prompts/get', { name, arguments: args });
}

/** Helper: parse the JSON body that SAP-Automate's RAG tools emit. */
export function parseToolJson<T = any>(result: CallToolResult): T | null {
  const first = result.content?.[0];
  if (!first?.text) return null;
  try { return JSON.parse(first.text) as T; }
  catch { return null; }
}
