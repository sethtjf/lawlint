# UI Style Guide Evolution

When making frontend changes that introduce **new patterns** or **deviate from the style guide**, follow this process:

## When This Applies

- New component patterns not covered by the style guide
- Different approach to an existing pattern (e.g., new empty state style)
- New animation/interaction patterns
- Color usage outside established tokens
- Layout structures that differ from documented patterns
- Typography choices not in the guide

## Process

### 1. Detect the deviation

Before implementing, recognize when you're about to:
- Create something the style guide doesn't cover
- Do something differently than the style guide specifies

### 2. Interview the user

Use the `grill` skill to discuss the design decision, covering:
- **Intent**: What problem does this new pattern solve?
- **Scope**: Should this be a one-off or a new standard pattern?
- **Consistency**: How does this relate to existing patterns?
- **Evolution**: Should this replace an existing pattern or coexist?

### 3. Update documentation

After the interview, if the pattern should become standard:

1. **Update `apps/web/STYLE_GUIDE.md`** - Add the new pattern with:
   - Code examples
   - When to use it
   - How it relates to existing patterns

2. **Update `.agents/skills/frontend-design/SKILL.md`** - Add quick reference constraint to the Implementation Constraints section if it's commonly used

3. **Update changelog** - Add entry to the changelog table at the bottom of the style guide

## Skip interview when

- Following the style guide exactly
- Making minor adjustments within established patterns (e.g., different icon)
- Bug fixes that don't change visual patterns
