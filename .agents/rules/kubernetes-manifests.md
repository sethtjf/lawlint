---
paths:
  - "k8s/**"
---

# Kubernetes Manifests

Manifests live in `k8s/<app>/{base,overlays}`, are Kustomize-built, and applied via **Flux GitOps** — never `kubectl apply` by hand to a cluster. `k8s/api-rs/base/` is the canonical, well-formed example; copy its shape. Apply changes via PR; rollback = revert.

## Review checklist (every manifest you write or change)

- [ ] **Latest stable API version** — not `v1beta*`/deprecated. Verify with `kubectl api-resources` / `kubectl explain`.
- [ ] **No naked Pods.** Long-running → `Deployment`; run-to-completion → `Job`/`CronJob`; stable identity/storage → `StatefulSet`.
- [ ] **Resource `requests` set, and memory `limits` set** (CPU limits optional).
- [ ] **`readinessProbe` + `livenessProbe`** (add `startupProbe` for slow boots).
- [ ] **Security context**: `runAsNonRoot: true`, non-root UID, `seccompProfile: RuntimeDefault`, drop caps, prefer `readOnlyRootFilesystem`.
- [ ] **Pinned image** (tag or digest, not `:latest` in prod-bound bases).
- [ ] **Recommended labels** (`app.kubernetes.io/*`) — set via Kustomize `labels`/`commonLabels`, not hand-copied.
- [ ] **Config via ConfigMap/Secret**, not baked into the image. Secrets come from **ExternalSecrets (ESO)** — never commit plaintext secrets.
- [ ] **Resilience**: ≥2 replicas + `PodDisruptionBudget` + `topologySpreadConstraints`; `RollingUpdate` with `maxUnavailable: 0`.
- [ ] **Minimal & grouped**: drop defaults K8s already fills in; keep one app's objects together; annotate intent with `kubernetes.io/description`.
- [ ] **Validated**: `kustomize build <dir> | kubeconform -strict` and/or `kubectl apply --dry-run=server` before opening a PR.

## YAML hygiene

Quote anything boolean-ish: use `true`/`false`, and quote `"yes"`/`"no"`/`"on"`/`"off"`/`"y"`/`"n"` (the YAML 1.1 "Norway problem"). Quote version-like strings (`"3.10"`).

## Services & networking

- **DNS for discovery**: reach a Service at `<svc>.<ns>.svc.cluster.local` (this repo uses short names like `http://agentd`, `http://llm-proxy` within the namespace). Prefer DNS over injected `*_SERVICE_HOST/PORT` env vars; this repo sets `enableServiceLinks: false` to suppress them.
- **Headless Service** (`clusterIP: None`) when clients need per-Pod IPs instead of load balancing.
- **Avoid `hostPort`/`hostNetwork`** — they pin Pods to nodes and hurt scheduling/scaling. Local access: `kubectl port-forward`; external: a proper Service type / Ingress.

## Handy kubectl

```bash
kubectl api-resources                          # find the current stable API version
kubectl get pods -l app.kubernetes.io/name=X   # operate on a labeled group
kubectl port-forward deploy/web 8080:80        # local access without hostPort
kubectl diff -k k8s/api-rs/overlays/prod       # preview what an apply would change
```
