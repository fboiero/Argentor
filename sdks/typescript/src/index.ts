/**
 * @argentor/sdk -- TypeScript client for the Argentor REST API.
 *
 * @example
 * ```ts
 * import { ArgentorClient } from '@argentor/sdk';
 *
 * const client = new ArgentorClient({ apiKey: 'sk-...' });
 * const result = await client.runTask({ role: 'assistant', context: 'Hello!' });
 * ```
 */

import { ArgentorAPIError, ArgentorConnectionError } from './errors';
import { parseSSEStream } from './streaming';
import type {
  AgentChatParams,
  AgentChatResponse,
  BatchResponse,
  BatchTask,
  ClientOptions,
  ConnectionInfo,
  CreatePersonaResponse,
  EvaluateParams,
  EvaluationResult,
  EnterpriseReadinessReport,
  HealthStatus,
  InstallSkillResponse,
  ListPersonasResponse,
  MarketplaceEntry,
  PersonaConfig,
  ReadinessStatus,
  RunTaskParams,
  RunTaskResponse,
  SessionInfo,
  SkillDescriptor,
  StreamEvent,
  ToolResult,
  UsageBreakdown,
  WebhookProxyParams,
  WebhookProxyResponse,
} from './types';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const DEFAULT_BASE_URL = 'http://localhost:8080';
const DEFAULT_TIMEOUT_MS = 60_000;

/** Build standard headers from client options. */
function buildHeaders(apiKey?: string, tenantId?: string): Record<string, string> {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  if (apiKey) headers['X-API-Key'] = apiKey;
  if (tenantId) headers['X-Tenant-ID'] = tenantId;
  return headers;
}

/** Throw an `ArgentorAPIError` when a response is not OK. */
async function handleError(resp: Response): Promise<void> {
  if (resp.ok) return;
  let body: Record<string, unknown> = {};
  try {
    body = (await resp.json()) as Record<string, unknown>;
  } catch {
    body = { detail: await resp.text().catch(() => 'Unknown error') };
  }
  const message =
    (body.detail as string) ?? (body.error as string) ?? resp.statusText ?? 'Unknown error';
  throw new ArgentorAPIError(message, resp.status, body);
}

// ============================================================================
// ArgentorClient
// ============================================================================

export class ArgentorClient {
  private readonly baseUrl: string;
  private readonly headers: Record<string, string>;
  private readonly timeoutMs: number;

  constructor(options: ClientOptions = {}) {
    this.baseUrl = (options.baseUrl ?? DEFAULT_BASE_URL).replace(/\/+$/, '');
    this.headers = buildHeaders(options.apiKey, options.tenantId);
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  }

  // -- internal fetch wrapper -----------------------------------------------

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    params?: Record<string, string>,
  ): Promise<T> {
    let url = `${this.baseUrl}${path}`;
    if (params) {
      const qs = new URLSearchParams(params).toString();
      if (qs) url += `?${qs}`;
    }

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);

    let resp: Response;
    try {
      resp = await fetch(url, {
        method,
        headers: this.headers,
        body: body !== undefined ? JSON.stringify(body) : undefined,
        signal: controller.signal,
      });
    } catch (err) {
      if (err instanceof DOMException && err.name === 'AbortError') {
        throw new (await import('./errors')).ArgentorTimeoutError(
          `Request to ${method} ${path} timed out after ${this.timeoutMs}ms`,
        );
      }
      throw new ArgentorConnectionError(
        `Failed to fetch ${method} ${path}: ${(err as Error).message}`,
      );
    } finally {
      clearTimeout(timer);
    }

    await handleError(resp);
    return resp.json() as Promise<T>;
  }

  private async requestText(method: string, path: string): Promise<string> {
    const url = `${this.baseUrl}${path}`;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);

    let resp: Response;
    try {
      resp = await fetch(url, { method, headers: this.headers, signal: controller.signal });
    } catch (err) {
      throw new ArgentorConnectionError(
        `Failed to fetch ${method} ${path}: ${(err as Error).message}`,
      );
    } finally {
      clearTimeout(timer);
    }

    await handleError(resp);
    return resp.text();
  }

  private async requestVoid(method: string, path: string): Promise<void> {
    const url = `${this.baseUrl}${path}`;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);

    let resp: Response;
    try {
      resp = await fetch(url, { method, headers: this.headers, signal: controller.signal });
    } catch (err) {
      throw new ArgentorConnectionError(
        `Failed to fetch ${method} ${path}: ${(err as Error).message}`,
      );
    } finally {
      clearTimeout(timer);
    }

    await handleError(resp);
  }

  // -- agent / task endpoints -----------------------------------------------

  /**
   * Execute a single agent task.
   *
   * @example
   * ```ts
   * const res = await client.runTask({ role: 'assistant', context: 'Hello!' });
   * ```
   */
  async runTask(params: RunTaskParams): Promise<RunTaskResponse> {
    const payload: Record<string, unknown> = {
      agent_role: params.role,
      context: params.context,
    };
    if (params.model !== undefined) payload.model = params.model;
    if (params.maxTokens !== undefined) payload.max_tokens = params.maxTokens;
    if (params.tools !== undefined) payload.tools = params.tools;

    return this.request<RunTaskResponse>('POST', '/v1/run', payload);
  }

  /**
   * Stream task results via SSE.
   *
   * @example
   * ```ts
   * for await (const event of client.runTaskStream({ role: 'assistant', context: 'Hi' })) {
   *   console.log(event);
   * }
   * ```
   */
  async *runTaskStream(params: RunTaskParams): AsyncGenerator<StreamEvent> {
    const payload: Record<string, unknown> = {
      agent_role: params.role,
      context: params.context,
    };
    if (params.model !== undefined) payload.model = params.model;
    if (params.maxTokens !== undefined) payload.max_tokens = params.maxTokens;
    if (params.tools !== undefined) payload.tools = params.tools;

    const resp = await fetch(`${this.baseUrl}/v1/run/stream`, {
      method: 'POST',
      headers: this.headers,
      body: JSON.stringify(payload),
    });

    await handleError(resp);
    yield* parseSSEStream(resp) as AsyncGenerator<StreamEvent>;
  }

  /**
   * Submit a batch of tasks for parallel execution.
   */
  async batchTasks(tasks: BatchTask[], maxConcurrent = 5): Promise<BatchResponse> {
    return this.request<BatchResponse>('POST', '/v1/batch', {
      tasks,
      max_concurrent: maxConcurrent,
    });
  }

  /**
   * Evaluate text against criteria.
   */
  async evaluate(params: EvaluateParams): Promise<EvaluationResult> {
    const payload: Record<string, unknown> = {
      response: params.text,
      context: params.context ?? '',
    };
    if (params.criteria) payload.criteria = params.criteria;
    return this.request<EvaluationResult>('POST', '/v1/evaluate', payload);
  }

  // -- agent chat -----------------------------------------------------------

  /**
   * Send a message through the agent chat endpoint.
   */
  async agentChat(params: AgentChatParams): Promise<AgentChatResponse> {
    const payload: Record<string, unknown> = { message: params.message };
    if (params.sessionId !== undefined) payload.session_id = params.sessionId;
    if (params.model !== undefined) payload.model = params.model;
    if (params.maxTokens !== undefined) payload.max_tokens = params.maxTokens;
    return this.request<AgentChatResponse>('POST', '/api/v1/agent/chat', payload);
  }

  /**
   * Get agent status.
   */
  async agentStatus(): Promise<Record<string, unknown>> {
    return this.request<Record<string, unknown>>('GET', '/api/v1/agent/status');
  }

  // -- session endpoints ----------------------------------------------------

  /**
   * Create a new session.
   */
  async createSession(): Promise<SessionInfo> {
    return this.request<SessionInfo>('POST', '/api/v1/sessions', {});
  }

  /**
   * Retrieve a session by ID.
   */
  async getSession(sessionId: string): Promise<SessionInfo> {
    return this.request<SessionInfo>('GET', `/api/v1/sessions/${encodeURIComponent(sessionId)}`);
  }

  /**
   * List all sessions.
   */
  async listSessions(): Promise<SessionInfo[]> {
    return this.request<SessionInfo[]>('GET', '/api/v1/sessions');
  }

  /**
   * Delete a session.
   */
  async deleteSession(sessionId: string): Promise<void> {
    return this.requestVoid('DELETE', `/api/v1/sessions/${encodeURIComponent(sessionId)}`);
  }

  // -- skill endpoints ------------------------------------------------------

  /**
   * List registered skills.
   */
  async listSkills(): Promise<SkillDescriptor[]> {
    return this.request<SkillDescriptor[]>('GET', '/api/v1/skills');
  }

  /**
   * Get details for a specific skill.
   */
  async getSkill(name: string): Promise<SkillDescriptor> {
    return this.request<SkillDescriptor>('GET', `/api/v1/skills/${encodeURIComponent(name)}`);
  }

  /**
   * Execute a skill by name.
   */
  async executeSkill(name: string, args: Record<string, unknown> = {}): Promise<ToolResult> {
    return this.request<ToolResult>(
      'POST',
      `/api/v1/skills/${encodeURIComponent(name)}/execute`,
      { arguments: args },
    );
  }

  // -- health & metrics -----------------------------------------------------

  /**
   * Check API server health.
   */
  async health(): Promise<HealthStatus> {
    return this.request<HealthStatus>('GET', '/health');
  }

  /**
   * Readiness probe.
   */
  async healthReady(): Promise<ReadinessStatus> {
    return this.request<ReadinessStatus>('GET', '/health/ready');
  }

  /**
   * Retrieve Prometheus-format metrics as a raw string.
   */
  async metrics(): Promise<string> {
    return this.requestText('GET', '/api/v1/metrics');
  }

  /**
   * Retrieve the enterprise readiness report.
   */
  async enterpriseReadiness(): Promise<EnterpriseReadinessReport> {
    return this.request<EnterpriseReadinessReport>('GET', '/api/v1/enterprise/readiness');
  }

  // -- connections ----------------------------------------------------------

  /**
   * List active WebSocket connections.
   */
  async listConnections(): Promise<ConnectionInfo[]> {
    return this.request<ConnectionInfo[]>('GET', '/api/v1/connections');
  }

  // -- personas -------------------------------------------------------------

  /**
   * Create a new agent persona.
   */
  async createPersona(
    tenantId: string,
    agentRole: string,
    persona: PersonaConfig,
  ): Promise<CreatePersonaResponse> {
    return this.request<CreatePersonaResponse>('POST', '/v1/personas', {
      tenant_id: tenantId,
      agent_role: agentRole,
      persona,
    });
  }

  /**
   * List personas for a tenant.
   */
  async listPersonas(tenantId: string): Promise<ListPersonasResponse> {
    return this.request<ListPersonasResponse>('GET', '/v1/personas', {
      tenant_id: tenantId,
    });
  }

  // -- usage ----------------------------------------------------------------

  /**
   * Get usage breakdown for a tenant.
   */
  async getUsage(tenantId: string): Promise<UsageBreakdown> {
    return this.request<UsageBreakdown>(
      'GET',
      `/v1/usage/${encodeURIComponent(tenantId)}`,
    );
  }

  // -- webhooks -------------------------------------------------------------

  /**
   * Forward a webhook event through the proxy.
   */
  async webhookProxy(params: WebhookProxyParams): Promise<WebhookProxyResponse> {
    const payload: Record<string, unknown> = { event: params.event, data: params.data };
    if (params.source) payload.source = params.source;
    if (params.secret) payload.secret = params.secret;
    return this.request<WebhookProxyResponse>('POST', '/v1/webhooks/proxy', payload);
  }

  // -- marketplace ----------------------------------------------------------

  /**
   * Search the skill marketplace.
   */
  async searchMarketplace(query?: string, category?: string): Promise<MarketplaceEntry[]> {
    const params: Record<string, string> = {};
    if (query) params.q = query;
    if (category) params.category = category;
    return this.request<MarketplaceEntry[]>('GET', '/v1/marketplace/search', undefined, params);
  }

  /**
   * Install a skill from the marketplace.
   */
  async installSkill(name: string): Promise<InstallSkillResponse> {
    return this.request<InstallSkillResponse>('POST', '/v1/marketplace/install', { name });
  }
}

export default ArgentorClient;

// Re-export everything consumers might need.
export * from './types';
export * from './errors';
export { parseSSEStream } from './streaming';

// Agent SDK -- subprocess-based agentic execution
export {
  query as agentQuery,
  querySimple as agentQuerySimple,
  askClaude,
  askOpenai,
  askGemini,
  askOllama,
  claudeOptions,
  openaiOptions,
  geminiOptions,
  ollamaOptions,
} from './agent';
export type { AgentOptions, AgentEvent, McpServerConfig } from './agent';
