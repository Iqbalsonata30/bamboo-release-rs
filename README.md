# bamboo-release

Small Rust CLI for creating and deploying Bamboo deployment versions when MCP cannot create a new release/version.

Credentials are read from environment variables and are not stored in this repo.

## Naming

By default the helper reads the Bamboo build result, gets the Bamboo branch name and build number, detects the target deployment environment name, scans existing deployment versions, then creates a name using the environment convention.

| Environment | Naming convention |
| --- | --- |
| Development / Dev | `release-<buildNumber>` |
| Prestage / Prestaging / Prerelease | `prerelease-<buildNumber>` |
| Preploy | `<branchName>-<buildNumber>` |

If the generated name already exists, the helper increments only the numeric suffix until it finds a free name.

Examples:

```text
BABMF-QCASHUIPLAFONPEGADAIAN15-3 to Prestage -> prerelease-3
BABMF-QCASHUIPLAFONPEGADAIAN15-3 to Prestage, if prerelease-3 exists -> prerelease-4
QCASHBE-QUS50-9 to Preploy on branch v1.622.1-release -> v1.622.1-release-9
```

Use `--name` only when you need to override this convention.

## Workflow

1. Trigger and poll a fresh Bamboo build with MCP, for example `QCASHBE-QUS50-9`.
2. Run this helper with that `--plan-result-key`.
3. The helper creates the next deployment version and immediately triggers deployment to `--environment-id`.
4. Poll deployment and verify OCP with MCP/Kubernetes tools.

Dev builds usually auto-trigger deployment in Bamboo, so this helper is mainly needed for prerelease/preploy when Bamboo has not created a deployment version automatically.

## Usage

```bash
export BAMBOO_URL="https://bamboo.bri.co.id"
export BAMBOO_USERNAME="<username>"
export BAMBOO_PASSWORD="<password-or-token>"

cargo run -- \
  --deployment-project-id 904529014 \
  --environment-id 904632219 \
  --plan-result-key QCASHBE-QUS50-9 \
  --description "Created for NQLA-57358"
```

Validate without calling Bamboo:

```bash
cargo run -- \
  --deployment-project-id 904529014 \
  --environment-id 904632219 \
  --plan-result-key QCASHBE-QUS50-9 \
  --name v1.622.1-release-9 \
  --dry-run
```

Auto-name dry-run still calls Bamboo because it must read the build branch and existing deployment versions. Use `--name` in dry-run mode when you only want to inspect the request shape without network calls.
