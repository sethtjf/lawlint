# Unit Testing

## No `mock.module`

Never use `mock.module` from `bun:test`. It operates on a global module registry and causes cross-file test contamination — mocks from one test file leak into others depending on evaluation order.

## Handler Testability: Factory Pattern

TypeScript handler modules must export a factory function that accepts a typed deps object and returns the handler functions. This makes dependencies explicit, injectable, and testable without module mocking.

### Handler file structure

```typescript
// apps/luis/src/handler.ts

// 1. Define deps interface — only what this module actually uses
export interface EmailHandlerDeps {
	dispatchEmail: (message: ForwardableEmailMessage) => Promise<void>;
	log: { info: (msg: string) => void; warn: (msg: string) => void; error: (msg: string) => void };
}

// 2. Default deps wired to real implementations
const defaultDeps: EmailHandlerDeps = {
	dispatchEmail: (await import("./dispatch")).dispatchEmail,
	log: console,
};

// 3. Factory creates handlers closed over deps
export function createEmailHandlers(deps: EmailHandlerDeps = defaultDeps) {
	return {
		async fetch(request: Request) {
			return Response.json({ ok: true });
		},
		async email(message: ForwardableEmailMessage) {
			await deps.dispatchEmail(message);
		},
	};
}

// 4. Re-export production instances for route wiring
export const { fetch, email } = createEmailHandlers();
```

### Deps interface rules

- Name it `<Module>Deps` (e.g., `SyncDeps`, `ItemsDeps`).
- Include **only** the functions/values the module actually calls.
- Use `typeof import(...).<export>` to stay in sync with the real module types.
- For logger, use a minimal interface (`{ info, warn, error }`) rather than the full `Logger` type.

### Route wiring

Routes/workers continue to import the re-exported production handlers. No changes needed to route files:

```typescript
import { fetch, email } from "./handler";

export default {
	fetch,
	email,
};
```

## Test file structure

```typescript
// apps/luis/src/__tests__/handler.unit.test.ts
import { beforeEach, describe, expect, it, mock } from "bun:test";
import { createEmailHandlers, type EmailHandlerDeps } from "../handler";

// 1. Create mock deps — every function is a bun mock
function createMockDeps(): EmailHandlerDeps {
	return {
		dispatchEmail: mock(() => Promise.resolve()),
		log: { info: mock(), warn: mock(), error: mock() },
	};
}

describe("worker handlers", () => {
	let deps: EmailHandlerDeps;
	let handlers: ReturnType<typeof createEmailHandlers>;

	// 2. Fresh deps + handlers per test
	beforeEach(() => {
		deps = createMockDeps();
		handlers = createEmailHandlers(deps);
	});

	it("acks health checks", async () => {
		const res = await handlers.fetch(new Request("http://localhost"));

		expect(res.status).toBe(200);
		expect(deps.dispatchEmail).not.toHaveBeenCalled();
	});
});
```

### Test rules

- **Never import real implementations** in test files — only the factory and types.
- **Create fresh deps in `beforeEach`** — each test gets isolated mocks.
- **Use `createMockDeps()` helper** at the top of each test file. This is local to the file, not shared across files.
- **Assert on deps directly** — `expect(deps.itemQueries.getItem).toHaveBeenCalledWith(...)`.
- **No top-level side effects** — no `await import()` of modules under test at file scope.

## Non-handler modules

For non-handler modules (services, utilities, middleware), apply the same principle: accept dependencies as function parameters rather than importing them at the top level.

```typescript
// graph-client.ts
export interface GraphClientDeps {
	apiUrl: string;
	accessToken: string;
	fetchImpl?: typeof fetch;
}

export function createGraphClient(deps: GraphClientDeps) {
	return {
		async search(query: string) {
			const fetcher = deps.fetchImpl ?? fetch;
			// ...
		},
	};
}
```

## Migration checklist

When refactoring an existing test file away from `mock.module`:

1. Identify all `mock.module` calls — these are the dependencies.
2. Define the `<Module>Deps` interface in the handler file.
3. Wrap handler functions in a `create<Module>Handlers(deps)` factory.
4. Wire `defaultDeps` to real imports.
5. Re-export production handler functions for route compatibility.
6. Rewrite the test to use `createMockDeps()` + `beforeEach` pattern.
7. Delete all `mock.module` calls and path resolution boilerplate.
8. Run the test in isolation: `bun test <file>`.
9. Run the full suite: `./scripts/run-unit-tests.sh`.
