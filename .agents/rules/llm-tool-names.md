---
paths:
  - apps/luis/**
---

# LLM-Facing Tool Names

Every tool name exposed to the model (the `name` field of an `AgentTool`, and
any dynamically registered MCP tool name) MUST match `^[a-zA-Z0-9_-]{1,128}$`.

- Use `snake_case`: `report_start`, `graph_search`, `report_list_templates`.
- Never use dots or other punctuation. Anthropic rejects the whole request with
  a 400 (`tools.N.custom.name: String should match pattern ...`) when any tool
  name violates the pattern, which kills every turn in the session — the model
  never runs and assistant messages come back empty.
- Dotted names are fine for *commands* on the api-rs/agentd surface (e.g. the
  `report.start` command, `chat.submit_turn`); the constraint applies only to
  tool names sent to the LLM provider.
- Guard new toolsets with a name-pattern assertion in the toolset's unit tests
  (see `apps/luis/src/tools.test.ts`).
