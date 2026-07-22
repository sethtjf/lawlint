---
paths:
  - ".github/workflows/**"
  - "e2e/**"
  - "playwright.config.ts"
---

# CI/CD Workflows

## Build Once, Deploy Anywhere (non-negotiable)

ADR 0075 governs every deploy-shaped workflow:

- `main` is the only long-lived source branch. Never create or depend on an
  environment branch or deployment-automation Git write-back.
- A deployable `main` SHA builds every Rust binary/extension, runtime container,
  Cloudflare code/assets package, migration image, and Kubernetes OCI release
  bundle exactly once per supported target platform.
- Staging validates those exact artifacts. Production and rollback may only
  promote/redeploy their recorded digests or checksums; they never compile,
  bundle, generate, or fall back to a fresh build.
- Runtime Dockerfile stages consume cataloged binaries/extensions and pinned
  native dependencies. Cargo, rustc, cargo-pgrx, and source generation are
  forbidden during runtime image assembly.
- Environment routes, bindings, secrets, URLs, replicas, resources, models, and
  Terraform inputs are deploy/runtime configuration. Keep them out of compiled
  bytes.
- Cloudflare per-environment builds are forbidden unless an ADR amendment records
  concrete framework evidence and explicit approval. No exception exists today.
- The signed release catalog must prove staging/production image, OCI bundle,
  and Cloudflare artifact identity before production can proceed.

The signed OCI release catalog and digest-promotion path described below are
the steady-state deployment architecture. Do not reintroduce branch-based Flux
write-back, mutable deployment commits, or deploy-time source builds.

## Architecture

Manifest-based promotion pipeline — validate on PR (and `merge_group`, when enabled), prebuild deploy images on PR for cache reuse, land on main, then a single `deploy.yml` deploys to staging by reusing prebuilt images by digest, validates, generates a deployment manifest, and — after a required-reviewer approval gate — promotes the exact validated artifacts to production. Staging and production are two stages of **one** workflow run (ADR 0071 D4): the former `Deploy / Staging` → (`workflow_run`) → `Deploy / Production` hop is gone, and the staging manifest is handed off in-run.

```
PR / merge_group ── ci.yml ("CI / Validate", full)         ── branch protection: "CI Success"
                 └─ prebuild-images.yml ("Images / Prebuild Deploy")
                                                          └── push + verify <ACR>/<image>:sha-<full_sha> candidate tags

docker/base change / weekly ──────── build-base-images.yml ("Images / Build Base")
                                       └── push shared Rust/ORT base images

Merge to main ───── ci.yml ("CI / Validate", cheap gates only — heavy TS/Rust jobs skipped, already validated)
         │           ├── detect-changes       (always runs)
         │           ├── telemetry-contract   (always runs)
         │           ├── k8s-config           (always runs)
         │           ├── k8s-manifests        (always runs)
         │           ├── workspace-invariants (always runs)
         │           └── ci-success           (succeeds → workflow_run fires)
         │                                 │
         │                                 ▼
         └────────────────── deploy.yml ("Deploy / Pipeline") — one run
                              │  ── Staging stage ──
                              ├── coalesce guard (skip if main advanced)
                              ├── terraform (if infra/ changed)
                              ├── CF deploys (web, admin-panel, marketing)
                              ├── K8s image promotion:
                              │     └── prebuilt sha-<full_sha> → crane copy by digest (fail closed if absent)
                              ├── cluster readiness + E2E tests
                              ├── generate staging deployment manifest (artifact)
                              │
                              │  ⛔ environment: production approval gate
                              │  ── Production stage (needs: staging) ──
                              ├── hold-gate (PRODUCTION_DEPLOY_HOLD check)
                              ├── verify staging manifest (in-run artifact: SHA + validation)
                              ├── terraform (prod, if infra changed per manifest)
                              ├── promote images by digest (staging ACR → prod ACR)
                              ├── CF deploys (web, admin-panel, marketing)
                              ├── production validation (cluster readiness + CF health)
                              └── GitHub Release (deploy-{short_sha})

Direct push to main (break-glass) ── ci.yml (cheap gates) ── deploy.yml (same as above)
```

**CI never touches the cluster.** Each cluster reconciles the signed OCI
release bundle through its `*-oci-active` flux-system overlay. Flux reads the
bundle and never writes image setters or deployment state back to Git.

All production-stage jobs use `environment: production` with required reviewers — the run **pauses at a job boundary** until approved in the GitHub Actions UI, before any production work runs. GitHub environments gate jobs (not workflows), so merging the two workflows into one leaves the approval gate intact. Re-running the production stage for a given staging deploy is `re-run failed jobs` on the same run rather than dispatching a second workflow.

## Workflow Overview

Workflow display names use `<Domain> / <Action>` so related Actions group together in GitHub's UI. Keep file names stable and descriptive; update `workflow_run.workflows` references whenever a display name changes.

| Display name | File | Trigger | Purpose |
|--------------|------|---------|---------|
| `CI / Validate` | `ci.yml` | PR, `merge_group`, push to main | Lint, typecheck, unit tests. The cheap gate jobs (`detect-changes`, `telemetry-contract`, `k8s-config`, `k8s-manifests`, `workspace-invariants`) run on every event. Merge queue is the required heavy-validation path before main; heavy TypeScript and Rust jobs run on non-draft PRs and `merge_group`, but not on push-to-main. Direct pushes are break-glass and receive only the cheap gates. |
| `CI / OpenAPI Sync` | `openapi-sync.yml` | PR (non-draft) touching API/OpenAPI sources | Regenerates the OpenAPI specs, tool-surface catalog, and TypeScript client, then fails on drift with a diff and local remediation commands for both same-repo and fork PRs. It never commits or pushes to the PR branch; contributors must regenerate and commit artifacts before opening or updating the PR. Skips draft PRs (the job's `if:` gates on `draft == false`; re-fires via the `ready_for_review` trigger). |
| `CI / DB Migrations` | `db-migrations.yml` | PR touching `packages/db/**` / `docker/postgres/**` / `packages/litgraph-{engine,pg}/**` / push to main / manual | Boots an EMPTY Postgres from the custom image and replays the full Atlas migration chain (`atlas migrate apply`, the equivalent of `just db-migrate` against a blank DB), failing if any migration errors. Also enforces `atlas.sum` integrity (`migrate hash`) and replays the chain onto a clean dev DB (`migrate validate`). Durable guardrail for the migration-from-empty incident. |
| `Images / Prebuild Deploy` | `prebuild-images.yml` | PR (same-repo, non-draft), `merge_group`, push to main | Build K8s deploy images, push, and verify candidate `sha-<full_sha>` tags in staging ACR so `Deploy / Pipeline`'s staging stage can reuse them by digest. Failures surface as a red prebuild job but do not block merges because prebuild is not a branch-protection required check. |
| `Images / Build Base` | `build-base-images.yml` | `docker/base/**` change / manual / weekly schedule | Build + push shared Rust base images (`litvue-rust-builder`, `litvue-rust-onnx-runtime`) to ACR |
| `Images / Build Postgres` | `build-postgres-image.yml` | `docker/postgres/**`, `packages/litgraph-{engine,pg}/**`, `packages/db/migrations/**`, the workflow file — PR (non-draft) / manual | Build + verify the Postgres image on PRs (no push). Skips draft PRs (the job's `if:` gates on `draft == false`; re-fires via the `ready_for_review` trigger) so WIP pushes don't pay the ~20-min build. Intentionally does NOT trigger on the workspace-root `Cargo.toml`/`Cargo.lock`: a lockfile bump anywhere in the monorepo fired this ~20-min build needlessly, and extension-compilation regressions from a dep change are already caught by `ci.yml`'s `rust-postgres-pgrx` lane (a lockfile change escalates `rust-affected` to a full `--workspace` run). |
| `Deploy / Pipeline` | `deploy.yml` | `CI / Validate` success on main / manual | Staging + production in one run. A workflow-run deploy first skips gracefully when its triggering SHA is no longer the current `main` HEAD; manual dispatches bypass only this coalescing guard. Staging deploys all apps to staging + E2E using prebuilt images promoted by digest and fails closed if an image is absent; the production stage `needs:` staging, pauses at the `environment: production` approval gate, then promotes the exact catalog-bound digests, deploys prebuilt CF artifacts, validates, and cuts the GitHub Release. Fires once per main commit (via `workflow_run` for `CI / Validate` on `head_branch == main`). |
| `Deploy / Health Observer` | `deploy-health.yml` | Completed `Deploy / Pipeline` run on main | Maintains the canonical `deploy-blocker` issue from the completed deploy's conclusion and jobs. Only a full deploy with a successful `Generate Staging Manifest` job records recovery; PR, prebuild, merge-queue, no-change, and inconclusive runs cannot mark deployment healthy. |
| `Infra / Apply` | `terraform.yml` | Called by deploy/promote / manual | Terraform apply if infra changed |
| `Infra / PR Plan` | `terraform-pr-plan.yml` | PR touching infra/workflows | Terraform plan for staging/production |
| `Infra / Drift Detection` | `terraform-drift.yml` | Daily / manual | Scheduled Terraform drift detection |

## Workflow Naming Convention

- Use `Domain / Action` for every workflow `name:`.
- Current domains are `CI`, `Images`, `Deploy`, and `Infra`.
- Use title case after the slash and keep names short enough to scan in GitHub Actions.
- Job names can remain user-facing descriptions such as `Rust`, `Build api-rs`, or `Web → Staging`.
- If a workflow is referenced by `workflow_run.workflows`, update that reference in the dependent workflow in the same change.
- Branch protection should key on stable aggregate job names (currently `CI Success`) rather than workflow display names where possible.

## Validation Model

Merge queue is required for landing changes on `main`; direct pushes are
restricted to break-glass administration. `CI / Validate` runs the cheap gates
(`detect-changes`, `telemetry-contract`, `k8s-config`, `k8s-manifests`, and
`workspace-invariants`) on every pull request, merge-group event, and main
push. The heavy TypeScript and Rust lanes run only for non-draft pull requests
and `merge_group`, validating the exact merge commit before it lands. Draft
pull requests run the cheap gates until `ready_for_review` re-triggers the
workflow. Main pushes, including break-glass pushes, intentionally run only
the cheap gates.

The heavy Rust work is split into parallel lanes so wall-clock ≈ max(lane), not
sum(lane): `rust-affected` computes the cargo target scope once, then
`rust-clippy` (clippy + fmt), the three `rust-unit` shards (unit + integration
tests partitioned with nextest's `hash:1/3`, `hash:2/3`, and `hash:3/3`),
`rust-doctests`, and the api-rs integration lanes run concurrently. The api-rs
test binaries are compiled exactly once by `rust-api-archive`
(`cargo nextest archive`) and replayed by the `rust-api` matrix (one leg per
required docker-backed lane) without recompiling. The `agents-postgres:local`
image the docker-backed tests need is likewise built exactly once by
`rust-postgres-image` and shared as a `docker save` artifact that `rust-unit`
and the `rust-api` legs `docker load` — rebuilding it per-lane would multiply
wall-clock and the chance of a transient cargo-pgrx build flake. `rust-agentd-objstore`
is a standalone docker-backed lane that drives the live session-log blob-offload
round-trip against MinIO end-to-end (`REQUIRE_DOCKER_TESTS=1`, so a missing emulator
fails rather than skips). `rust-agentd-gate-v1-1` is the CI-runnable slice
of the ADR 0022 v1.1 cluster gate (#1367): it stands up an in-process gateway+worker
over real TCP/WebSocket sockets and drives the gate's G02 (real WS load + dropped-
connection accounting), G01/G03 (scale targets), G11 (global admission-cap rejection
with typed 503 + Retry-After through the gateway), and the G06 object-store offload round-trip
(OffloadingSessionLog over an in-memory blob store — no Docker). The full cluster
gate runs out-of-band on a real cluster via the `agentd-cluster-gate` binary +
`k8s/agentd/overlays/gate-v1.1`. `rust-agentd-fdb` is the ephemeral-FoundationDB
lane (#1533, ADR 0034 D2 / ADR 0035): it installs the official `fdbserver` 7.3.x
(pinned 7.3.77, matching the crate's `fdb-7_3` feature and `k8s/agentd/fdb-spike`),
brings up the single-process cluster the server postinst provisions, compiles
agentd with `--features fdb`, and runs the full `session_log` conformance suite —
compare-and-append, gapless replay, concurrent writers, snapshot/wake, blob
offload, and the cross-org isolation gate — against that real cluster with
`REQUIRE_FDB_TESTS=1` so it fails rather than silently skips when no cluster is
reachable. It gates the FoundationDB backend the store adopts from the start. `rust-postgres-pgrx`
is the Rust Postgres lane for `packages/litgraph-pg`: the extension's `#[pg_test]`
E2E suite can't run under a plain `cargo test`/`nextest` (the workspace test lanes
exclude `litgraph-pg` for exactly this reason), so this lane installs
`cargo-pgrx@0.18.0` + the Postgres 18 server binaries and runs
`cargo pgrx test pg18 --package litgraph-pg`, which builds the extension, installs
it into a managed Postgres, and executes each test inside a real backend. It
covers traversal correctness (walk + shortest
path), the AFTER-INSERT never-raise contract, and the shared_dsm projection path
(the extension is preloaded via `postgresql_conf_options` so the cache runs in
production's shared-memory mode). It is gated on `--workspace`/`litgraph` affected
scope. The thin `rust` job fans the
lanes back in (fails if any failed/was cancelled, tolerates legitimately skipped
lanes) so `ci-success` and branch protection keep keying on a single `Rust`
result.

`ci-success` always runs (`if: always()`), requires all cheap gates to succeed,
and on pull-request / merge-group events also requires an affected heavy lane
to succeed. It tolerates legitimately skipped heavy jobs on drafts, unaffected
paths, and main pushes. A successful main-push run is what
`Deploy / Pipeline`'s `workflow_run` trigger expects. Before staging starts,
that workflow-run path compares `github.event.workflow_run.head_sha` with the
current `main` HEAD through the GitHub API; if `main` advanced, the deploy
exits through a successful guard job and the staging and production jobs are
skipped. The lookup retries transient API failures and fails open (proceeds
with deployment) if the current HEAD still cannot be resolved. Manual
dispatches intentionally bypass the supersession guard but still use the
dispatch event's current `DEPLOY_SHA`. All artifact lookups and promotions
remain digest-bound and fail closed when the signed catalog or prebuilt
artifact is missing, unsigned, or mismatched.

`Images / Prebuild Deploy` (`prebuild-images.yml`) has its own (independent) event triggers (`pull_request`, `merge_group`, `push: [main]`); it fails closed when Azure authentication, ACR access, image publication, or post-push verification fails, but it never gates `ci-success` or `Deploy / Pipeline`. Prebuilds on push-to-main remain useful as a cache warmer for the staging deploy that follows.

## Immutable Artifact Promotion

`main` is the only long-lived source branch. The release workflow builds and
signs the immutable catalog, runtime images, Cloudflare artifacts, and OCI
bundle once. Staging validates those exact artifacts; production and rollback
only promote previously validated image, bundle, catalog, and Cloudflare
digests/checksums. Missing, mutable, unsigned, or mismatched artifacts fail
closed. Flux has no image write-back or Git deployment-state path.


## Branch Protection & Required Checks

- `CI Success` is the single required status check for `main` and its merge queue. It is a thin aggregator job inside `CI / Validate` that fans in `detect-changes`, `telemetry-contract`, `k8s-config`, `k8s-manifests`, `workspace-invariants`, `typescript`, and `rust`. The `rust` job is itself a thin fan-in over the parallel Rust lanes (`rust-affected`, `rust-clippy`, the three `rust-unit` shards (`Rust Unit & Integration Tests (shard 1/3)`, `(shard 2/3)`, and `(shard 3/3)`), `rust-doctests`, `rust-postgres-image`, `rust-postgres-pgrx`, `rust-api-archive`, `rust-api`, `rust-agentd-objstore`, `rust-agentd-gate-v1-1`, `rust-agentd-fdb`), so `ci-success` only has to key on the single `rust` result.
- On non-draft PR / `merge_group`: `CI Success` requires every affected heavy lane to succeed (or be legitimately skipped via `detect-changes`), along with all cheap gates.
- On draft PRs and `push` to `main`: `CI Success` requires only the cheap gates. Branch protection is enforced through merge queue before landing; keeping `CI Success` green on main pushes preserves the `workflow_run` chain into `Deploy / Pipeline`, including for break-glass pushes.

## Separation of Concerns

| Concern | Owner |
|---------|-------|
| Build shared base images | `Images / Build Base` |
| Build candidate deploy images | `Images / Prebuild Deploy` |
| Promote staging deploy images | `Deploy / Pipeline` (staging stage) |
| Push to ACR | GitHub Actions (`Images / ...` and `Deploy / Pipeline`) |
| Publish/promote K8s release bundle | `release-build.yml` and `Deploy / Pipeline` |
| Apply manifests to cluster | Flux reading the signed OCI bundle through `*-oci-active` |
| Secret management | Azure Key Vault synced by External Secrets Operator (ESO) — `ExternalSecret` → native k8s `Secret` |
| Migration ordering | Flux Kustomization depends-on |

## Secret Sync (External Secrets Operator)

Secrets are synced from a backend (Azure Key Vault by default) into native
Kubernetes `Secret` objects by the External Secrets Operator, replacing the
Azure CSI `SecretProviderClass` driver. Apps consume the resulting `Secret`
via `secretKeyRef`/`envFrom` — they are backend-agnostic, so switching to AWS
Secrets Manager, HashiCorp Vault, or on-prem only requires changing the
`SecretStore`/`ClusterSecretStore` backend, not app manifests.

- ESO controller install: `k8s/infrastructure/external-secrets.yaml` (Flux `HelmRelease`).
- Backend + workload identity: `k8s/external-secrets/` (`azure-keyvault` `ClusterSecretStore` + `external-secrets-azure` ServiceAccount), federated by `azurerm_federated_identity_credential.external_secrets`.
- Flux ordering: the `external-secrets` Kustomization `depends_on` `infrastructure`; secret-consuming Kustomizations (`db-migrate`, `otel-collector`, `postgres-cluster`) `depends_on` `external-secrets`.
- ESO materializes the `Secret` out-of-band, so a missing key no longer hard-stalls a pod the way an atomic CSI mount did.

## K8s Config Boundaries

Do not consume Kustomize `configMapGenerator` or `secretGenerator` outputs
across Flux Kustomization boundaries. Generated names are hash-suffixed and
only safely rewritten within the same rendered overlay. Cross-service config
must use a stable ConfigMap/Secret name or an explicit per-service overlay
value.

## K8s Images

| Image | Dockerfile | Trigger Paths |
|-------|-----------|---------------|
| `api-rs` | `apps/api-rs/Dockerfile` | `apps/api-rs/**`, `packages/api-core/**`, `packages/api-billing/**`, `packages/api-sftp/**`, `packages/api-sync/**`, `packages/api-upload/**`, `packages/api-budgets/**`, `packages/auth-rs/**`, `packages/telemetry-rs/**` |
| `api-migrate` | `packages/db/Dockerfile.migrate` | `packages/db/migrations/**` |
| `indexer` | `apps/indexer/Dockerfile` | `apps/indexer/**`, `apps/indexer-restate/**`, `packages/{pipeline-rs,indexer-core,indexer-manifest,indexer-sink-pg,file-analyzer-rs,embedder-rs,enricher-rs,embeddings-core,eval-core,telemetry-rs}/**` (unified image: `index`/`build-manifest`/`run-manifest`/`verify`/`encode` run-modes are the batch k8s Job CLI; the former NRT HTTP `server` run-mode was retired per ADR 0009, and the `indexer-restate` Restate handler that shared this image was retired per #1939/epic #1940 — its source is kept for reference until removal. NRT + batch indexing orchestration now runs as agentd actor kinds inside the `agentd` image, `--features indexing`) |
| `postgres` | `docker/postgres/Dockerfile` | `docker/postgres/**`, `packages/litgraph-engine/**`, `packages/litgraph-pg/**`, Postgres deployment wiring |

## Shared Rust Base Images (build-time only)

Not deployed to K8s — these are FROM-bases consumed by the Rust service
Dockerfiles above. Built and pushed by `build-base-images.yml` on changes
to `docker/base/**`, weekly schedule, or manual dispatch. Tagged `:latest`,
`:sha-<short>`, and a semantic tag (`:rust-<version>` / `:ort-<version>`).

| Base image | Dockerfile | Consumers |
|------------|-----------|-----------|
| `litvue-rust-builder` | `docker/base/rust-builder/Dockerfile` | All Rust service builder stages |
| `litvue-rust-onnx-runtime` | `docker/base/rust-onnx-runtime/Dockerfile` | Runtime stage of `indexer` |

Service Dockerfiles consume the bases via build-args with upstream defaults
(`rust:1.91-trixie`, `debian:trixie-slim`) so local `docker build` keeps
working; CI overrides the args to point at the ACR-hosted tags.

## Rust Affected-Crate Targeting

`ci.yml`'s `rust-affected` job computes a cargo target scope that the
`rust-clippy` / `rust-unit` / `rust-doctests` / `rust-api-archive` lanes all
consume (and that `rust-agentd-objstore` gates on, running only when the
affected set covers `agentd` or falls back to `--workspace`), so
clippy/tests/doctests run only against the workspace crates touched
by a PR (plus their transitive reverse dependents), falling back to
`--workspace` when a global file changes or when the affected set already covers
most of the workspace.

The mapping is computed by `scripts/ci-rust-affected.ts`, which consumes the
list of changed files (`git diff --name-only origin/$BASE_REF...HEAD`) and
emits one of:

| Output | Meaning |
|--------|---------|
| `none` | No Rust-relevant files changed (e.g. only TS edits). Job still runs `--workspace` as a safe default. |
| `all`  | A "global" file changed (root `Cargo.toml`, `Cargo.lock`, `rust-toolchain*`, this script, or `ci.yml`), or the affected set covers ≥70% of the workspace. |
| `-p <crate> ...` | The exact `cargo` selectors to use. |

Notes:
- Workspace membership and the dependency graph come from `cargo metadata
  --no-deps`, so adding/removing a crate in the root `Cargo.toml` is picked
  up automatically (and triggers a workspace fallback by virtue of the root
  `Cargo.toml` change itself).
- `cargo fmt --all -- --check` (in the `rust-clippy` lane) intentionally stays
  workspace-wide; it's fast and we want consistent formatting regardless of
  which crates were touched.
- Rust check, Clippy, and all three `rust-unit` shards are required.
- The required docker-backed api-rs lanes (`ready_with_postgres`,
  `fga_integration`, `auth_http`) compile once in `rust-api-archive`
  (`cargo nextest archive -p api --tests`) and run from the archive in the
  `rust-api` matrix; this also subsumes the former "API integration compile"
  step (the archive build fails if any api test target fails to compile).
- The api-rs full-schema integration tests treat `CI=true` (always set by
  GitHub Actions) as "docker required" — see `require_docker_tests()` in
  `apps/api-rs/tests/common.rs` — so they panic rather than skip when
  `agents-postgres:local` is missing. That image is built exactly once by the
  `rust-postgres-image` job and shared as a `docker save` artifact; `rust-unit`
  (which runs the workspace/affected test set, including those tests) and the
  `rust-api` legs `docker load` it. Rebuilding it per-lane would multiply
  wall-clock and the odds of a transient cargo-pgrx build flake on a cold layer
  cache.
- Run the smoke tests locally with `bash scripts/test-ci-rust-affected.sh`
  after editing the script.

## E2E Tests

**Location:** `e2e/` directory, config in `playwright.config.ts`

```bash
bun run test:e2e         # Against staging
bun run test:e2e:ui      # With Playwright UI
```

## Eval Benchmarks

There is currently no CI eval lane. The three eval harnesses
(`datasets/harnesses/{retrieval,graph,reasoning}`) and the `CI / Evals Retrieval`
workflow were removed (see `docs/decisions/0026`): they were inert spike
artifacts (synthetic-only, `continue-on-error`, unmaintained baselines), so a
deliberate eval stack is deferred to a future design. The `runs`
bookkeeping commands (`just eval runs list/show/diff/compare/upload`) remain as a
harness-agnostic view over any `run.json` artifacts a future eval stack produces.

## Concurrency Rules

**Every PR-triggered workflow must declare a `concurrency` block.** The standard
group is `${{ github.workflow }}-${{ github.ref }}` so a new push to a PR cancels
the superseded run and exactly one run reports per SHA. Validation/plan workflows
(read-only, idempotent) set `cancel-in-progress: true`; workflows that push to a
registry, deploy, or mutate Terraform state set `cancel-in-progress: false` so a
mid-flight side effect is never interrupted.

`merge_group` and `push: main` runs of `CI / Validate` are never cancelled: their
`CI Success` aggregator is the canonical required check for the landed/queued SHA,
and a concurrency-cancelled aggregator surfaces as a stuck-red required check on
the rollup. Only superseded `pull_request` runs (on an already-replaced SHA) are
cancelled. `Images / Prebuild Deploy` additionally segments its group by
`github.event_name` so a PR run and a push/`merge_group` run for the same ref stay
independent.

| Workflow | Behavior |
|----------|----------|
| `CI / Validate` | Cancel in-progress only on `pull_request` updates. `merge_group` and `push: main` runs are never cancelled. |
| `CI / OpenAPI Sync` | Cancel in-progress on PR update (overlapping validation runs would duplicate work and report stale results). |
| `Images / Prebuild Deploy` | Cancel in-progress on PR update; never cancel on push/`merge_group` |
| `Images / Build Postgres` | Cancel in-progress on PR update (build + verify, no push) |
| `CI / Evals Retrieval` | Cancel in-progress on PR update (read-only benchmark, no side effects) |
| `Images / Build Base` | Never cancel (registry pushes don't tolerate cancellation) |
| `Deploy / Pipeline` | Never cancel (staging + production in one run — a cancel could leave a partial deploy). Group `deploy-pipeline`, shared with `Rollback / Production` so a forward deploy and a rollback are mutually exclusive. Workflow-run deploys coalesce at the start of staging when their triggering SHA is no longer current `main`; manual dispatches bypass the guard. |
| `Deploy / Health Observer` | Never cancel; group `deploy-health-observer` serializes updates to the canonical blocker so every completed deploy is recorded without issue races. |
| `Infra / Apply` | Never cancel (prevent state conflicts) |
| `Infra / PR Plan` | Cancel in-progress on PR update (read-only plans with `-lock=false`) |
| `Rollback / Production` | Never cancel; shares the `deploy-pipeline` group with `Deploy / Pipeline` (forward deploy and rollback never mutate the production ACR concurrently) |
| `Infra / Drift Detection` | Never cancel (independent per-env runs) |

## Staging Deployment Manifest

The staging stage generates a `staging-deployment-manifest` artifact (schemaVersion 1) containing:
- SHA, run metadata
- Image digests for api-rs, api-migrate, indexer, postgres (from build artifact or previous successful staging manifest)
- Cloudflare deploy statuses
- Validation results (cluster readiness, E2E)

The production stage downloads and verifies this manifest **from the same run** (in-run hand-off) before promoting — there is no cross-workflow run-ID artifact lookup. No manifest = no promotion (if nothing deployed, `generate-staging-manifest` is skipped and the whole production stage skips with it). The artifact is still retained (90 days) so the separate `Rollback / Production` workflow can locate a historical manifest by SHA. GitHub tracks workflows by file path, so the manifest/last-deployed-SHA lookups (in `deploy.yml` and `rollback-production.yml`) query `deploy.yml` runs first and fall back to the pre-rename `deploy-staging.yml` runs; drop that transitional fallback once history under the old path has aged out.

## Deployment Health Signal

A **completed default-branch `Deploy / Pipeline` run is the only authority on
deployment health.** PR CI (`CI / Validate`), `Images / Prebuild Deploy`, and
merge-queue checks are *not* deployment evidence — they say nothing about whether
`main` actually reached staging/production. A successful prebuild is not a
successful deployment.

A dedicated **observer** workflow — `deploy-health.yml` (`Deploy / Health
Observer`) — reacts to a *completed* `Deploy / Pipeline` run via `workflow_run`
(`branches: [main]`) and turns it into a single durable, deduplicated
`deploy-blocker` GitHub issue via `scripts/deploy-health-signal.ts` (pure policy
in `scripts/lib/deploy-health.ts`, replayed by
`scripts/deploy-health.unit.test.ts`). The observer is deliberately **not** a
terminal job inside `deploy.yml`: an in-run job's own failure would flip the
deploy run's conclusion, corrupting the very signal it reports. Running in a
separate `workflow_run` keeps the deploy run the sole authority on its own
outcome. The signal is classified purely from the completed run's conclusion and
jobs (`classifyRun`):

- **Failure** → open the canonical blocker, or update it (deduped by the
  `deploy-blocker` label + fixed title across **open and closed** state — a
  manually-closed blocker is *reopened*, never duplicated). It records the run
  URL, head commit, failing job/step, first-failure time (the deploy run's real
  start, not the observer's wall-clock), last-successful commit,
  consecutive-failure count, and a **distinct-cause history** — so an unrelated
  later failure (e.g. an artifact download) stays visible instead of being hidden
  by the original root cause.
- **Success** → resolve the blocker and record the recovered commit/run. Only a
  *full* successful deploy that **actually deployed something** recovers (proven
  from the completed run's jobs); a no-change / inconclusive success does not.
  PR/prebuild/merge-queue success can never clear it — those workflows never
  trigger the observer.
- **Noop** → a run that deployed nothing (or was cancelled/inconclusive) never
  flips health either way.

The observer serializes its runs (`concurrency: deploy-health-observer`,
`cancel-in-progress: false`) so concurrent deploy completions can never race on
the single issue. Durable state lives in the blocker issue body (a
machine-readable `deploy-health-state` block), so no external store is needed; a
fresh streak resolves the last-successful commit from the GitHub API (the most
recent successful `deploy.yml` run on `main`, found by **paginating** — no fixed
run ceiling), never from a PR/prebuild run.

**Backlog launches pause while the blocker is open.** When the latest completed
default-branch deployment is failed, do not launch new backlog work (the roundup
**Launch** verb — dispatching cloud sessions, `deliver`, or scoping subagents):
land the deployment fix first. An open `deploy-blocker` issue is the machine- and
human-visible stop sign; landing work on top of a broken deploy only deepens the
backlog that never reached staging (the 2026-07-14 incident).

## Image Prebuild & Reuse

`prebuild-images.yml` builds K8s deploy images on PR / `merge_group` /
push to main and pushes them to the staging ACR with the
content-addressable tag `sha-<full_sha>`. `deploy.yml`'s staging-stage
`build-images` job then tries to reuse a prebuilt image by digest
and fails closed when no prebuilt image is available.

Local notes:
- The `sha-<full_sha>` tag is a candidate lookup tag. The staging
  `build-images` job promotes its resolved manifest by digest into the
  release-tag form consumed by the signed OCI bundle.
- Prebuilds are skipped for forks (no Azure access) and draft PRs by the
  upstream `detect-changes` job.
- Prebuild is fail-closed: it must push the required `sha-<full_sha>` tag and
  verify that tag exists in the staging ACR with a manifest-list-safe,
  retried existence check, or the job fails. It is not a branch-protection
  required check, so a red prebuild surfaces the problem without blocking
  merges.
- The prebuild job is intentionally NOT scoped to
  `environment: staging`, so a branch-restricted environment cannot
  block PR runs from starting. It authenticates with the repo/org-level
  `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, and
  `AZURE_SUBSCRIPTION_ID_STAGING` secrets and targets the checked-in staging
  ACR name constant, so no GitHub variable is needed. The environment-scoped
  `ACR_NAME` is unavailable to this job. Azure login and ACR login failures
  are fatal.
- Promotion to production is unchanged — it still copies by digest
  from the staging ACR.

## Adding a New K8s Image

1. Create Dockerfile in the app directory
2. Add to `prebuild-images.yml` `detect-changes` filter and matrix
3. Add to `deploy.yml`'s staging-stage matrix builder (detect-changes + add_target)
4. Add to `deploy.yml`'s `promote-images` production-stage image matrix (and `rollback-production.yml`'s matrix)
5. Add image to `deploy.yml`'s `generate-staging-manifest` resolve step
6. Add the image to the signed OCI release catalog and bundle inputs
7. Add the digest-pinned image reference to the OCI bundle overlay

## Manual Deploy & Promotion

Staging and production are one run, so there is no separate "promote SHA to production" dispatch:

- **Re-run production for a staging deploy that already passed:** open the `Deploy / Pipeline` run and `re-run failed jobs` (or re-run the production-stage jobs). The verified in-run manifest is reused; no SHA input needed.
- **Deploy the current `main` HEAD manually:** `gh workflow run deploy.yml` (optionally `-f force=true` to deploy every app regardless of change detection). It runs the staging stage, then pauses at the `environment: production` approval gate.
- **Roll back production to a prior known-good SHA:** use `Rollback / Production` (`gh workflow run rollback-production.yml [-f sha=<full-40-char-commit-sha>]`), which reuses that SHA's retained staging manifest. See its header for the deploy-hold procedure.
