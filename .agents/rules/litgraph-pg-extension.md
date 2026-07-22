---
paths:
  - packages/litgraph-pg/**
  - docker/postgres-extension-upgrade/**
---

# litgraph_pg extension discipline

## What belongs in the extension vs. an Atlas migration

`litgraph_pg` is a compiled pgrx extension. Only objects that **genuinely need
the compiled `.so`** belong in it:

- the graph engine and `litgraph_query` traversal SRFs,
- projection / edge primitives (`litgraph_*` `LANGUAGE c` functions),
- the trigger functions that call those C primitives, and the small
  `LANGUAGE sql`/`plpgsql` glue they depend on (e.g.
  `litgraph_pg_triggers_skipped`, `litgraph_pg_default_max_bytes`) when it is
  inseparable from that trigger wiring.

**Pure SQL/plpgsql helpers that do not call a C primitive must NOT live in the
extension.** Put them in Atlas migrations under `packages/db/migrations/`.
Migrations are content-hashed via `atlas.sum` and deploy through the normal,
checksum-guarded path, so they cannot hit the extension version-gating trap
below. (Example: `gen_public_id()` was a C `#[pg_extern]`; it moved to a plain
plpgsql migration in 0.2.2 precisely because it never needed the `.so`.)

## Why: the version-gating trap

PostgreSQL keys extension upgrades on the version **string**, not on content.
`ALTER EXTENSION litgraph_pg UPDATE TO '<v>'` is a **no-op** when a cluster is
already marked `<v>`, and the `postgres-extensions` reconciler
(`docker/postgres-extension-upgrade/bin/reconcile-extensions.sh`) only runs the
`UPDATE` when the installed version differs from the pinned target in
`k8s/postgres-extensions/base/configmap-targets.yaml`.

So **any change to the extension's SQL surface that does not bump the version
never reaches clusters already at that version.** Fresh installs (CI, local)
pick it up via `CREATE EXTENSION` and look fine, while staging/prod silently
stay stale. That is the bug behind PR #1639 (a stale edge-insert trigger body
and a missing function on clusters that reported `0.2.1`).

## The rule: never edit a released SQL surface in place

To change the extension's SQL surface — a `#[pg_extern]` signature, an
`extension_sql!` block (trigger/plpgsql body, `CREATE TRIGGER` DDL), or a
checked-in `sql/*.sql` — you MUST ship it as a **new version**, all together:

1. `default_version` in `packages/litgraph-pg/litgraph_pg.control`
2. `version` in `packages/litgraph-pg/Cargo.toml` (identical value)
3. a new `packages/litgraph-pg/sql/litgraph_pg--<prev>--<new>.sql` upgrade
   script using `CREATE OR REPLACE` for every changed/added object (idempotent
   vs. a fresh install at `<new>`, so the parity gate stays green)
4. `litgraph_pg=<new>` in `k8s/postgres-extensions/base/configmap-targets.yaml`
5. a defensive Atlas migration under `packages/db/migrations/` that runs
   `ALTER EXTENSION litgraph_pg UPDATE TO '<new>'`
6. regenerate the surface manifest: `just litgraph-surface-update`

Keep versions human-readable and monotonic. Do **not** use a content hash as
the version: PG upgrade scripts are named `--<from>--<to>.sql` and `<from>` is
the per-cluster installed version, unknown at build time.

## Guardrails (enforced)

- **Surface guard** — `scripts/litgraph-pg-surface-guard.ts` (Bun) pins a content
  hash of the SQL surface to `default_version` in
  `packages/litgraph-pg/surface.lock.json`. CI (the `rust-postgres-pgrx` lane)
  fails when the surface changed without the full version-bump set above, and
  rejects regenerating the manifest at the same version (it diffs the pinned
  hash against the PR base). Run `just litgraph-surface-check` locally before
  pushing. This complements the `Images / Build Postgres` parity gate, which
  compares upgrade-path vs. fresh-install catalog *identities* (not bodies) and
  so cannot catch a changed plpgsql/trigger body or an in-place edit.
- **Drift audit** — `scripts/litgraph-pg-drift-audit.ts` (Bun) is a read-only check
  that compares a live cluster's deployed catalog (owned functions/triggers and
  their `pg_proc.prosrc` bodies) against the repo's expected surface for the
  installed version, to find clusters that silently missed a past upgrade.
