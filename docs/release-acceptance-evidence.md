# Release Acceptance Evidence Template

Use this template for every public release candidate. Store filled reports under
`.release-evidence/` or another untracked artifact location, then attach the
redacted report, logs, hashes, screenshots, and recordings to the GitHub release
or internal release ticket.

Do not paste provider keys, MCP keys, bootstrap secrets, user-key hashes,
pairing secrets, stream tickets, access tokens, screenshots containing secrets,
or raw provider/MCP configuration files into this document.

## Candidate

| Field | Value |
| --- | --- |
| Release version | |
| Git commit | |
| Release engineer | |
| Date / timezone | |
| Windows release machine | |
| Relay host | |
| Runner host | |
| Mobile/PWA devices | |
| Provider matrix owner | |

## 14.1 Base Gates

| Gate | Command or workflow | Result | Evidence |
| --- | --- | --- | --- |
| Rust format | `cargo fmt --all -- --check` | | |
| Git whitespace | `git diff --check` | | |
| Cargo check slices | `python scripts/cargo_workspace_slice.py check <slice>` for `claude`, `codex`, `roo`, `apps-shared` | | |
| Cargo clippy slices | `python scripts/cargo_workspace_slice.py clippy <slice>` for `claude`, `codex`, `roo`, `apps-shared` | | |
| Cargo audit | `cargo audit --quiet` | | |
| Gitleaks | `gitleaks detect --source . --redact` | | |
| GUI install | `npm ci` in `apps/remote-code-gui` | | |
| GUI audit | `npm audit --audit-level=moderate --registry=https://registry.npmjs.org/` | | |
| GUI tests | `npm test` | | |
| GUI build | `npm run build` | | |
| Windows desktop bundle | `npm run desktop:build` | | |

## 14.2 End-To-End Acceptance

| Scenario | Required evidence | Result | Evidence |
| --- | --- | --- | --- |
| CLI headless | Prompt, stream-json transcript, parser result, sanitized log | | |
| TUI | Launch screenshot/recording, provider/tool/permission/session checks | | |
| Desktop GUI | New project/session, one agent prompt, permission dialog, sanitized logs | | |
| Codex GUI | Thread/turn/app-server notifications entering GUI | | |
| Roo GUI | In-process Roo prompt, permission behavior, known-limitation notes | | |
| Control Plane | `/healthz`, `/v1/meta`, register, heartbeat, create, events stream | | |
| Runner | Outbound register, command pull, local `remote-code`, event upload, approval relay | | |
| Mobile/PWA | Pairing, restore, prompt, interrupt, approval, artifact, timeline, refresh auth | | |
| Remote Transport | Relay, Direct WS, Outbound Poll, QUIC E2E and failure diagnostics | | |
| Tailscale optional path | Tailnet direct detect/manual/disabled/fallback/E2EE/approval/artifact | | |
| Provider matrix | MiniMax, KuaiKAT, and DeepSeek across Claude/Codex/Roo paths, timing/cost/tool/diff notes | | |
| MCP matrix | MiniMax/context7/sequentialthinking/memory/puppeteer startup, health, discovery, call, failure, redaction | | |
| Secure deployment | Relay host contains only control plane and no source tree/provider keys/workspaces | | |
| Release packages | Windows installer install/launch; Linux relay and Web/PWA deployable artifacts | | |

### Tailscale Evidence Report

When `REMOTE_CODE_ACCEPTANCE_TAILSCALE_ENABLED` is set, attach a separate
redacted report with `-TailscaleEvidenceReport`. The acceptance script requires
these machine-readable markers:

```text
PASS: tailnet-direct
PASS: manual-strategy
PASS: disabled-path
PASS: failure-fallback
PASS: e2ee
PASS: approval
PASS: artifact
PASS: acl-device-trust
```

Do not include any `FAIL:`, `SKIP:`, or `MANUAL:` marker in the attached
Tailscale report for a public release candidate.

## 17 Release Boundary Checklist

| Boundary item | Result | Evidence |
| --- | --- | --- |
| Base gates and CI-required slices passed | | |
| Desktop first launch brings embedded runner online | | |
| Mobile/PWA pairing, refresh, prompt, approval, artifact download passed | | |
| Relay host only runs control plane | | |
| Relay host contains no source directory or provider keys | | |
| Strong `REMOTE_CODE_CONTROL_PLANE_BOOTSTRAP_SECRET` configured | | |
| User-key hash source and rotation policy recorded when enabled | | |
| Query access token legacy switch remains off unless exception recorded | | |
| Default remote transport is `smart`; manual override wins | | |
| Relay, Direct WS, Outbound Poll, QUIC completed declared E2E | | |
| Tailscale ACL/device-trust advice recorded when Tailscale is enabled | | |
| MiniMax, KuaiKAT, and DeepSeek provider release records complete | | |
| MiniMax, context7, sequentialthinking, memory, puppeteer MCP release records complete | | |
| Docs, reports, logs, screenshots, recordings, exports, Git files contain no real secrets | | |

## Recommended Command

```powershell
powershell -ExecutionPolicy Bypass -File scripts\acceptance-release.ps1 `
  -RunBaseGates `
  -IncludeWorkspaceTests `
  -IncludeDesktopBundle `
  -IncludeProviderMatrix `
  -IncludeMcpMatrix `
  -IncludeRemoteE2E `
  -IncludeMobilePwaE2E `
  -IncludeTransportE2E `
  -IncludeTailscaleE2E `
  -RelayHostAuditReport .\relay-host-audit.txt `
  -RequireComplete `
  -UseProxy
```

The script writes a sanitized report and logs under `.release-evidence/`.
`-RequireComplete` makes `FAIL`, `SKIP`, and `MANUAL` statuses blocking for
the requested gates. Public release candidates must use the full command above
so every release gate is requested. A disabled optional Tailscale path may close
as `N/A`; an enabled tailnet path must attach a separate redacted evidence
report with `-TailscaleEvidenceReport` containing the required `PASS:` markers
above. Relay host boundary checks must come from
`deploy/tencent-cloud/audit-relay-host.sh` running on the relay host.
