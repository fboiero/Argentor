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
import type { AgentChatParams, AgentChatResponse, BatchResponse, BatchTask, ClientOptions, ConnectionInfo, CreatePersonaResponse, EvaluateParams, EvaluationResult, EnterpriseReadinessReport, HealthStatus, InstallSkillResponse, ListPersonasResponse, MarketplaceEntry, PersonaConfig, ReadinessStatus, RunTaskParams, RunTaskResponse, SessionInfo, SkillDescriptor, StreamEvent, ToolResult, UsageBreakdown, WebhookProxyParams, WebhookProxyResponse } from './types';
export declare class ArgentorClient {
    private readonly baseUrl;
    private readonly headers;
    private readonly timeoutMs;
    constructor(options?: ClientOptions);
    private request;
    private requestText;
    private requestVoid;
    /**
     * Execute a single agent task.
     *
     * @example
     * ```ts
     * const res = await client.runTask({ role: 'assistant', context: 'Hello!' });
     * ```
     */
    runTask(params: RunTaskParams): Promise<RunTaskResponse>;
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
    runTaskStream(params: RunTaskParams): AsyncGenerator<StreamEvent>;
    /**
     * Submit a batch of tasks for parallel execution.
     */
    batchTasks(tasks: BatchTask[], maxConcurrent?: number): Promise<BatchResponse>;
    /**
     * Evaluate text against criteria.
     */
    evaluate(params: EvaluateParams): Promise<EvaluationResult>;
    /**
     * Send a message through the agent chat endpoint.
     */
    agentChat(params: AgentChatParams): Promise<AgentChatResponse>;
    /**
     * Get agent status.
     */
    agentStatus(): Promise<Record<string, unknown>>;
    /**
     * Create a new session.
     */
    createSession(): Promise<SessionInfo>;
    /**
     * Retrieve a session by ID.
     */
    getSession(sessionId: string): Promise<SessionInfo>;
    /**
     * List all sessions.
     */
    listSessions(): Promise<SessionInfo[]>;
    /**
     * Delete a session.
     */
    deleteSession(sessionId: string): Promise<void>;
    /**
     * List registered skills.
     */
    listSkills(): Promise<SkillDescriptor[]>;
    /**
     * Get details for a specific skill.
     */
    getSkill(name: string): Promise<SkillDescriptor>;
    /**
     * Execute a skill by name.
     */
    executeSkill(name: string, args?: Record<string, unknown>): Promise<ToolResult>;
    /**
     * Check API server health.
     */
    health(): Promise<HealthStatus>;
    /**
     * Readiness probe.
     */
    healthReady(): Promise<ReadinessStatus>;
    /**
     * Retrieve Prometheus-format metrics as a raw string.
     */
    metrics(): Promise<string>;
    /**
     * Retrieve the enterprise readiness report.
     */
    enterpriseReadiness(): Promise<EnterpriseReadinessReport>;
    /**
     * List active WebSocket connections.
     */
    listConnections(): Promise<ConnectionInfo[]>;
    /**
     * Create a new agent persona.
     */
    createPersona(tenantId: string, agentRole: string, persona: PersonaConfig): Promise<CreatePersonaResponse>;
    /**
     * List personas for a tenant.
     */
    listPersonas(tenantId: string): Promise<ListPersonasResponse>;
    /**
     * Get usage breakdown for a tenant.
     */
    getUsage(tenantId: string): Promise<UsageBreakdown>;
    /**
     * Forward a webhook event through the proxy.
     */
    webhookProxy(params: WebhookProxyParams): Promise<WebhookProxyResponse>;
    /**
     * Search the skill marketplace.
     */
    searchMarketplace(query?: string, category?: string): Promise<MarketplaceEntry[]>;
    /**
     * Install a skill from the marketplace.
     */
    installSkill(name: string): Promise<InstallSkillResponse>;
}
export default ArgentorClient;
export * from './types';
export * from './errors';
export { parseSSEStream } from './streaming';
export { query as agentQuery, querySimple as agentQuerySimple, askClaude, askOpenai, askGemini, askOllama, claudeOptions, openaiOptions, geminiOptions, ollamaOptions, } from './agent';
export type { AgentOptions, AgentEvent, McpServerConfig } from './agent';
//# sourceMappingURL=index.d.ts.map