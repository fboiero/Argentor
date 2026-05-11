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
// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
const DEFAULT_BASE_URL = 'http://localhost:8080';
const DEFAULT_TIMEOUT_MS = 60_000;
/** Build standard headers from client options. */
function buildHeaders(apiKey, tenantId) {
    const headers = { 'Content-Type': 'application/json' };
    if (apiKey)
        headers['X-API-Key'] = apiKey;
    if (tenantId)
        headers['X-Tenant-ID'] = tenantId;
    return headers;
}
/** Throw an `ArgentorAPIError` when a response is not OK. */
async function handleError(resp) {
    if (resp.ok)
        return;
    let body = {};
    try {
        body = (await resp.json());
    }
    catch {
        body = { detail: await resp.text().catch(() => 'Unknown error') };
    }
    const message = body.detail ?? body.error ?? resp.statusText ?? 'Unknown error';
    throw new ArgentorAPIError(message, resp.status, body);
}
// ============================================================================
// ArgentorClient
// ============================================================================
export class ArgentorClient {
    baseUrl;
    headers;
    timeoutMs;
    constructor(options = {}) {
        this.baseUrl = (options.baseUrl ?? DEFAULT_BASE_URL).replace(/\/+$/, '');
        this.headers = buildHeaders(options.apiKey, options.tenantId);
        this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    }
    // -- internal fetch wrapper -----------------------------------------------
    async request(method, path, body, params) {
        let url = `${this.baseUrl}${path}`;
        if (params) {
            const qs = new URLSearchParams(params).toString();
            if (qs)
                url += `?${qs}`;
        }
        const controller = new AbortController();
        const timer = setTimeout(() => controller.abort(), this.timeoutMs);
        let resp;
        try {
            resp = await fetch(url, {
                method,
                headers: this.headers,
                body: body !== undefined ? JSON.stringify(body) : undefined,
                signal: controller.signal,
            });
        }
        catch (err) {
            if (err instanceof DOMException && err.name === 'AbortError') {
                throw new (await import('./errors')).ArgentorTimeoutError(`Request to ${method} ${path} timed out after ${this.timeoutMs}ms`);
            }
            throw new ArgentorConnectionError(`Failed to fetch ${method} ${path}: ${err.message}`);
        }
        finally {
            clearTimeout(timer);
        }
        await handleError(resp);
        return resp.json();
    }
    async requestText(method, path) {
        const url = `${this.baseUrl}${path}`;
        const controller = new AbortController();
        const timer = setTimeout(() => controller.abort(), this.timeoutMs);
        let resp;
        try {
            resp = await fetch(url, { method, headers: this.headers, signal: controller.signal });
        }
        catch (err) {
            throw new ArgentorConnectionError(`Failed to fetch ${method} ${path}: ${err.message}`);
        }
        finally {
            clearTimeout(timer);
        }
        await handleError(resp);
        return resp.text();
    }
    async requestVoid(method, path) {
        const url = `${this.baseUrl}${path}`;
        const controller = new AbortController();
        const timer = setTimeout(() => controller.abort(), this.timeoutMs);
        let resp;
        try {
            resp = await fetch(url, { method, headers: this.headers, signal: controller.signal });
        }
        catch (err) {
            throw new ArgentorConnectionError(`Failed to fetch ${method} ${path}: ${err.message}`);
        }
        finally {
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
    async runTask(params) {
        const payload = {
            agent_role: params.role,
            context: params.context,
        };
        if (params.model !== undefined)
            payload.model = params.model;
        if (params.maxTokens !== undefined)
            payload.max_tokens = params.maxTokens;
        if (params.tools !== undefined)
            payload.tools = params.tools;
        return this.request('POST', '/v1/run', payload);
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
    async *runTaskStream(params) {
        const payload = {
            agent_role: params.role,
            context: params.context,
        };
        if (params.model !== undefined)
            payload.model = params.model;
        if (params.maxTokens !== undefined)
            payload.max_tokens = params.maxTokens;
        if (params.tools !== undefined)
            payload.tools = params.tools;
        const resp = await fetch(`${this.baseUrl}/v1/run/stream`, {
            method: 'POST',
            headers: this.headers,
            body: JSON.stringify(payload),
        });
        await handleError(resp);
        yield* parseSSEStream(resp);
    }
    /**
     * Submit a batch of tasks for parallel execution.
     */
    async batchTasks(tasks, maxConcurrent = 5) {
        return this.request('POST', '/v1/batch', {
            tasks,
            max_concurrent: maxConcurrent,
        });
    }
    /**
     * Evaluate text against criteria.
     */
    async evaluate(params) {
        const payload = {
            response: params.text,
            context: params.context ?? '',
        };
        if (params.criteria)
            payload.criteria = params.criteria;
        return this.request('POST', '/v1/evaluate', payload);
    }
    // -- agent chat -----------------------------------------------------------
    /**
     * Send a message through the agent chat endpoint.
     */
    async agentChat(params) {
        const payload = { message: params.message };
        if (params.sessionId !== undefined)
            payload.session_id = params.sessionId;
        if (params.model !== undefined)
            payload.model = params.model;
        if (params.maxTokens !== undefined)
            payload.max_tokens = params.maxTokens;
        return this.request('POST', '/api/v1/agent/chat', payload);
    }
    /**
     * Get agent status.
     */
    async agentStatus() {
        return this.request('GET', '/api/v1/agent/status');
    }
    // -- session endpoints ----------------------------------------------------
    /**
     * Create a new session.
     */
    async createSession() {
        return this.request('POST', '/api/v1/sessions', {});
    }
    /**
     * Retrieve a session by ID.
     */
    async getSession(sessionId) {
        return this.request('GET', `/api/v1/sessions/${encodeURIComponent(sessionId)}`);
    }
    /**
     * List all sessions.
     */
    async listSessions() {
        return this.request('GET', '/api/v1/sessions');
    }
    /**
     * Delete a session.
     */
    async deleteSession(sessionId) {
        return this.requestVoid('DELETE', `/api/v1/sessions/${encodeURIComponent(sessionId)}`);
    }
    // -- skill endpoints ------------------------------------------------------
    /**
     * List registered skills.
     */
    async listSkills() {
        return this.request('GET', '/api/v1/skills');
    }
    /**
     * Get details for a specific skill.
     */
    async getSkill(name) {
        return this.request('GET', `/api/v1/skills/${encodeURIComponent(name)}`);
    }
    /**
     * Execute a skill by name.
     */
    async executeSkill(name, args = {}) {
        return this.request('POST', `/api/v1/skills/${encodeURIComponent(name)}/execute`, { arguments: args });
    }
    // -- health & metrics -----------------------------------------------------
    /**
     * Check API server health.
     */
    async health() {
        return this.request('GET', '/health');
    }
    /**
     * Readiness probe.
     */
    async healthReady() {
        return this.request('GET', '/health/ready');
    }
    /**
     * Retrieve Prometheus-format metrics as a raw string.
     */
    async metrics() {
        return this.requestText('GET', '/api/v1/metrics');
    }
    /**
     * Retrieve the enterprise readiness report.
     */
    async enterpriseReadiness() {
        return this.request('GET', '/api/v1/enterprise/readiness');
    }
    // -- connections ----------------------------------------------------------
    /**
     * List active WebSocket connections.
     */
    async listConnections() {
        return this.request('GET', '/api/v1/connections');
    }
    // -- personas -------------------------------------------------------------
    /**
     * Create a new agent persona.
     */
    async createPersona(tenantId, agentRole, persona) {
        return this.request('POST', '/v1/personas', {
            tenant_id: tenantId,
            agent_role: agentRole,
            persona,
        });
    }
    /**
     * List personas for a tenant.
     */
    async listPersonas(tenantId) {
        return this.request('GET', '/v1/personas', {
            tenant_id: tenantId,
        });
    }
    // -- usage ----------------------------------------------------------------
    /**
     * Get usage breakdown for a tenant.
     */
    async getUsage(tenantId) {
        return this.request('GET', `/v1/usage/${encodeURIComponent(tenantId)}`);
    }
    // -- webhooks -------------------------------------------------------------
    /**
     * Forward a webhook event through the proxy.
     */
    async webhookProxy(params) {
        const payload = { event: params.event, data: params.data };
        if (params.source)
            payload.source = params.source;
        if (params.secret)
            payload.secret = params.secret;
        return this.request('POST', '/v1/webhooks/proxy', payload);
    }
    // -- marketplace ----------------------------------------------------------
    /**
     * Search the skill marketplace.
     */
    async searchMarketplace(query, category) {
        const params = {};
        if (query)
            params.q = query;
        if (category)
            params.category = category;
        return this.request('GET', '/v1/marketplace/search', undefined, params);
    }
    /**
     * Install a skill from the marketplace.
     */
    async installSkill(name) {
        return this.request('POST', '/v1/marketplace/install', { name });
    }
}
export default ArgentorClient;
// Re-export everything consumers might need.
export * from './types';
export * from './errors';
export { parseSSEStream } from './streaming';
// Agent SDK -- subprocess-based agentic execution
export { query as agentQuery, querySimple as agentQuerySimple, askClaude, askOpenai, askGemini, askOllama, claudeOptions, openaiOptions, geminiOptions, ollamaOptions, } from './agent';
//# sourceMappingURL=index.js.map