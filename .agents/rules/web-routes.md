---
paths:
  - apps/web/src/routes/**/*.tsx
---

# Dashboard Route Patterns

## TanStack Router File Convention

Add files to `apps/web/src/routes/`:

```typescript
// apps/web/src/routes/_app/sessions/new.tsx
import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/_app/sessions/new")({
  component: NewSessionPage,
});

function NewSessionPage() {
  return <div>...</div>;
}
```

## Route Structure

The app is **graph-native and org-scoped via WorkOS claims** — there are no
`$orgSlug`/`$matterSlug` URL params. The active org comes from the auth session;
matters/folders/sessions are graph nodes addressed by id under `/items/$nodeId`. The
home `/` is the primary status surface; the constellation lives at `/graph`.
Matters are surfaced on the home and also have a dedicated list at `/matters`.

```
routes/
├── __root.tsx                  # Root layout
├── index.tsx                   # "/" graph landing (LensCanvas); → /organizations if no org
├── api-docs.tsx                # Scalar API reference
└── _app.tsx                    # Authenticated shell (AppShell); → "/" if unauthenticated
    └── _app/
        ├── matters.tsx         # Layout (<Outlet/>)
        ├── matters/
        │   ├── index.tsx       # "/matters" matters list (Atlas surface)
        │   └── new.tsx         # Create matter
        ├── items/$nodeId.tsx    # Universal node viewer (matter/folder/file/session/report)
        ├── organizations/
        │   ├── index.tsx       # Org selection
        │   └── new.tsx         # Create org
        ├── sessions/
        │   ├── index.tsx       # Sessions list
        │   └── new.tsx         # Create session
        ├── settings.tsx        # Settings layout; "/settings" → /settings/general
        └── settings/
            ├── general.tsx
            ├── account.tsx
            ├── billing.tsx
            ├── connections.tsx
            ├── members.tsx
            ├── sync.tsx
            └── usage.tsx
```

There is no `/org/$orgSlug/*` tree — org-scoped settings live under `/settings/*`.

## Requirements

- MUST NOT edit `routeTree.gen.ts` - it's auto-generated
- MUST use `createFileRoute()` with path matching file location
- SHOULD co-locate route component in same file
- Route params available via `Route.useParams()`
