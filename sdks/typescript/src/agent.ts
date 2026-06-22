/**
 * Argentor Agent SDK -- wrap the argentor CLI for agentic execution.
 *
 * Like Claude Agent SDK wraps Claude Code, this wraps the `argentor` binary
 * and communicates via NDJSON over stdin/stdout.
 *
 * @example
 * ```ts
 * import { query, AgentOptions } from 'argentor-sdk/agent';
 *
 * for await (const event of query('Fix the bug in auth.py', {
 *   provider: 'claude',
 *   model: 'claude-sonnet-4-20250514',
 *   apiKey: 'sk-...',
 * })) {
 *   console.log(event);
 * }
 * ```
 */

import { spawn, type ChildProcess } from 'node:child_process';
import { createInterface } from 'node:readline';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface McpServerConfig {
  name: string;
  command: string;
  args: string[];
}

export interface AgentOptions {
  /** LLM provider: "claude", "openai", "gemini", "ollama". */
  provider?: string;
  /** Model identifier (e.g. "claude-sonnet-4-20250514", "gpt-4o"). */
  model?: string;
  /** API key for the chosen provider. */
  apiKey?: string;
  /** Optional system prompt override. */
  systemPrompt?: string;
  /** Maximum agentic turns before stopping. */
  maxTurns?: number;
  /** Sampling temperature. */
  temperature?: number;
  /** Tool / skill names available to the agent. `undefined` = builtins. */
  tools?: string[];
  /** Permission mode: "default", "strict", "permissive", "plan". */
  permissionMode?: 'default' | 'strict' | 'permissive' | 'plan';
  /** Working directory for the agent subprocess. */
  workingDirectory?: string;
  /** MCP server configurations to attach. */
  mcpServers?: McpServerConfig[];
  /** Whether to include intermediate streaming tokens. */
  includeStreaming?: boolean;
  /** Explicit path to the argentor binary. */
  argentorBinary?: string;
}

export interface AgentEvent {
  type:
    | 'system'
    | 'assistant'
    | 'tool_use'
    | 'tool_result'
    | 'stream'
    | 'result'
    | 'error'
    | 'guardrail';
  [key: string]: unknown;
}

// ---------------------------------------------------------------------------
// Provider presets
// ---------------------------------------------------------------------------

/** Preset for Anthropic Claude models. */
export function claudeOptions(apiKey: string, overrides?: Partial<AgentOptions>): AgentOptions {
  return { provider: 'claude', model: 'claude-sonnet-4-20250514', apiKey, ...overrides };
}

/** Preset for OpenAI models. */
export function openaiOptions(apiKey: string, overrides?: Partial<AgentOptions>): AgentOptions {
  return { provider: 'openai', model: 'gpt-4o', apiKey, ...overrides };
}

/** Preset for Google Gemini models. */
export function geminiOptions(apiKey: string, overrides?: Partial<AgentOptions>): AgentOptions {
  return { provider: 'gemini', model: 'gemini-2.0-flash', apiKey, ...overrides };
}

/** Preset for local Ollama models (no API key required). */
export function ollamaOptions(model = 'llama3', overrides?: Partial<AgentOptions>): AgentOptions {
  return { provider: 'ollama', model, apiKey: '', ...overrides };
}

// ---------------------------------------------------------------------------
// Binary resolution
// ---------------------------------------------------------------------------

function findArgentorBinary(customPath?: string): string {
  if (customPath) return customPath;

  // Check common Cargo build paths and system paths
  const candidates = [
    resolve('target/release/argentor'),
    resolve('target/debug/argentor'),
    '/usr/local/bin/argentor',
  ];

  for (const candidate of candidates) {
    if (existsSync(candidate)) return candidate;
  }

  // Fall back to bare name and let the OS resolve via PATH
  return 'argentor';
}

// ---------------------------------------------------------------------------
// Core query function
// ---------------------------------------------------------------------------

/**
 * Run an agent query and yield events as they arrive.
 *
 * Spawns `argentor --headless` as a child process, sends init + query
 * messages over stdin (NDJSON), and yields parsed `AgentEvent` objects
 * from the stdout NDJSON stream.
 *
 * @example
 * ```ts
 * for await (const event of query('Explain this codebase', claudeOptions('sk-...'))) {
 *   if (event.type === 'assistant') console.log(event.text);
 *   if (event.type === 'result') console.log('Done:', event.text ?? event.output);
 * }
 * ```
 */
export async function* query(
  prompt: string,
  options: AgentOptions = {},
): AsyncGenerator<AgentEvent> {
  const binary = findArgentorBinary(options.argentorBinary);

  const child: ChildProcess = spawn(binary, ['--headless'], {
    stdio: ['pipe', 'pipe', 'pipe'],
    cwd: options.workingDirectory,
  });

  if (!child.stdin || !child.stdout) {
    throw new Error('Failed to open stdin/stdout on argentor subprocess');
  }

  // ---- send init message --------------------------------------------------
  const initMsg = JSON.stringify({
    type: 'init',
    provider: options.provider ?? 'claude',
    model: options.model ?? 'claude-sonnet-4-20250514',
    api_key: options.apiKey ?? '',
    system_prompt: options.systemPrompt ?? null,
    max_turns: options.maxTurns ?? 10,
    temperature: options.temperature ?? 0.7,
    tools: options.tools ?? null,
    permission_mode: options.permissionMode ?? 'default',
    mcp_servers: options.mcpServers ?? null,
    working_directory: options.workingDirectory ?? null,
  });
  child.stdin.write(initMsg + '\n');

  // ---- send query message -------------------------------------------------
  const queryMsg = JSON.stringify({
    type: 'query',
    prompt,
    include_streaming: options.includeStreaming ?? false,
  });
  child.stdin.write(queryMsg + '\n');

  // ---- read NDJSON from stdout --------------------------------------------
  const rl = createInterface({ input: child.stdout });

  for await (const line of rl) {
    const trimmed = line.trim();
    if (!trimmed) continue;

    let data: Record<string, unknown>;
    try {
      data = JSON.parse(trimmed) as Record<string, unknown>;
    } catch {
      continue; // skip non-JSON lines
    }

    const event: AgentEvent = { ...data, type: (data.type as AgentEvent['type']) ?? 'unknown' };
    yield event;

    if (event.type === 'result' || event.type === 'error') {
      break;
    }
  }

  // ---- cleanup ------------------------------------------------------------
  child.stdin.end();
  await new Promise<void>((resolve) => {
    child.on('close', () => resolve());
    // If already exited, resolve immediately
    if (child.exitCode !== null) resolve();
  });
}

// ---------------------------------------------------------------------------
// Simple query (returns final text only)
// ---------------------------------------------------------------------------

/**
 * Run a query and return just the final output string.
 *
 * Convenience wrapper around {@link query} for callers who only need
 * the final result.
 *
 * @throws Error if the agent returns an error event.
 */
export async function querySimple(prompt: string, options?: AgentOptions): Promise<string> {
  for await (const event of query(prompt, options)) {
    if (event.type === 'result') {
      return (event.text as string) ?? (event.output as string) ?? '';
    }
    if (event.type === 'error') {
      throw new Error(`Agent error: ${(event.message as string) ?? 'Unknown error'}`);
    }
  }
  return '';
}

// ---------------------------------------------------------------------------
// Convenience one-liners
// ---------------------------------------------------------------------------

/** Run a prompt with Anthropic Claude and return the final text. */
export async function askClaude(prompt: string, apiKey: string): Promise<string> {
  return querySimple(prompt, claudeOptions(apiKey));
}

/** Run a prompt with OpenAI and return the final text. */
export async function askOpenai(prompt: string, apiKey: string): Promise<string> {
  return querySimple(prompt, openaiOptions(apiKey));
}

/** Run a prompt with Google Gemini and return the final text. */
export async function askGemini(prompt: string, apiKey: string): Promise<string> {
  return querySimple(prompt, geminiOptions(apiKey));
}

/** Run a prompt with a local Ollama model and return the final text. */
export async function askOllama(prompt: string, model = 'llama3'): Promise<string> {
  return querySimple(prompt, ollamaOptions(model));
}
