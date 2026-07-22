# Documentation Standards

Most documentation lives in README.md files within the directories they describe.
Repository-level decision records and formal specs live under the existing
top-level `docs/` tree.

## Where to document

- App-level docs: `apps/<app>/README.md`
- Package docs: `packages/<pkg>/README.md`
- Infrastructure: `k8s/README.md`, `infra/README.md`
- Operational runbooks: `k8s/runbooks/`
- Architecture decisions: `docs/decisions/`
- Formal specifications: `docs/specs/`
- UI style guide: `apps/web/STYLE_GUIDE.md`

## Do not create new top-level `docs/` subdirectories

Use the existing `docs/decisions/` for ADRs and `docs/specs/` for formal specs.
Do not add new top-level `docs/` subdirectories without first deciding that the
content cannot be colocated with the code it describes.
