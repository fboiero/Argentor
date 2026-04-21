# API Reference

## Browsing Locally

Generate and open the full API documentation with:

```bash
cargo doc --workspace --no-deps --open
```

This builds HTML docs for all 15 crates and opens them in your browser.

## docs.rs (once published)

Each crate will be available at:

```
https://docs.rs/argentor-core
https://docs.rs/argentor-security
https://docs.rs/argentor-session
https://docs.rs/argentor-skills
https://docs.rs/argentor-agent
https://docs.rs/argentor-builtins
https://docs.rs/argentor-memory
https://docs.rs/argentor-mcp
https://docs.rs/argentor-orchestrator
https://docs.rs/argentor-compliance
https://docs.rs/argentor-channels
https://docs.rs/argentor-gateway
https://docs.rs/argentor-a2a
https://docs.rs/argentor-tee
https://docs.rs/argentor-cloud
```

All crates are configured with `all-features = true` so every public API is visible.

## Key Entry Points

| What you want | Crate | Type |
|---|---|---|
| Run an agent | `argentor-agent` | `AgentRunner` |
| Add a skill | `argentor-skills` | `Skill` trait, `SkillRegistry` |
| Built-in tools | `argentor-builtins` | `register_builtins()` |
| Multi-agent | `argentor-orchestrator` | `Orchestrator` |
| MCP integration | `argentor-mcp` | `McpClient`, `McpSkill` |
| Permissions | `argentor-security` | `PermissionSet`, `AuditLog` |
| Session state | `argentor-session` | `Session` |
| Core types | `argentor-core` | `Message`, `ToolCall`, `ToolResult` |
