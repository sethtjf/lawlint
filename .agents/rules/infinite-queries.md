# Infinite Queries

## Generated Options Have a Null Bug

The `hey-api` generated `*InfiniteOptions()` queryFn uses `typeof pageParam === 'object'` to distinguish cursors from structured page params. Since `typeof null === 'object'` in JS, passing `initialPageParam: null` crashes with `Cannot read properties of null (reading 'body')`.

**Always wrap the generated queryFn to normalize null:**

```typescript
function useItemsInfinite(collectionId: string, labelId: string | null) {
	const { queryKey, queryFn } = itemsCollectionItemsListInfiniteOptions({
		path: { collection_id: collectionId },
		query: { label_id: labelId, limit: 50 },
	});
	return useSuspenseInfiniteQuery({
		queryKey,
		// Normalize null → { query: {} } for initial page (typeof null === 'object' bug)
		// @ts-expect-error Generated queryFn union includes skipToken, safe for suspense
		queryFn: (ctx) => queryFn({ ...ctx, pageParam: ctx.pageParam ?? { query: {} } }),
		getNextPageParam: (lastPage) => lastPage.next_cursor ?? undefined,
		initialPageParam: null as string | null,
	});
}
```

## Key Patterns

| Pattern | Value | Why |
|---------|-------|-----|
| `initialPageParam` | `null as string \| null` | First fetch has no cursor |
| `getNextPageParam` | `lastPage.next_cursor ?? undefined` | Return `undefined` (not `null`) to signal no more pages |
| Flatten pages | `data.pages.flatMap(p => p.items)` | Accumulate all loaded pages |
| Load more | `hasNextPage` + `fetchNextPage()` | TanStack Query manages state |
| Loading state | `isFetchingNextPage` | For load-more button spinner |

## useSuspenseInfiniteQuery vs useInfiniteQuery

Prefer `useSuspenseInfiniteQuery` when each data-fetching component is wrapped in a `<Suspense>` boundary. This gives per-component loading states without manual `isLoading` checks.

Since TanStack Query has no `useInfiniteQueries` (plural), use **component-level splitting**: each component that needs its own paginated list owns its own `useSuspenseInfiniteQuery` call.

```tsx
// Each expanded folder owns its own infinite query
function FolderContents({ collectionId, labelId }) {
	const { data, hasNextPage, fetchNextPage, isFetchingNextPage } =
		useItemsInfinite(collectionId, labelId);
	// ...
}

// Parent wraps in Suspense for per-folder loading
{isExpanded && (
	<Suspense fallback={<LoadingRow />}>
		<FolderContents labelId={label.id} />
	</Suspense>
)}
```

## Collapse/Expand Caching

Unmounting a component with `useSuspenseInfiniteQuery` doesn't lose data — React Query cache retains pages until `staleTime` expires. Re-mounting is instant from cache.
