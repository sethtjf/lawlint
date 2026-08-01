# Simple English rules

This is an opt-in lawlint rule package inspired by the
[SimpleEnglish skill](https://github.com/AminBlg/SimpleEnglish/tree/main/skills/simple-english),
an unofficial aid for ASD-STE100 Simplified Technical English. It focuses on
short, direct technical prose, clear instructions, simple verb forms, plain
word choices, and consistent terminology.

## Enable the package

```sh
lawlint --rule-dir packages/simple-english draft.md
```

Or add it to `.lawlint/config.json`:

```json
{
  "ruleDirs": ["packages/simple-english"]
}
```

The package is deliberately external and opt-in. It has two practical modes:

- **Pragmatic (default):** enable the package for technical documentation while
  keeping domain terms such as `webhook`, `commit`, and `endpoint`.
- **Strict:** use it for text that names STE or ASD-STE100 compliance. Full
  compliance needs the official dictionary and a human review.

## Overlap with core rules

The package does not duplicate `core/no-semicolons`; that rule already covers
STE Rule 8.1. When using this package, consider disabling
`core/sentence-length`, because its 45-word limit is less strict than this
package's 25-word descriptive limit (and STE uses 20 words for procedural
text). `core/prefer-short-words` and `core/prefer-concise-phrases` also overlap
with some plain-word substitutions, so review duplicate findings if you enable
both.

The package skips the substitutions already covered by those core rules,
including `utilize`, `facilitate`, `in order to`, `due to the fact that`, `in
the event that`, and `when it comes to`.

## Attribution and disclaimer

The rule examples and slop-to-simple table are adapted from
[AminBlg/SimpleEnglish](https://github.com/AminBlg/SimpleEnglish), released
under the MIT license. This package is an unofficial aid. It is not affiliated
with or endorsed by ASD or STEMG. ASD-STE100 is an ASD trademark. The official
dictionary is copyrighted; full compliance requires the official dictionary
available from [asd-ste100.org](https://asd-ste100.org/).
