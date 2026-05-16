/**
 * Tests for the Argentor TypeScript SDK.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ArgentorClient } from '../index';
import {
  ArgentorAPIError,
  ArgentorConnectionError,
  ArgentorTimeoutError,
  ArgentorError,
} from '../errors';
import type {
  ClientOptions,
  RunTaskParams,
  RunTaskResponse,
  SessionInfo,
  SkillDescriptor,
  ToolResult,
  HealthStatus,
  ReadinessStatus,
  BatchResponse,
  EvaluationResult,
  StreamEvent,
  ConnectionInfo,
  EnterpriseReadinessReport,
  MarketplaceEntry,
  InstallSkillResponse,
  BatchTask,
  AgentChatResponse,
} from '../types';

// ---------------------------------------------------------------------------
// Client construction
// ---------------------------------------------------------------------------

describe('ArgentorClient construction', () => {
  it('uses default base URL when none provided', () => {
    const client = new ArgentorClient();
    // Access private field via any for testing
    expect((client as any).baseUrl).toBe('http://localhost:8080');
  });

  it('accepts a custom base URL', () => {
    const client = new ArgentorClient({ baseUrl: 'http://custom:9090' });
    expect((client as any).baseUrl).toBe('http://custom:9090');
  });

  it('strips trailing slashes from base URL', () => {
    const client = new ArgentorClient({ baseUrl: 'http://example.com/' });
    expect((client as any).baseUrl).toBe('http://example.com');
  });

  it('uses default timeout when none provided', () => {
    const client = new ArgentorClient();
    expect((client as any).timeoutMs).toBe(60_000);
  });

  it('accepts a custom timeout', () => {
    const client = new ArgentorClient({ timeoutMs: 5000 });
    expect((client as any).timeoutMs).toBe(5000);
  });
});

// ---------------------------------------------------------------------------
// Header building
// ---------------------------------------------------------------------------

describe('Header building', () => {
  it('always includes Content-Type', () => {
    const client = new ArgentorClient();
    expect((client as any).headers['Content-Type']).toBe('application/json');
  });

  it('includes X-API-Key when apiKey is set', () => {
    const client = new ArgentorClient({ apiKey: 'sk-test' });
    expect((client as any).headers['X-API-Key']).toBe('sk-test');
  });

  it('includes X-Tenant-ID when tenantId is set', () => {
    const client = new ArgentorClient({ tenantId: 'tenant-42' });
    expect((client as any).headers['X-Tenant-ID']).toBe('tenant-42');
  });

  it('omits X-API-Key when not provided', () => {
    const client = new ArgentorClient();
    expect((client as any).headers['X-API-Key']).toBeUndefined();
  });

  it('omits X-Tenant-ID when not provided', () => {
    const client = new ArgentorClient();
    expect((client as any).headers['X-Tenant-ID']).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// URL building
// ---------------------------------------------------------------------------

describe('URL building', () => {
  it('concatenates base URL with path', () => {
    const client = new ArgentorClient({ baseUrl: 'http://api.example.com' });
    // The request method builds URLs as `${baseUrl}${path}`
    const url = `${(client as any).baseUrl}/v1/run`;
    expect(url).toBe('http://api.example.com/v1/run');
  });

  it('builds session URL with ID', () => {
    const client = new ArgentorClient({ baseUrl: 'http://api.example.com' });
    const sessionId = 'sess-123';
    const url = `${(client as any).baseUrl}/api/v1/sessions/${encodeURIComponent(sessionId)}`;
    expect(url).toBe('http://api.example.com/api/v1/sessions/sess-123');
  });

  it('builds skill execute URL', () => {
    const client = new ArgentorClient({ baseUrl: 'http://api.example.com' });
    const name = 'echo';
    const url = `${(client as any).baseUrl}/api/v1/skills/${encodeURIComponent(name)}/execute`;
    expect(url).toBe('http://api.example.com/api/v1/skills/echo/execute');
  });

  it('encodes special characters in URL segments', () => {
    const client = new ArgentorClient({ baseUrl: 'http://api.example.com' });
    const name = 'skill with spaces';
    const url = `${(client as any).baseUrl}/api/v1/skills/${encodeURIComponent(name)}/execute`;
    expect(url).toContain('skill%20with%20spaces');
  });
});

// ---------------------------------------------------------------------------
// Error classes
// ---------------------------------------------------------------------------

describe('Error classes', () => {
  it('ArgentorError is an Error', () => {
    const err = new ArgentorError('test');
    expect(err).toBeInstanceOf(Error);
    expect(err.message).toBe('test');
    expect(err.name).toBe('ArgentorError');
  });

  it('ArgentorAPIError has statusCode and responseBody', () => {
    const err = new ArgentorAPIError('Not found', 404, { detail: 'Not found' });
    expect(err.statusCode).toBe(404);
    expect(err.responseBody.detail).toBe('Not found');
    expect(err.message).toContain('404');
    expect(err.name).toBe('ArgentorAPIError');
  });

  it('ArgentorAPIError defaults responseBody to empty object', () => {
    const err = new ArgentorAPIError('err', 500);
    expect(err.responseBody).toEqual({});
  });

  it('ArgentorConnectionError inherits from ArgentorError', () => {
    const err = new ArgentorConnectionError('refused');
    expect(err).toBeInstanceOf(ArgentorError);
    expect(err.name).toBe('ArgentorConnectionError');
  });

  it('ArgentorTimeoutError inherits from ArgentorError', () => {
    const err = new ArgentorTimeoutError('timed out');
    expect(err).toBeInstanceOf(ArgentorError);
    expect(err.name).toBe('ArgentorTimeoutError');
  });
});

// ---------------------------------------------------------------------------
// Type exports
// ---------------------------------------------------------------------------

describe('Type exports', () => {
  it('ClientOptions type is usable', () => {
    const opts: ClientOptions = { baseUrl: 'http://test', apiKey: 'k' };
    expect(opts.baseUrl).toBe('http://test');
  });

  it('RunTaskParams type is usable', () => {
    const params: RunTaskParams = { role: 'assistant', context: 'hello' };
    expect(params.role).toBe('assistant');
  });

  it('RunTaskResponse type is usable', () => {
    const resp: RunTaskResponse = { task_id: 't1', status: 'done' };
    expect(resp.task_id).toBe('t1');
  });

  it('SessionInfo type is usable', () => {
    const session: SessionInfo = { session_id: 's1' };
    expect(session.session_id).toBe('s1');
  });

  it('SkillDescriptor type is usable', () => {
    const skill: SkillDescriptor = { name: 'echo', description: 'Echo skill' };
    expect(skill.name).toBe('echo');
  });

  it('ToolResult type is usable', () => {
    const result: ToolResult = { success: true, output: 'hi' };
    expect(result.success).toBe(true);
  });

  it('HealthStatus type is usable', () => {
    const health: HealthStatus = { status: 'ok', version: '1.0.0' };
    expect(health.status).toBe('ok');
  });

  it('StreamEvent type is usable', () => {
    const ev: StreamEvent = { event: 'delta', content: 'text', done: false };
    expect(ev.content).toBe('text');
  });

  it('EnterpriseReadinessReport type is usable', () => {
    const report: EnterpriseReadinessReport = {
      version: '1.4.7',
      posture: 'ready',
      score: 86,
      runtime: {
        skills_registered: 42,
        active_connections: 0,
        active_sessions: 0,
        uptime_seconds: 60,
      },
      checks: [
        {
          id: 'rest_api',
          category: 'runtime',
          title: 'REST API mounted',
          status: 'active',
          detail: 'REST management endpoints are available.',
        },
      ],
      next_actions: ['Run the enterprise golden path smoke test.'],
    };
    expect(report.checks[0].status).toBe('active');
  });

  it('BatchTask type is usable', () => {
    const task: BatchTask = { agent_role: 'analyst', context: 'data' };
    expect(task.agent_role).toBe('analyst');
  });

  it('MarketplaceEntry type is usable', () => {
    const entry: MarketplaceEntry = { name: 'cool', version: '0.1.0', rating: 4.5 };
    expect(entry.rating).toBe(4.5);
  });
});

// ---------------------------------------------------------------------------
// Client methods with mocked fetch
// ---------------------------------------------------------------------------

describe('Client methods with mocked fetch', () => {
  let client: ArgentorClient;

  beforeEach(() => {
    client = new ArgentorClient({ baseUrl: 'http://test:8080', apiKey: 'test-key' });
  });

  it('health() calls GET /health', async () => {
    const mockResponse = { status: 'ok', version: '1.0.0' };
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        json: () => Promise.resolve(mockResponse),
      }),
    );

    const result = await client.health();
    expect(result).toEqual(mockResponse);
    expect(fetch).toHaveBeenCalledWith(
      'http://test:8080/health',
      expect.objectContaining({ method: 'GET' }),
    );

    vi.unstubAllGlobals();
  });

  it('runTask() calls POST /v1/run with correct payload', async () => {
    const mockResponse: RunTaskResponse = { task_id: 't1', status: 'completed' };
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        json: () => Promise.resolve(mockResponse),
      }),
    );

    const result = await client.runTask({ role: 'assistant', context: 'Hello' });
    expect(result.task_id).toBe('t1');

    const fetchCall = (fetch as any).mock.calls[0];
    expect(fetchCall[0]).toBe('http://test:8080/v1/run');
    const body = JSON.parse(fetchCall[1].body);
    expect(body.agent_role).toBe('assistant');
    expect(body.context).toBe('Hello');

    vi.unstubAllGlobals();
  });

  it('throws ArgentorAPIError on non-OK response', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: false,
        status: 500,
        statusText: 'Internal Server Error',
        json: () => Promise.resolve({ detail: 'Something broke' }),
      }),
    );

    await expect(client.health()).rejects.toThrow(ArgentorAPIError);

    vi.unstubAllGlobals();
  });

  it('throws ArgentorConnectionError on fetch failure', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('ECONNREFUSED')));

    await expect(client.health()).rejects.toThrow(ArgentorConnectionError);

    vi.unstubAllGlobals();
  });

  it('listSkills() calls GET /api/v1/skills', async () => {
    const mockSkills: SkillDescriptor[] = [{ name: 'echo' }, { name: 'time' }];
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        json: () => Promise.resolve(mockSkills),
      }),
    );

    const result = await client.listSkills();
    expect(result).toHaveLength(2);

    vi.unstubAllGlobals();
  });

  it('enterpriseReadiness() calls GET /api/v1/enterprise/readiness', async () => {
    const mockReport: EnterpriseReadinessReport = {
      version: '1.4.7',
      posture: 'ready',
      score: 86,
      runtime: {
        skills_registered: 42,
        active_connections: 0,
        active_sessions: 0,
        uptime_seconds: 60,
      },
      checks: [],
      next_actions: [],
    };
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        json: () => Promise.resolve(mockReport),
      }),
    );

    const result = await client.enterpriseReadiness();
    expect(result.posture).toBe('ready');
    expect(fetch).toHaveBeenCalledWith(
      'http://test:8080/api/v1/enterprise/readiness',
      expect.objectContaining({ method: 'GET' }),
    );

    vi.unstubAllGlobals();
  });
});

// ---------------------------------------------------------------------------
// parseSSEStream
// ---------------------------------------------------------------------------

describe('parseSSEStream', () => {
  it('is exported from index', async () => {
    const mod = await import('../index');
    expect(typeof mod.parseSSEStream).toBe('function');
  });
});
