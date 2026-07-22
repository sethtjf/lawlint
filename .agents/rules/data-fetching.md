---
paths:
  - apps/web/src/**
---

# Data Fetching

## Generated Hooks (REQUIRED)

`@litvue/sdk-typescript` auto-generates TanStack Query hooks via `@hey-api/openapi-ts` from the unified API spec (`api.json`):

| Import Path | Contents |
|-------------|----------|
| `@litvue/sdk-typescript/api` | SDK functions + types for the graph-native API (`nodesList`, `graphQuery`, `edgesList`, ontology, indexing, LLM, etc.) |
| `@litvue/sdk-typescript/api/query` | `*Options()`, `*QueryKey()`, `*Mutation()` TanStack Query hooks for the API |
| `@litvue/sdk-typescript/client` | `configureClient()`, `apiClient` |
| `@litvue/sdk-typescript/api/zod` | Zod validation schemas |

## MANDATORY: Use Generated Hooks

**ALWAYS** use generated hooks from `@litvue/sdk-typescript/api/query`.

**NEVER:**
- Use raw `fetch()` for API calls
- Use `useEffect` + `fetch` for data loading
- Create custom API wrapper functions
- Duplicate types that exist in `@litvue/sdk-typescript/api`

### Exceptions (explicit permission required)

Only use raw fetch for:
1. **WebSocket connections** — the agentd AG-UI session stream (`apps/web/src/lib/agentd-connection.ts` → `GET /v1/sessions/{id}/stream`) is a direct browser→agentd connection and is not covered by OpenAPI.
2. **SSE streaming** — endpoints returning `text/event-stream`.
3. **Presigned upload PUT** — the PUT to Azure Blob Storage goes direct-to-bucket; only the preceding node-scoped upload initialization goes through the API client.

Inline helpers in the component file if needed. Do NOT create wrapper files.

### Route with Loader (Preferred)

```typescript
import { createFileRoute } from "@tanstack/react-router";
import { useSuspenseQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
  nodesListOptions,
  nodesListQueryKey,
  nodesCreateMutation,
  nodesSoftDeleteMutation,
} from "@litvue/sdk-typescript/api/query";

export const Route = createFileRoute("/_app/matters/")({
  loader: ({ context: { queryClient } }) =>
    queryClient.ensureQueryData(nodesListOptions({ query: { node_type: "matter" } })),
  pendingComponent: () => <LoadingSkeleton />,
  errorComponent: ({ error }) => <ErrorDisplay error={error} />,
  component: MattersPage,
});

function MattersPage() {
  const { data } = useSuspenseQuery(
    nodesListOptions({ query: { node_type: "matter" } }),
  );
  const matters = data.items;

  const queryClient = useQueryClient();

  const createMatter = useMutation({
    ...nodesCreateMutation(),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: nodesListQueryKey({ query: { node_type: "matter" } }),
      });
    },
  });

  const deleteNode = useMutation({
    ...nodesSoftDeleteMutation(),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: nodesListQueryKey({ query: { node_type: "matter" } }),
      });
    },
  });

  return (
    <div>
      <button
        onClick={() =>
          createMatter.mutate({
            body: { node_type: "matter", properties: { name: "New Matter" } },
          })
        }
      >
        New Matter
      </button>
      {matters.map((n) => (
        <MatterCard
          key={n.id}
          node={n}
          onDelete={() => deleteNode.mutate({ path: { id: n.id } })}
        />
      ))}
    </div>
  );
}
```

## Anti-Patterns (DO NOT USE)

```typescript
// BAD: Manual state + useEffect fetch
const [nodes, setNodes] = useState([]);
useEffect(() => {
  fetch("/nodes").then(r => r.json()).then(setNodes);
}, []);

// BAD: Raw fetch inside useQuery
const { data } = useQuery({
  queryKey: ["nodes"],
  queryFn: () => fetch("/nodes").then(r => r.json()),
});

// BAD: Custom API wrapper
import { api } from "@/lib/api-client";
const nodes = await api.listNodes(); // WRONG — use generated hooks
```

### Correct Pattern

```typescript
// GOOD: list children of a container by parent_node_id
import { nodesListOptions } from "@litvue/sdk-typescript/api/query";
const { data } = useQuery(
  nodesListOptions({ query: { parent_node_id: folderId } }),
);

// GOOD: fetch a single node by id
import { nodesGetOptions } from "@litvue/sdk-typescript/api/query";
const { data: node } = useSuspenseQuery(
  nodesGetOptions({ path: { id: nodeId } }),
);

// GOOD: traverse the graph from an authorized node
import { graphQueryMutation } from "@litvue/sdk-typescript/api/query";
const graphQuery = useMutation(graphQueryMutation());
graphQuery.mutate({ body: { start_node_id: nodeId, depth: 2 } });
```

## Decision Matrix

| Use Case | Approach |
|----------|----------|
| Initial page data | `ensureQueryData` in loader + `useSuspenseQuery` |
| Frequently updating data | `useSuspenseQuery` with short `staleTime` |
| User-initiated mutations | Generated `*Mutation()` + `invalidateQueries` |
| Optimistic updates | `useMutation` with `onMutate`/`onError`/`onSettled` |
| Long lists with paging | `*InfiniteOptions()` (e.g. `nodesListInfiniteOptions`) |

## Naming Conventions

Hey-api derives generated names from each handler's `operation_id`. The api-rs handlers use a dotted form (`nodes.list`, `edges.list`, `graph.query`), which hey-api collapses to camelCase: `nodesList`, `edgesList`, `graphQuery`. Each operation produces:

- `{name}Options()` — query options for GET endpoints
- `{name}QueryKey()` — cache key for invalidation
- `{name}Mutation()` — mutation options for POST/PATCH/DELETE
- `{name}InfiniteOptions()` — infinite-query variant for paginated GETs

Type names follow the same pattern: a `GET /nodes` handler with `operation_id = "nodes.list"` produces `NodesListData`, `NodesListResponse`, etc. Domain types use a `Schema` suffix to avoid collision with DOM globals — `NodeSchema` (not `Node`), `EdgeSchema` (not `Edge`).

When you add or rename a handler, the operation_id determines the client surface — pick names that read well from the consumer side, then run `just openapi-sync` to update the committed specs and regenerate the client. Use `just openapi-check` before committing to verify the specs still match source.

Target API surface for reference after OpenAPI/client regeneration:

- `nodesListOptions({ query: { node_type: "matter" } })` — list nodes, filtered
- `nodesListInfiniteOptions({ query: { parent_node_id } })` — paginated list
- `nodesGetOptions({ path: { node_id } })` — fetch single node
- `nodesCreateMutation()` — create node
- `nodesPatchMutation()` — update node
- `nodesDeleteMutation()` — delete node
- `edgesListOptions({ query: { source_node_id: node_id } })` — list edges from a node
- `edgesListInfiniteOptions({ query: { source_node_id: node_id } })` — paginated edges
- `edgesCreateMutation()`, `edgesDeleteMutation()` — edge CRUD
- `graphQueryMutation()` — neighbourhood traversal (`POST /graph`)
- Generated ontology hooks cover `/organizations/{organization_id}/ontology`, `/ontologies*`, and `/nodes/{node_id}/ontology`.
- Generated file upload/blob hooks cover `POST /files/{container_id}/uploads` and `GET /files/{file_id}/blob`.
- Generated indexing hooks cover `GET/POST /nodes/{node_id}/indexing`.
- Generated LLM hooks cover `/llm/v1/chat/completions` and `/llm/v1/models`.

Domain types: `NodeSchema`, `EdgeSchema`, `GraphNodeSummary`, `GraphEdgeSummary`, and generated ontology/indexing schemas.
