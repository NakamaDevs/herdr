# Nakamadevs engineering governance v2

Policy version: **2.1.0**

This file is the canonical v2 engineering contract for Nakamadevs repositories. Repositories keep a versioned local mirror for offline use. Clarifications update this file in place with a patch or minor policy version. Incompatible governance starts a new `engineering-vN.md` contract and is adopted deliberately.

Version 2.0 replaces identity-bearing branch names with one issue-first form.
People and agents now use the same branch form. Version 2.0 also adds privacy
controls for personally identifiable information (PII) and protected health
information (PHI). It also replaces legacy administrator enforcement with an
exact owner bypass. These changes are incompatible with v1 branch enforcement.

A repository adopts v2 only when it updates its local mirror, profile,
instructions, checker, fixtures, and hooks in one pull request. Before
adoption, record each identity-bearing branch for an open pull request in the
exact `LEGACY_BRANCH_ALLOWLIST` in `.branch-policy`. The hooks and CI accept
only those recorded v1 branches. Remove an entry after its pull request closes.
Create every new branch with the v2 form after adoption. Rollback restores the
last adopted v1 mirror, profile, instructions, checker, fixtures, and hooks as
one unit.

Version 1.7 adds workload-class runner routing as a compatible, deliberately
adopted minor revision. It does not establish a universal operating-system
default. A repository pinned to an earlier v1 minor keeps that declared
contract until it updates its mirror, workflow, context version, and executable
checker together. The repository catalog records that migration; repositories
must not claim 1.7 adoption while retaining pre-1.7 routing.

Version 1.8 adds the Vercel Skills catalog as a compatible, deliberately
adopted minor revision. Repositories must update `governance_version`, the
skills source, the exact catalog revision, and the contract checker together.
Migration replaces the former plugin marketplace record with
`NakamaDevs/skills` and `npx skills add`. Rollback restores the previous
catalog record and checker from the last adopted revision.

Version 1.9 adds the harness and supervised orchestration controls as a
compatible, deliberately adopted minor revision. The controls apply only after
a repository declares agent-ready status or starts a supervised pilot. A
repository that declares `false` keeps its current obligations. Version 1.9
extends the existing `agent-contract` check with the `harness.agent_ready`
declaration. It adds no new checker command, no workflow change, and no runner
label. A profile at 1.8.0 or earlier stays valid without the `[harness]` table.
Migration updates the mirror, `governance_version`, and the declaration
together. Rollback sets the declaration to `false` and restores the previous
governance version.

The detailed operating model remains in [`docs/governance/agentic-sdlc.md`](https://github.com/NakamaDevs/Kaicho/blob/91ec23b858c2d6aa74a987901592c7493259e89d/docs/governance/agentic-sdlc.md) and the companion documents under `docs/governance/`. This contract is the concise, versioned adoption surface; detailed documents provide rationale and procedures.

## Owner maintenance

The lead administrator is the GitHub user `hectorddmx`, user ID `303818`.
Each active protection ruleset must give this exact `User` actor an `always`
bypass. Do not grant this bypass to a role, team, bot, deploy key, or GitHub
App. Other users remain subject to repository protection.

Never enable classic branch-protection `enforce_admins`. This mechanism cannot
express a user-specific exception. It can also override the exact ruleset
bypass.

The lead administrator can commit, merge, push, or perform owner maintenance
without self-approval. Use this access only for an intentional repository
operation. Keep the GitHub ruleset audit history.

## Delivery profiles (2.1)

Version 2.1 adds the owner-approved delivery profiles in
[delivery-profiles.md](https://github.com/NakamaDevs/Kaicho/blob/91ec23b858c2d6aa74a987901592c7493259e89d/docs/governance/delivery-profiles.md). That document
replaces inherited requirements for mandatory PRs, human approval before every
merge, and resolving every review finding before delivery for solo work and
explicitly authorized agent delivery. Collaborative work retains its normal
CI/CD and approval flow. The exact owner's intentional override remains
available in every profile, including when checks fail or are skipped.

Every delivered commit includes a supported Linear magic word and issue ID in
its body. Preserve Conventional Commit headers. The commit-msg checker enforces
this on adoption; it does not retroactively reject published history. Repair a
missed link through an immutable commit attachment in Linear, not a history
rewrite. Shared policies and releases do not automatically require new PRs.

The [delivery registry](https://github.com/NakamaDevs/Kaicho/blob/91ec23b858c2d6aa74a987901592c7493259e89d/repositories/delivery.json) records the owner's
classification, separately from the [team routing policy](https://github.com/NakamaDevs/Kaicho/blob/91ec23b858c2d6aa74a987901592c7493259e89d/governance/organization.md).
WillisApp and Amanuensis are collaborative. Other owner-maintained NakamaDevs
repositories default to solo, per the owner's explicit declaration. This is a
policy default, not a claim that every repository's hooks have been migrated.
Apply the current policy during the next requested work; do not manufacture
adoption PRs. A standalone unattended harness still requires explicit scope,
credentials, and dispatch authorization; a commit trailer grants no permission.

Pinned v2.0 copies retain their old behavior until adoption. For this authorized
rollout, update the central policy and apply it to current work without a new
approval loop. Rollback restores the prior profile and checks for future work;
it must not rewrite commits, undo issue moves, or remove the owner's bypass.

## Delivery contract

Every non-trivial change follows research, plan, implement (RPI):

1. **Research** — read repository instructions, the linked Linear issue, nearby code, tests, release paths, and current CI. Record unknowns and risks before editing.
2. **Plan** — define scope, acceptance evidence, stack-specific validation, rollout and rollback. Keep the plan in the Linear issue or PR when coordination benefits from it.
3. **Implement** — make the smallest cohesive change, keep generated files reproducible, run the validation ladder, review the diff independently, then publish evidence.

Tiny documentation corrections may collapse the written plan, but must still be inspected and validated proportionately.

## Linear and branches

Linear is the delivery ledger. Link work to an issue in the repository's Linear project before implementation unless handling an incident. Incident work must be backfilled.

The default branch is `main`. It blocks routine direct pushes, force pushes,
and deletion. Changes merge through pull requests after required checks and
conversations complete. The exact lead administrator keeps the `always` bypass
for commits, merges, pushes, and owner maintenance.

Branch form for people and agents:

```text
<type>/NAK-123-short-description
```

`<type>` is one Conventional Commit type: `feat`, `fix`, `docs`, `refactor`, `test`, `build`, `ci`, `chore`, `perf`, `style`, or `revert`. `feature` stays valid as an alias of `feat`. Pick the type that describes the change, the same way you pick the type of the commit header.

Lowercase the description, use hyphens, and match the issue to the repository's Linear project. Repositories may add types only through a documented policy revision.

Do not add a username, initials, account name, tool name, model name, or agent
namespace. Linear, the pull request, and Git commit metadata record ownership.
The branch name records the work.

Repositories may add an isolated-worktree requirement through repository-local
instructions when their orchestration environment supports it. Kaicho's own
agent instructions require Herdr-managed linked worktrees and define creation,
sandbox boundaries, dirty-work recovery, cleanup, and rollback in
[`docs/governance/herdr-worktrees.md`](https://github.com/NakamaDevs/Kaicho/blob/91ec23b858c2d6aa74a987901592c7493259e89d/docs/governance/herdr-worktrees.md).
That Kaicho-local tool requirement is not an additional v2 adoption gate for
other repositories.

## Commits and history

Use Conventional Commits: `type(optional-scope): imperative summary`. Supported types are `feat`, `fix`, `docs`, `refactor`, `test`, `build`, `ci`, `chore`, `perf`, `style`, and `revert`. Keep the header at 100 characters or fewer. The commit-message hook checks local commits, and CI checks the PR title so squash-merge history remains semantic. Explain behavior, motivation, and breaking changes in the body/footer when needed.

Keep commits cohesive and the protected branch linear. Never rewrite someone else's published history without explicit coordination.

## Validation ladder

Validate in increasing cost and stop on failure:

1. Policy and static checks: branch, commit, formatting, lint, generated-file drift, workflow syntax.
2. Focused tests for the changed behavior.
3. Repository quick verification.
4. Full repository verification and required platform matrix.
5. Build/package/release checks when artifact or deployment behavior changes.

Local hooks provide fast feedback but are not a security boundary. The branch-format gate runs before commit and push. CI validates the actual PR head branch against the policy already present on the base branch. A v2 adoption PR can use a valid v1 or v2 branch form when the trusted base policy has no `LEGACY_BRANCH_ALLOWLIST` key. After merge, only exact names in `LEGACY_BRANCH_ALLOWLIST` keep the v1 exception. CI is authoritative. Record commands and outcomes in the PR. Explain omissions with risk and follow-up. Never claim a check that was not run.

## Independent review

Review the final diff from the base branch, not only the implementation narrative. Use the repository's review skill to load the relevant stack checklist. Prioritize correctness, security, data loss, concurrency, compatibility, operational failure, and missing tests over style. Findings identify the file/location, failure mode, impact, and a practical fix. If there are no findings, say so and note residual risks or untested platforms.

Every repository keeps `.github/CODEOWNERS` with `@NakamaDevs/devs` as the broad fallback owner. Put narrower subsystem rules after the fallback so the most specific matching rule wins. Named users and teams must have write access to the repository. Main-branch protection requires at least one approving review from a code owner; the PR author cannot satisfy that approval. GitHub evaluates the CODEOWNERS file on the PR base branch, so ownership changes take effect after the change lands.

The author resolves every conversation before merge. A reviewer must not rely solely on generated summaries or passing checks.

## Pull requests and CI

PRs link the Linear issue and describe scope, RPI evidence, risk, validation, release/deployment impact, rollback, and follow-up. Draft PRs may run CI on `opened`, `synchronize`, and `reopened`; do not add `ready_for_review` when those events already cover the same commit. Avoid duplicate workflow triggers and redundant matrix jobs.

CI must be reproducible through repository-owned commands, pin or deliberately manage tool versions, use least-privilege permissions, avoid secret exposure, and publish enough failure context to reproduce locally. Required gates should map to stable aggregate jobs so branch protection is maintainable. NakamaDevs workflows run only on explicitly selected organization self-hosted capability labels; they do not fall back to GitHub-hosted runners. Untrusted fork code must not reach persistent runners.

Runner selection follows the workload contract; no operating system is the
universal default. Native macOS work runs on `nakama-macos-arm64`, native
Windows work runs on `nakama-windows-x64`, cross-platform desktop work covers
all supported desktop operating-system lanes (`nakama-macos-arm64`,
`nakama-linux-x64`, and `nakama-windows-x64`), and server-only work runs on the
local `nakama-linux-x64` lane. Portable non-server work can select one active
persistent capability, including `nakama-linux-arm64`. The job must support the
selected operating system and architecture. Active Windows validation is
x64-only; Windows ARM64 runner labels are retired and rejected. Jobs declare
their class with `# nakama-workload:` for the repository-owned runner-policy
checker.

Dependabot internal updates and every job executing a Dependabot-authored pull
request use an implemented disposable capability. Platform-specific jobs use
the matching operating system and architecture. A `portable-non-server` job
can use any implemented disposable capability that the job supports.
Metadata-only gates use only
`nakama-untrusted-metadata-linux-x64` and do not check out or execute repository
code. Pull-request authorship is classified with
`github.event.pull_request.user.login`, never `github.actor` or
`github.triggering_actor`, so synchronize, reopen, and maintainer activity do
not change the trust decision. Missing disposable capacity queues or blocks;
it never selects a persistent label.

## Versioning, artifacts, and deployment

Use Semantic Versioning for released products. Automation derives versions from Conventional Commits or an explicitly documented release decision. A release identifies the source commit, immutable artifact checksums, build/toolchain provenance, and release notes. Never replace a published artifact under the same version.

Deployment changes define environment, approval boundary, compatibility/migration order, health signal, rollback procedure, and post-deploy observation window. Prefer promotion of the same verified artifact across environments. Roll back or mitigate when health gates fail, then document the incident and follow-up in Linear.

## Supply-chain security

Follow [`governance/supply-chain-security.md`](https://github.com/NakamaDevs/Kaicho/blob/91ec23b858c2d6aa74a987901592c7493259e89d/governance/supply-chain-security.md). Routine dependency and tool resolution excludes releases newer than seven days, while CI and releases consume committed immutable locks, checksums, digests, and full-SHA Actions references. Security exceptions are explicit, narrow, independently approved, and evidence-backed; scanners and package age never replace review.

## Secret management

Follow [`governance/secret-management.md`](https://github.com/NakamaDevs/Kaicho/blob/91ec23b858c2d6aa74a987901592c7493259e89d/governance/secret-management.md). Secrets stay out of Git and use short-lived, least-privilege storage and delivery. Repositories enable GitHub secret scanning and push protection where supported, run the pinned and integrity-locked Gitleaks CLI against staged changes and complete history, redact all findings, and prove the detector with a synthetic canary. Suspected leaks are revoked and rotated before history cleanup; scanner allowlists are narrow, explained, and code-owner reviewed.

## Privacy and regulated data

Follow [`governance/privacy.md`](https://github.com/NakamaDevs/Kaicho/blob/91ec23b858c2d6aa74a987901592c7493259e89d/governance/privacy.md). Do not put real PII or PHI in
prompts, branches, commits, issues, pull requests, review comments, logs,
telemetry, screenshots, recordings, fixtures, examples, artifacts, releases,
or public Gists. Use synthetic data and reserved example values.

Collect and retain only the data required for an approved purpose. Record the
owner, classification, allowed systems, access boundary, retention period, and
deletion method before processing sensitive data. Give agents only bounded,
redacted context. A detector is supporting evidence. It does not prove that
content is free of PII or PHI.

Stop publication when sensitive data enters a delivery record. Restrict
access, notify the responsible privacy and security owners, preserve redacted
evidence, and follow the approved incident process. Rewrite published history
only with explicit authorization and a verified target set.

## Agent workflows and skills

Use `AGENTS.md` for durable repository and subtree instructions, versioned Agent Skills for reusable workflows, and repository-owned scripts, hooks, CI, rulesets, and CODEOWNERS for enforcement. Authenticated external actions belong in approved connectors or MCP servers, not in prose or credentials embedded in skills.

Kaicho defines skill governance requirements. `NakamaDevs/skills` publishes reusable Agent Skills. Consumer repositories install selected skills with `npx skills add` and record the source and exact catalog commit in `.nakama/repository.toml`. Do not copy shared catalog skills into consumer repositories.

See [`docs/governance/agent-skills.md`](https://github.com/NakamaDevs/Kaicho/blob/91ec23b858c2d6aa74a987901592c7493259e89d/docs/governance/agent-skills.md) for the distribution, versioning, adoption, and user-setup model.

## Agent-ready repositories and supervised orchestration

A repository can declare that it is agent-ready. A supervised orchestrator can
then read issues from Linear and start agent runs for that repository. Both
steps are deliberate. Neither is a default.

An agent-ready repository keeps the knowledge that the agent needs inside the
repository. It enforces its architecture invariants with executable checks. It
runs each rung of its validation ladder from one documented command.

A supervised pilot records its approval, sandbox, network, credential, and
external-effect posture before the first run. Each run uses one isolated
workspace and stops at a human review handoff. A human code owner approves
every merge. Autonomous merge, release, deployment, and production writes stay
prohibited.

A repository declares agent-ready status with the `harness.agent_ready` boolean
in `.nakama/repository.toml`. A profile at 1.9.0 or later must carry the
`[harness]` table. A profile at 1.8.0 or earlier can omit it and must not set
`true`. `governance_version` must be an exact release version without a
prerelease suffix or build metadata. Dispatch rejects a missing declaration and
a `false` declaration.

Each run attempt keeps an audit record outside process memory. A named operator
can stop one run and stop all dispatch with one action, without a code change.
The reviewing code owner is not the operator and not the deputy.

[`docs/governance/harness-and-orchestration-controls.md`](https://github.com/NakamaDevs/Kaicho/blob/91ec23b858c2d6aa74a987901592c7493259e89d/docs/governance/harness-and-orchestration-controls.md)
defines the controls, the required evidence, the owners, the entry gates, the
exit gates, and the failure handling.

## Repository adoption

Each repository must provide:

- a concise root `AGENTS.md` that points to its local governance mirror and repository review skill;
- `.nakama/repository.toml`, a pinned Agent Skills source, and the cross-platform `scripts/nakama_governance.py` contract check in CI;
- `.branch-policy`, branch and commit checkers, tests, and installed local hooks;
- a PR template and CI that call repository-owned validation commands;
- the v1.7 runner-policy command and tests, explicit workload-class routing,
  complete supported-OS coverage for cross-platform desktop work, metadata-only
  isolation, portable non-server capability checks, and author-based disposable
  Dependabot routing;
- `.github/CODEOWNERS` with the organization reviewer-team fallback and protected-main code-owner approval;
- a seven-day dependency cooldown, immutable resolution, full-SHA Actions, and repository-owned supply-chain validation;
- GitHub push protection where supported plus repository-owned staged and history secret scanning with a synthetic canary;
- `.gitleaks.toml` with only narrow documented exceptions and a response procedure that prioritizes revocation;
- a privacy review for public text, fixtures, logs, telemetry, screenshots,
  recordings, and generated artifacts;
- `harness.agent_ready` in `.nakama/repository.toml` for every profile at
  1.9.0 or later;
- the harness and supervised orchestration controls when the repository
  declares agent-ready status or runs a supervised pilot;
- stack-specific review guidance and release/deployment commands where applicable;
- a README link to governance, contribution gates, and agent skills.

Keep instructions token-efficient: route first, load only the changed stack's reference, avoid duplicating long platform guidance in `AGENTS.md`, and prefer executable commands over prose.
