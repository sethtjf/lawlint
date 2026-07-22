---
paths:
  - apps/web/src/**/*.tsx
  - apps/admin-panel/src/**/*.tsx
  - packages/ui/**/*.tsx
---

# UI Styling: Tailwind and Components First

Build UI with Tailwind utility classes and the shared `@litvue/ui` components (the shadcn
copy-and-own library). Custom components and inline `style={}` props are rare exceptions.

## Rules

- Reach for an existing `@litvue/ui` primitive (Button, Dialog, Card, Input, Sidebar) before
  writing markup. Do not hand-roll what the library already provides.
- Use Tailwind utilities, never inline `style`. Consume design tokens through arbitrary
  values (`z-[var(--z-surface)]`, `bg-[var(--accent-gold)]`) and shared CSS recipes
  (`.glass-panel`). Custom CSS belongs in tokens and recipes (`app.css`, `globals.css`).

## When inline `style` is acceptable

Only for genuinely dynamic runtime values (virtualizer offsets, computed widths or
positions) or props passed to third-party components. Mark each one:

```tsx
// biome-ignore lint/plugin: dynamic virtualizer row offset
style={{ transform: `translateY(${row.start}px)` }}
```

## Enforcement

A Biome GritQL plugin (`./biome/no-inline-style.grit`, wired in the root `biome.json`
`overrides`) flags every inline `style={}` repo-wide across all TS/TSX, except
`packages/email` (email clients require inline CSS). Suppress legitimate dynamic cases with
`// biome-ignore lint/plugin: <reason>`; convert everything else to Tailwind.
