---
paths:
  - "**/*"
---

# Commit Conventions

## PR Title Format (Enforced)

PR titles **must** follow [Conventional Commits](https://conventionalcommits.org):

```
type(scope): description
```

- `type` - Required, lowercase
- `scope` - Optional, in parentheses
- `description` - Required, starts with lowercase

## Commit Types

| Type | Use For | Version Bump |
|------|---------|--------------|
| `feat` | New feature | minor |
| `fix` | Bug fix | patch |
| `perf` | Performance improvement | patch |
| `refactor` | Code restructure (no behavior change) | patch |
| `style` | Formatting, whitespace | patch |
| `test` | Adding/updating tests | patch |
| `build` | Build system, dependencies | patch |
| `chore` | Maintenance, tooling | patch |
| `revert` | Revert previous commit | patch |
| `docs` | Documentation only | **skipped** |
| `ci` | CI/CD configuration | **skipped** |

## Breaking Changes

Indicate breaking changes with `!` after the type:

```
feat!: redesign authentication API
fix(api)!: change response format
```

Or include `BREAKING CHANGE:` in the commit body.

Breaking changes trigger a **major** version bump.

## Valid Examples

```
feat: add user profile page
fix(auth): resolve token refresh loop
docs: update API documentation
refactor(api): extract validation logic
feat!: remove deprecated v1 endpoints
chore: update dependencies
ci: add Python type checking
```

## Invalid Examples

```
Add user authentication          # Missing type
Feat: add authentication         # Type not lowercase
feat: Add authentication         # Description not lowercase
feat add authentication          # Missing colon
feat: add authentication.        # Period at end
feature: add authentication      # Invalid type
```

## When Writing Commits

1. Use imperative mood: "add feature" not "added feature"
2. Keep subject line under 72 characters
3. Don't end subject with period
4. Separate subject from body with blank line
5. Use body to explain "what" and "why", not "how"
