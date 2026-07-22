# Migration Safety

## Migrations and code must deploy together safely

Atlas migrations run automatically via Flux on push to main. They typically complete before API pods finish rolling out. This means **a migration will hit the production database while the old API code is still serving traffic**.

## The rule

A migration must be safe to run against the **currently deployed** API code, not just the code in the same commit.

## Two-phase pattern

If a migration changes data that the API queries depend on (e.g., renaming a value, adding a new enum, restructuring rows), split it into two deploys:

### Phase 1: Make the code handle both old and new states
- Deploy API code that works with **both** the old and new data
- Example: if adding `item_type = 'container'`, the API should treat unknown item types gracefully, or explicitly handle both `'upload'` and `'container'`

### Phase 2: Run the migration
- In the next push, add the migration that changes the data
- The already-deployed code handles both states, so no outage

## Quick check before adding a migration

Ask: "If this migration runs but the old API code is still serving requests, will anything break?"

- **Safe:** Adding a new column with a default, adding an index, adding a new table
- **Unsafe:** Changing values the API filters on, renaming columns, dropping columns the API reads, restructuring data the API queries assume

If unsafe, use the two-phase pattern.

## Incident reference

On 2026-03-20, a migration set `item_type = 'container'` on archive items. The API code that handles this type hadn't deployed yet, so those items became invisible in production. Required two follow-up migrations (revert + re-apply) to fix.
