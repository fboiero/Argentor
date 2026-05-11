/**
 * TypeScript type definitions for the Argentor REST API.
 */
export interface ClientOptions {
    /** Base URL of the Argentor API server. Defaults to http://localhost:8080 */
    baseUrl?: string;
    /** API key for authentication (sent as X-API-Key header). */
    apiKey?: string;
    /** Tenant identifier (sent as X-Tenant-ID header). */
    tenantId?: string;
    /** Request timeout in milliseconds. Defaults to 60 000. */
    timeoutMs?: number;
}
export interface RunTaskParams {
    /** Agent role / persona (e.g. "code_reviewer"). */
    role: string;
    /** The user prompt or task description. */
    context: string;
    /** Optional model override (e.g. "gpt-4"). */
    model?: string;
    /** Maximum tokens for the response. */
    maxTokens?: number;
    /** List of tool / skill names available to the agent. */
    tools?: string[];
}
export interface RunTaskResponse {
    task_id: string;
    status: string;
    result?: string;
    tokens_used?: number;
    duration_ms?: number;
    metadata?: Record<string, unknown>;
}
export interface StreamEvent {
    event?: string;
    data?: Record<string, unknown>;
    content?: string;
    done?: boolean;
}
export interface BatchTask {
    agent_role: string;
    context: string;
    model?: string;
    max_tokens?: number;
}
export interface BatchResult {
    task_id: string;
    status: string;
    result?: string;
    error?: string;
}
export interface BatchResponse {
    batch_id: string;
    results: BatchResult[];
    total: number;
    succeeded: number;
    failed: number;
}
export interface EvaluateParams {
    /** The text to evaluate. */
    text: string;
    /** Context for the evaluation. */
    context?: string;
    /** Evaluation criteria. */
    criteria?: string[];
}
export interface CriterionScore {
    criterion: string;
    score: number;
    explanation?: string;
}
export interface EvaluationResult {
    overall_score: number;
    scores: CriterionScore[];
    summary?: string;
}
export interface AgentChatParams {
    message: string;
    sessionId?: string;
    model?: string;
    maxTokens?: number;
}
export interface AgentChatResponse {
    response?: string;
    session_id?: string;
    tokens_used?: number;
    metadata?: Record<string, unknown>;
}
export interface SessionInfo {
    session_id: string;
    created_at?: string;
    updated_at?: string;
    metadata?: Record<string, unknown>;
}
export interface SkillParameter {
    name: string;
    description?: string;
    required?: boolean;
    type?: string;
}
export interface SkillDescriptor {
    name: string;
    description?: string;
    parameters?: SkillParameter[];
    version?: string;
}
export interface ToolResult {
    success: boolean;
    output?: string;
    error?: string;
    metadata?: Record<string, unknown>;
}
export interface HealthStatus {
    status: string;
    version?: string;
    uptime_seconds?: number;
}
export interface ReadinessStatus {
    ready: boolean;
    checks?: Record<string, unknown>;
}
export type EnterpriseReadinessPosture = 'ready' | 'needs_configuration' | 'not_ready';
export type EnterpriseReadinessCheckStatus = 'active' | 'available' | 'attention';
export interface EnterpriseRuntimeSnapshot {
    skills_registered: number;
    active_connections: number;
    active_sessions: number;
    uptime_seconds: number;
}
export interface EnterpriseReadinessCheck {
    id: string;
    category: string;
    title: string;
    status: EnterpriseReadinessCheckStatus;
    detail: string;
}
export interface EnterpriseReadinessReport {
    version: string;
    posture: EnterpriseReadinessPosture;
    score: number;
    runtime: EnterpriseRuntimeSnapshot;
    checks: EnterpriseReadinessCheck[];
    next_actions: string[];
}
export interface ConnectionInfo {
    connection_id: string;
    connected_at?: string;
    metadata?: Record<string, unknown>;
}
export interface PersonaConfig {
    name: string;
    system_prompt?: string;
    temperature?: number;
    model?: string;
    tools?: string[];
    metadata?: Record<string, unknown>;
}
export interface CreatePersonaResponse {
    persona_id: string;
    tenant_id: string;
    agent_role: string;
    created_at: string;
}
export interface PersonaSummary {
    persona_id: string;
    agent_role: string;
    name: string;
    created_at: string;
}
export interface ListPersonasResponse {
    tenant_id: string;
    personas: PersonaSummary[];
}
export interface ModelUsage {
    model: string;
    input_tokens: number;
    output_tokens: number;
    total_tokens: number;
    cost_usd?: number;
}
export interface UsageBreakdown {
    tenant_id: string;
    period_start: string;
    period_end: string;
    models: ModelUsage[];
    total_tokens: number;
    total_cost_usd?: number;
}
export interface WebhookProxyParams {
    event: string;
    data: Record<string, unknown>;
    source?: string;
    secret?: string;
}
export interface WebhookProxyResponse {
    accepted: boolean;
    event_id?: string;
    message?: string;
}
export interface MarketplaceEntry {
    name: string;
    description?: string;
    version?: string;
    author?: string;
    category?: string;
    downloads?: number;
    rating?: number;
}
export interface InstallSkillResponse {
    success: boolean;
    name: string;
    version?: string;
    message?: string;
}
export interface ApiErrorBody {
    detail?: string;
    error?: string;
    [key: string]: unknown;
}
//# sourceMappingURL=types.d.ts.map