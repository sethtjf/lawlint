---
paths:
  - apps/api-rs/src/**/*.rs
  - scripts/lib/workos-*.ts
---

# RBAC Pattern for API Routes

## System of Record

**WorkOS is the single source of truth for org-level RBAC.**

- Roles and permissions defined in `scripts/lib/workos-rbac.ts`
- Seeded to WorkOS via `bun run seed:workos` (`scripts/seed-workos-rbac.ts`)
- JWT `permissions[]` claim carries the resolved permission slugs
- App middleware reads `user.permissions` array — no local role→permission lookup

Matter-level roles remain app-internal (`src/permissions.ts`).

## Permission Slug Format

`resource:action` — e.g., `billing:update`, `matter:create`, `agent:write`.

## Roles

| Role | Scope | Description |
|------|-------|-------------|
| `owner` | Org | Full org control including delete |
| `admin` | Org | Manage members, billing. Cannot delete org |
| `member` | Org | Create matters, use agents. No billing/member mgmt |
| `viewer` | Org | Read-only |

## Auth Flow

```
┌─────────────────────────────────────────────────────────────────┐
│  1. AUTHENTICATION (Who are you?)                               │
│     workosSessionMiddleware → validates JWT, sets user context   │
│     user.permissions = claims.permissions[]                     │
├─────────────────────────────────────────────────────────────────┤
│  2. ACCESS CONTROL (What context are you in?)                   │
│     requireOrganization() → ensures active org selected         │
├─────────────────────────────────────────────────────────────────┤
│  3. AUTHORIZATION (What can you do?)                            │
│     requireOrgRole('owner', 'admin') → role-based               │
│     requirePermission({ resource: ['action'] }) → JWT perms     │
└─────────────────────────────────────────────────────────────────┘
```

## Available Middleware

| Middleware | Purpose | Response on Failure |
|------------|---------|---------------------|
| `workosSessionMiddleware` | Validates auth, sets user context | 401 Unauthorized |
| `requireAuth()` | Ensures user is authenticated | 401 AUTH_REQUIRED |
| `requireOrganization()` | Ensures active org selected | 403 ORG_REQUIRED |
| `requireOrgRole(...roles)` | Checks org-level role (string match) | 403 ROLE_REQUIRED |
| `requirePermission({...})` | Checks JWT permissions array | 403 PERMISSION_DENIED |
| `requireHumanUser()` | Blocks agent/bot users | 403 HUMAN_REQUIRED |

## Usage Examples

```typescript
// Role-based: org owner only
billing.use("*", requireOrgRole("owner"));

// Permission-based: type-safe against workos-rbac.ts slugs
myRoutes.use(
  createProjectRoute.getRoutingPath(),
  requirePermission({ matter: ['create'] }),
);

// Billing (permission + human only)
myRoutes.use(
  upgradeRoute.getRoutingPath(),
  requirePermission({ billing: ['update'] }),
  requireHumanUser(),
);

// Conditional check in handler
const canEdit = hasPermission(c, { matter: ['update'] });
```

## Adding a New Permission

1. Add to `scripts/lib/workos-rbac.ts`:
   ```typescript
   { slug: "newResource:create", name: "Create New Resource" },
   ```

2. Add to appropriate roles in `environmentRoles`

3. Seed to WorkOS: `WORKOS_API_KEY=sk_... bun run seed:workos`

4. Use in routes:
   ```typescript
   requirePermission({ newResource: ['create'] })
   ```

The `Permission` type auto-derives from the definition file — typos are compile errors.

## FGA Resource-Scoped Permissions (Container)

Postgres-backed FGA gates per-resource access to graph containers. `matter`
nodes map to the single FGA resource type — `container`. Folder/file/excerpt/
session/entity/assertion descendants inherit access from their owning matter.
Org-level JWT permissions still come from WorkOS.

- Permission slugs: `container:read|write|update|delete|manage_members`
- Role slugs: `container-owner` / `container-contributor` / `container-viewer`
- Role/permission slugs are defined in `apps/api-rs/src/fga.rs`; assignments
  are stored in Postgres `container_roles`.
- FGA checks at the route level run through
  `apps/api-rs/src/modules/nodes/access.rs`; the helper resolves a node to
  its owning container, then checks `AppState.fga`.

Handlers should call the domain API in `apps/api-rs/src/fga.rs` (`Fga`,
`ContainerPermission`, `ContainerRoleKind`) rather than constructing slugs
directly.
