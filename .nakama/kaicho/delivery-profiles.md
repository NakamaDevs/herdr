# Delivery profiles

## Purpose and authority

Deliver useful, working software with short feedback loops. Linear records why
code changed; tests and review provide evidence. PR count, ceremonies, and
repeated reviews are not delivery goals. Prefer small changes, focused tests,
and immediate feedback, consistent with extreme programming.

This is the owner's explicit v2.1 direction. It replaces older mandatory-PR and
human-gate text for the routes below, including policy and release work. It does
not change paid services, publish credentials, authorize unrequested deployments,
or make a configured unattended harness trusted merely by adding a trailer.

## Choose the route

| Route | Default use | Delivery |
| --- | --- | --- |
| `solo` | Owner-maintained NakamaDevs repositories | Small tested commits may go directly to main; no PR or human approval required. |
| `collaborative` | WillisApp and Amanuensis | Keep existing CI/CD, PRs and human approvals for ordinary shared work. |
| `agent` | A change explicitly assigned for autonomous delivery, including in collaborative repositories | Record scope, actual validation and independent review evidence; deliver without a human review gate within that authorization. |
| `owner-override` | The owner intentionally chooses immediate delivery in any repository | May push or merge despite pending/failed checks, missing approval or open findings. Record what was skipped and any follow-up. |

The registry is `repositories/delivery.json`. Team ownership (NAK, OPS, APP,
GMD) is independent of delivery route. Future catalog tools can consume both
registries. Do not create adoption PRs across all repositories. Use this policy
on the next requested change and reconcile only the local rules that block it.
Preserve other contributors' work. Reuse existing isolated worktrees and branches;
no blanket permission to overwrite divergent main or force-push history exists.

## Commit traceability

Keep the semantic header and put the Linear reference in the body:

```text
fix(review): preserve unsaved comments

Refs NAK-913

Delivery-Mode: solo
Validation: focused note tests and full local gate passed
```

Use the real issue's team. Use `Refs` for partial work and `Fixes` for completed
work, with status behavior determined by Linear's configured automation.
Preserve the reference in the final squash commit. Before implementation,
confirm the issue's team and project using the connected Linear integration.
After publication, confirm the commit URL is attached to the intended issue.

Linear commit linking requires its magic-word toggle and a push-event webhook.
PR integration alone does not prove commit linking works. Verify a real pushed
commit before claiming automatic linking. If it is unavailable, attach the full
immutable GitHub commit URL to the issue and record the change and evidence.
This attachment is traceability; it does not recreate native commit automation.

If a published commit missed its reference, add that same attachment manually
or with Linear's attachment/link API. Do not amend, force-push, manufacture an
empty commit, or rename a published branch just to repair tracking. A PR link
can also be repaired by editing its description; PR comments do not create
native links. Never claim that an arbitrary evidence trailer triggers Linear.

## Evidence for agent delivery

Record the accountable owner, issue, allowed delivery/deployment scope, full
source revision or tree hash, and run location in Linear or a repository evidence
file. The final commit links this record. Include:

- commands actually executed, results, and environment;
- independent final-diff review, findings, fixes or explicitly accepted deferrals;
- end-to-end checks for changed behavior; exercise controls for UI changes;
- visual comparison or snapshot evidence where appearance matters;
- residual risk, skipped checks, rollback and any deployment observation.

One meaningful review pass is sufficient unless changes or unresolved findings
justify another. Multiple agents and adversarial reviews are tools, not rituals.
Do not say a harness, test, browser check or review ran when it did not.

```text
feat(player): add playback controls

Refs APP-123

Delivery-Mode: agent
Evidence: docs/evidence/APP-123.md
Validation: unit suite and playback interaction checks passed
Review: independent final-diff review; two findings fixed
Risk: native Windows was not exercised
Rollback: revert this commit
```

These trailers describe evidence. They do not automatically bypass GitHub rules,
prove the evidence, or grant a bot the owner's identity. Delegated credentials
and an unattended dispatcher must enforce the explicit scope. Do not grant broad
team, role or application bypass as a shortcut.

## Owner override and follow-up

The exact owner retains `always` bypass. Keep classic `enforce_admins` off.
GitHub reports effective rules separately from local hooks; admin privileges do
not disable a local hook. For an explicitly requested owner override, bypass
only the identified local gate, record the reason, and preserve commit identity.
Do not uninstall everyone's hooks or remove collaborator protections.

The owner may deliver now and review later. Preserve each deferred finding's
source thread, affected commit, impact, acceptance criteria, and follow-up issue.
Resolve a deferred thread only with a reply explaining the accepted deferral;
do not label it fixed. Avoid creating duplicate issues for the same finding.

`[skip ci]` may be included for an explicitly authorized skip. GitHub applies it
to push and pull_request workflows, not every trigger. Required checks can remain
pending, so it is not a substitute for bypass. Never disable CI/CD globally or
pretend skipped tests passed. A skip that prevents a desired deployment requires
an explicitly authorized deployment path; merging alone does not prove deployment.

## Sources

- [Linear GitHub integration](https://linear.app/docs/github): commit linking,
  magic words, integration setup and PR linking.
- [GitHub workflow skipping](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/skip-workflow-runs): supported events and pending required checks.
- [GitHub rulesets](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/available-rules-for-rulesets): branch protections and bypass behavior.
