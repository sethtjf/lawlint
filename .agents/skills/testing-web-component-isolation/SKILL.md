---
name: testing-web-component-isolation
description: Test a self-contained apps/web React component (e.g. the d3-force MatterConstellation graph) end-to-end in the running app via a temporary dev-only public route seeded with fixtures, without standing up the full backend/auth/seed stack. Use when a PR changes only rendering/interaction of a component whose data-fetching is unchanged.
---

# Testing web components in isolation (apps/web)

When a PR changes only the **visualization/interaction** of a component (data fetching
unchanged), you do NOT need the full local stack (Docker + Postgres + Rust api-rs + Azure
seed + WorkOS login). Render the real component in the running vite app via a temporary
public route seeded with representative fixtures. This exercises the exact changed code.

## Why not the full stack / staging
- Full stack (`just dev web api` + `just seed`) needs Docker, a Rust build, an Azure blob
  seed download, and WorkOS localhost login — heavy and fragile for a pure frontend change.
- Pointing a local frontend at the staging API fails: api-rs CORS (`cors_allowed_origins`)
  does not allow the localhost origin.

## Procedure
1. Start vite only: `cd apps/web && bun run dev` → serves `http://localhost:3000`.
2. Add a TEMPORARY top-level route `apps/web/src/routes/<name>.tsx` using
   `createFileRoute("/<name>")`. Top-level routes (siblings of `login.tsx`, NOT under
   `_app`) are **public — no auth gate**, so no WorkOS login is needed. The TanStack Router
   plugin auto-regenerates `routeTree.gen.ts` on file add (do not hand-edit it).
3. Import the real component and render it with fixtures matching the generated API types
   (e.g. `GraphNodeSummary`/`GraphEdgeSummary` from `@litvue/sdk-typescript/api`). Render
   multiple instances to cover different prop configs (interactive vs not, different `scale`).
4. Open `localhost:3000/<name>` in the browser and test.
5. **Cleanup:** delete the temp route file; vite regenerates `routeTree.gen.ts` back to the
   committed state. Verify `git status --porcelain` is clean before finishing. Kill vite
   (`pkill -f "vite dev"`).

## Interaction-testing tips (computer tool)
- Nodes in the d3-force graph are tiny (~4–5px) and the sim keeps micro-drifting, so precise
  clicks miss and land on background. **Zoom the graph in first** (wheel-up over the panel)
  to enlarge nodes before trying to grab one.
- `left_mouse_down` takes **no coordinate** — `mouse_move` to the target first, then
  `left_mouse_down`, then stepped `mouse_move`s, then `left_mouse_up`.
- To prove a **node drag** (vs a background pan), capture a screenshot **mid-drag while the
  button is held**: the grabbed node sits under the cursor with edges stretched while the
  rest stays put. After release the node eases back into the sim, so a post-release shot is
  ambiguous. Tell-tale of an accidental pan: the whole scene (incl. the pinned center node)
  translates by exactly the drag delta.
- Use `hover` to both confirm you're on a node (it highlights + shows its label) and to test
  hover-highlight. The center/focus node is pinned (`fx/fy`), so it won't drag freely — grab
  a leaf node instead.
- `zoom` (inspect region) does not move the mouse, so you can zoom-inspect while holding a drag.
- react-scan's dev FPS/toolbar overlay (bottom-right) is harmless but visible in recordings.

## Devin Secrets Needed
None — this isolation approach needs no secrets (no backend/auth/seed).
