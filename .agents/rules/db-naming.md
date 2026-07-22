# Database Naming Conventions

## organization_id, never org_id

All foreign keys referencing the `organizations` table use `organization_id`. The abbreviated form `org_id` was fully removed in a prior migration and is never present in the schema, queries, or application code. Never introduce it.

Verified as of the graph-native rewrite (Phase 1+):

```bash
grep -rc "org_id\b" packages/db/ apps/api-rs/src/ scripts/
# → 0 matches across the board
```

## General Rules

- Column names match the referenced table's singular form: `organization_id`, `parent_node_id`, `source_node_id`, `target_node_id`.
- Raw SQL (tagged templates, migrations) must use the canonical column names from `packages/db/schema.sql`.
- When writing or reviewing SQL, verify column names against the schema file.

## Current graph-native schema quick reference

| Table | Org key | Primary key | Parent / link columns |
|-------|---------|-------------|------------------------|
| `organizations` | `id` | `id` | — |
| `nodes` | `organization_id` | `id` | `parent_node_id`, `container_id` (generated from `properties->>'container_id'`) |
| `edges` | `organization_id` | `id` | `source_node_id`, `target_node_id` |
| `node_settings` | `organization_id` | `node_id` | — (one-to-one with nodes) |

Source of truth: `packages/db/schema.sql`. Migrations live in `packages/db/migrations/`; any column rename must go through `atlas migrate diff` and update `atlas.sum`.
