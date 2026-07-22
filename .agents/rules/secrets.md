# Secrets Management

## Azure Key Vault Integration

Secrets use `kv:` prefix in `.dev.vars.example` and `.env.example` files:

```bash
# .dev.vars.example (committed)
ANTHROPIC_API_KEY=kv:anthropic-api-key

# Resolved at dev time to actual values
```

## Commands

```bash
bun run secrets:init          # Copy .dev.vars.example -> .dev.vars
bun run secrets:resolve       # Fetch secrets from Key Vault
just dev                      # Auto-resolves before starting
```

## Required Secrets

Core local secrets:
- `apps/llm-proxy/.env`: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` (the inference plane holds provider keys)
- `apps/api-rs/.env`: `WORKOS_CLIENT_ID`, `WORKOS_API_KEY`

## Adding a New Secret

1. Add to Key Vault:
   ```bash
   az keyvault secret set --vault-name kv-litvue-dev-eastus2 --name my-secret --value "..."
   ```
2. Add to the relevant `.dev.vars.example` or `.env.example`: `MY_SECRET=kv:my-secret`
3. Add to `Env` interface in worker code or app config in Rust services
4. If required, add to `REQUIRED_SECRETS` in `scripts/resolve-secrets.ts`

## Production

Use `wrangler secret put SECRET_NAME`
