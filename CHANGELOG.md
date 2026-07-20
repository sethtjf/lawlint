# Changelog

## [0.6.0](https://github.com/sethtjf/lawlint/compare/v0.5.0...v0.6.0) (2026-07-20)


### Features

* **ai:** cloud-first defaults — hosted providers lead, local is acknowledged opt-in ([#50](https://github.com/sethtjf/lawlint/issues/50)) ([#51](https://github.com/sethtjf/lawlint/issues/51)) ([6009a7c](https://github.com/sethtjf/lawlint/commit/6009a7c59b2c5bd3a5f3e5095558a63c64ffb062))
* **cli:** lawlint learn — mine a personal rule package from the user's corpus ([#40](https://github.com/sethtjf/lawlint/issues/40)) ([#46](https://github.com/sethtjf/lawlint/issues/46)) ([34c844e](https://github.com/sethtjf/lawlint/commit/34c844e6554a3d95f43339a4cf0809bf06dec28e))
* **core:** layer-2 statistical rules — burstiness and triad density ([#37](https://github.com/sethtjf/lawlint/issues/37)) ([#48](https://github.com/sethtjf/lawlint/issues/48)) ([db3e0ee](https://github.com/sethtjf/lawlint/commit/db3e0ee26335710727fd402fea5a0bb2ae850ba9))
* **core:** split rule intent (style|detection); score aggregates detection rules only ([#38](https://github.com/sethtjf/lawlint/issues/38)) ([#43](https://github.com/sethtjf/lawlint/issues/43)) ([9831e26](https://github.com/sethtjf/lawlint/commit/9831e261a2684f7d6f479afe16752c6d4ed0e873))
* **docx:** lint and fix .docx via tracked changes + comments ([#34](https://github.com/sethtjf/lawlint/issues/34)) ([f3c2255](https://github.com/sethtjf/lawlint/commit/f3c2255e863b31a5d0c2d34cf1e88bbe890377d2))
* **eval:** judged evaluation — tier-3 metrics and verdict-discipline rate ([#39](https://github.com/sethtjf/lawlint/issues/39) Part 2) ([#49](https://github.com/sethtjf/lawlint/issues/49)) ([d91573b](https://github.com/sethtjf/lawlint/commit/d91573b20b1f28c3764ad9faffab7f9a0138d868))
* **init:** AI model preferences — local/hosted catalog, keys outside the project ([#41](https://github.com/sethtjf/lawlint/issues/41)) ([#44](https://github.com/sethtjf/lawlint/issues/44)) ([3e7c3d3](https://github.com/sethtjf/lawlint/commit/3e7c3d356ab9e77247c608e79aaf91f52b95ed83))
* **rules:** add Orwell writing rules and AI-voice checks ([#47](https://github.com/sethtjf/lawlint/issues/47)) ([29577be](https://github.com/sethtjf/lawlint/commit/29577bec2f988705754af5ce37f646d5cded9abb))


### Bug Fixes

* **judge:** drop negative-verdict findings and demand [] for clean chunks ([#42](https://github.com/sethtjf/lawlint/issues/42)) ([0a2994f](https://github.com/sethtjf/lawlint/commit/0a2994ffaa92ee8e14b7da3e5ca7a93078f730c4))

## [0.5.0](https://github.com/sethtjf/lawlint/compare/v0.4.0...v0.5.0) (2026-07-18)


### Features

* mechanical fixes, diff view, and AI remediation prompts ([#32](https://github.com/sethtjf/lawlint/issues/32)) ([75ef160](https://github.com/sethtjf/lawlint/commit/75ef160299cee27fd08afb7cdfcd4295e3b20454))
* **website:** simplify navigation, hero, and docs structure ([#28](https://github.com/sethtjf/lawlint/issues/28)) ([d8ae603](https://github.com/sethtjf/lawlint/commit/d8ae6030d05972e614b256d734faedc22a26e1a5))

## [0.4.0](https://github.com/sethtjf/lawlint/compare/v0.3.0...v0.4.0) (2026-07-18)


### Features

* **cli:** add lawlint init and .lawlint/config.json discovery ([#25](https://github.com/sethtjf/lawlint/issues/25)) ([528be79](https://github.com/sethtjf/lawlint/commit/528be790b9e1c5441087e0fd9d28e18b8f9a16b3))
* **cli:** launch ratatui TUI on bare lawlint invocation ([#24](https://github.com/sethtjf/lawlint/issues/24)) ([311219e](https://github.com/sethtjf/lawlint/commit/311219e42d329ff532a917d94ea99adf0dbb03a5))
* **website:** add changelog page rendering release notes ([#27](https://github.com/sethtjf/lawlint/issues/27)) ([350326e](https://github.com/sethtjf/lawlint/commit/350326e79f5d956b671247d3f296fede199f2404))

## [0.3.0](https://github.com/sethtjf/lawlint/compare/v0.2.0...v0.3.0) (2026-07-17)


### Features

* **cli:** add --version, auto update-check, and self-update ([#21](https://github.com/sethtjf/lawlint/issues/21)) ([cd61088](https://github.com/sethtjf/lawlint/commit/cd6108860f2eab31e930af906538653890f8bfc2))

## [0.2.0](https://github.com/sethtjf/lawlint/compare/v0.1.0...v0.2.0) (2026-07-17)


### Features

* rebuild playground as a React island with shadcn/ui + Tailwind ([#15](https://github.com/sethtjf/lawlint/issues/15)) ([f9d9965](https://github.com/sethtjf/lawlint/commit/f9d9965608b62fe0cec132ef616db20b9c08c377))
* rewrite rules engine as 3-tier declarative system with local AI judge ([#17](https://github.com/sethtjf/lawlint/issues/17)) ([f0eed56](https://github.com/sethtjf/lawlint/commit/f0eed5692ace4c480b8f4a6c7d1002be9dd8b6d2))
* **website:** add favicon from desktop app icon ([#16](https://github.com/sethtjf/lawlint/issues/16)) ([74fd62f](https://github.com/sethtjf/lawlint/commit/74fd62f6fcfe50b20e958944c9cf83b5327bc753))
