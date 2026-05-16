# @argentor/sdk v1.4.5

TypeScript SDK client for the [Argentor](https://github.com/fboiero/Agentor) AI agent framework REST API.

## Installation

```bash
npm install @argentor/sdk
```

## Quick Start

```typescript
import { ArgentorClient } from '@argentor/sdk';

const client = new ArgentorClient({
  baseUrl: 'http://localhost:8080',
  apiKey: 'your-api-key',
});

// Run a task
const result = await client.runTask({
  role: 'code_reviewer',
  context: 'Review the following pull request...',
});
console.log(result);

// Stream results via SSE
for await (const event of client.runTaskStream({
  role: 'assistant',
  context: 'Explain how Argentor works',
})) {
  process.stdout.write(JSON.stringify(event));
}

// Batch execution
const batch = await client.batchTasks(
  [
    { agent_role: 'analyst', context: 'Analyze Q1 sales data' },
    { agent_role: 'analyst', context: 'Analyze Q2 sales data' },
  ],
  5,
);
console.log(batch);

// Evaluate a response
const evaluation = await client.evaluate({
  text: 'The code looks clean and follows best practices.',
  context: 'Code review task',
  criteria: ['accuracy', 'completeness', 'helpfulness'],
});
console.log(evaluation);

// List skills
const skills = await client.listSkills();
for (const skill of skills) {
  console.log(skill.name);
}

// Execute a skill
const skillResult = await client.executeSkill('echo', { text: 'Hello!' });
console.log(skillResult);

// Health check
const health = await client.health();
console.log(health);
```

## Agent SDK (subprocess wrapper)

The agent module wraps the `argentor` CLI binary as a subprocess,
similar to how Claude Agent SDK wraps Claude Code.  It communicates
via NDJSON over stdin/stdout and works with any LLM provider.

```typescript
import { agentQuery, claudeOptions } from '@argentor/sdk';

// Stream events from the agent
for await (const event of agentQuery('What files are in this directory?', claudeOptions('sk-...'))) {
  if (event.type === 'assistant') console.log(event.text);
  if (event.type === 'tool_use') console.log(`[tool] ${event.name}`);
  if (event.type === 'result') console.log('Done:', event.text ?? event.output);
}
```

### One-liner convenience functions

```typescript
import { askClaude, askOpenai, askGemini, askOllama } from '@argentor/sdk';

const result = await askClaude('Explain Argentor', 'sk-...');
const result2 = await askOpenai('Explain Argentor', 'sk-...');
const result3 = await askGemini('Explain Argentor', 'AIza...');
const result4 = await askOllama('Explain Argentor', 'llama3');
```

### Provider presets

```typescript
import { claudeOptions, openaiOptions, geminiOptions, ollamaOptions } from '@argentor/sdk';

claudeOptions('sk-...');              // Anthropic Claude
openaiOptions('sk-...');              // OpenAI GPT-4o
geminiOptions('AIza...');             // Google Gemini
ollamaOptions('llama3');              // Local Ollama (no key)
```

### AgentOptions

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `provider` | `string` | `"claude"` | LLM provider |
| `model` | `string` | `"claude-sonnet-4-20250514"` | Model name |
| `apiKey` | `string` | `""` | API key |
| `systemPrompt` | `string` | - | System prompt override |
| `maxTurns` | `number` | `10` | Max agentic turns |
| `temperature` | `number` | `0.7` | Sampling temperature |
| `tools` | `string[]` | - | Tool names (undefined = builtins) |
| `permissionMode` | `string` | `"default"` | `"default"`, `"strict"`, `"permissive"`, `"plan"` |
| `workingDirectory` | `string` | - | Working directory |
| `mcpServers` | `McpServerConfig[]` | - | MCP server configs |
| `argentorBinary` | `string` | - | Path to binary (auto-detected if omitted) |

## Error Handling

```typescript
import { ArgentorClient, ArgentorAPIError, ArgentorConnectionError } from '@argentor/sdk';

const client = new ArgentorClient({ baseUrl: 'http://localhost:8080' });

try {
  const result = await client.runTask({ role: 'assistant', context: 'Hello' });
} catch (err) {
  if (err instanceof ArgentorAPIError) {
    console.error(`API error ${err.statusCode}: ${err.message}`);
    console.error('Response body:', err.responseBody);
  } else if (err instanceof ArgentorConnectionError) {
    console.error(`Connection failed: ${err.message}`);
  }
}
```

## API Reference

### `new ArgentorClient(options?)`

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `baseUrl` | `string` | `http://localhost:8080` | Argentor API base URL |
| `apiKey` | `string` | - | API key for `X-API-Key` header |
| `tenantId` | `string` | - | Tenant ID for `X-Tenant-ID` header |
| `timeoutMs` | `number` | `60000` | Request timeout in ms |

### Methods

| Method | Description |
|--------|-------------|
| `runTask(params)` | Execute a single agent task |
| `runTaskStream(params)` | Stream task results via SSE |
| `batchTasks(tasks, maxConcurrent?)` | Submit a batch of tasks |
| `evaluate(params)` | Evaluate text against criteria |
| `agentChat(params)` | Send a message via agent chat |
| `agentStatus()` | Get agent status |
| `createSession()` | Create a new session |
| `getSession(id)` | Retrieve a session |
| `listSessions()` | List all sessions |
| `deleteSession(id)` | Delete a session |
| `listSkills()` | List registered skills |
| `getSkill(name)` | Get skill details |
| `executeSkill(name, args?)` | Execute a skill |
| `health()` | Check API health |
| `healthReady()` | Readiness probe |
| `metrics()` | Get Prometheus metrics (string) |
| `listConnections()` | List WebSocket connections |
| `createPersona(tenantId, agentRole, persona)` | Create persona |
| `listPersonas(tenantId)` | List tenant personas |
| `getUsage(tenantId)` | Get usage breakdown |
| `webhookProxy(params)` | Forward a webhook event |
| `searchMarketplace(query?, category?)` | Search skill marketplace |
| `installSkill(name)` | Install from marketplace |

## Requirements

- Node.js >= 18 (uses native `fetch`)
- TypeScript >= 5.4 (for development)

## License

AGPL-3.0-only
