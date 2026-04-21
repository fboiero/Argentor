# Argentor Observability Dashboard

A minimal, zero-dependency, single-page observability dashboard for live agent monitoring.

## How to run

1. Start the Argentor gateway:

   ```bash
   cargo run --bin argentor -- serve
   ```

   The gateway listens on `http://localhost:8080` by default.

2. Open the dashboard in a browser:

   ```
   http://localhost:8080/dashboard
   ```

   This serves the embedded control plane dashboard built into the gateway binary.

3. **Standalone observability dashboard** (no build step, no npm):

   Open `dashboard/observe.html` directly from the repository root in any modern browser:

   ```bash
   open dashboard/observe.html
   # or on Linux:
   xdg-open dashboard/observe.html
   ```

   The page auto-connects to `ws://localhost:8080/ws`. Use the URL field in the nav bar to connect to a different gateway instance.

## Features

| Feature | Description |
|---------|-------------|
| WebSocket connection | Auto-connects and auto-reconnects with exponential backoff |
| Active sessions | Real-time count of sessions not yet in "done" state |
| Agent timeline | Chronological log of agent turns, tool calls, and completions |
| Tool call log | Scrollable table of all tool invocations with inputs |
| Token tracker | Estimated token usage derived from message lengths and streaming events |
| Cost tracker | Running cost estimate using claude-3-5-sonnet pricing by default |
| Status indicators | Per-session state: idle / thinking / tool-calling / done |

## Files

| File | Description |
|------|-------------|
| `dashboard/observe.html` | Standalone SPA — open directly in a browser, no build step |
| `dashboard/api.js` | ES-module WebSocket client (`AgentorWsClient`) — importable from any page |
| `crates/argentor-gateway/dashboard.html` | Embedded control plane dashboard served at `/dashboard` |
| `crates/argentor-gateway/src/dashboard.rs` | Axum route that serves the embedded dashboard HTML |

## Connecting the dashboard to a live agent

Send a message via WebSocket to see the timeline populate:

```bash
wscat -c ws://localhost:8080/ws
> {"content": "What is 2+2?"}
```

The dashboard subscribes to the same WebSocket endpoint and renders each event as it arrives.

## Cost estimation

Default model: **claude-3-5-sonnet** (`$3.00 / 1M input, $15.00 / 1M output`).

Token counts are estimated from message lengths (~4 chars/token) unless the gateway
emits explicit `Usage` events in the stream, in which case those values are used directly.

To add a new model to the pricing table, edit the `PRICING` object at the top of
`dashboard/observe.html`.
