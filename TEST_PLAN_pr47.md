# Test Plan — PR #47 Orwell writing rules (lawlint website)

Surface: website (astro dev on http://localhost:4321), built from branch
`devin/1784519965-orwell-writing-rules`. `bun run dev` runs wasm-pack build + generates
`rules.json`, so playground and /rules reflect the 5 new built-in rules.

Ground truth (from native CLI on the same core):
- Dirty text → 9 diagnostics, score 1: no-achievement-language×2, prefer-concise-phrases×1,
  prefer-short-words×3, no-dead-metaphors×1, no-foreign-phrases×2.
- Clean text → 0 diagnostics, score 100.

## Test 1 — /rules lists 27 rules including the 5 new ones
Steps: Navigate to http://localhost:4321/rules.
Pass/Fail:
- The last numbered row shows "27" (index padStart), i.e. 27 total rules. FAIL if 22 or any other count.
- Rows exist for: no-dead-metaphors, prefer-short-words, prefer-concise-phrases,
  no-foreign-phrases, no-achievement-language, each with its description text. FAIL if any missing.
- Click into `no-dead-metaphors` (/rules/no-dead-metaphors): detail page renders with description
  and the bad/good example ("low-hanging fruit" → plain statement). FAIL if 404 or empty.

## Test 2 — Playground flags all 5 new rules on dirty text
Steps: Navigate to /playground. Clear editor. Paste:
"We successfully implemented comprehensive handling. In order to utilize the platform, we endeavor to facilitate discovery. The settlement was low-hanging fruit. The clause covers, inter alia, notice vis-à-vis the guarantor."
Pass/Fail (must match CLI ground truth):
- Total findings = 9. FAIL otherwise.
- no-achievement-language fires on "successfully" and "comprehensive" (×2).
- prefer-concise-phrases fires on "in order to" (×1).
- prefer-short-words fires on "utilize", "endeavor", "facilitate" (×3).
- no-dead-metaphors fires on "low-hanging fruit" (×1).
- no-foreign-phrases fires on "inter alia" and "vis-à-vis" (×2).
- Score displayed = 1. FAIL if not matching CLI.
- A broken impl (rules not built into WASM) would show these as 0 findings → clearly distinguishable.

## Test 3 — Playground clean prose: no new false positives
Steps: Clear editor. Paste:
"The parties agree to share the records within ten business days."
Pass/Fail:
- 0 findings, score 100. FAIL if any of the 5 new rules (or others) fire.
