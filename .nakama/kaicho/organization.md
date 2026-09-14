# Organization and work ownership

NakamaDevs is a two-person software development company. Linear teams describe
work ownership, not separate companies, headcount, file types, or technologies.
Kaicho owns this contract. Repository instructions and Linear configuration must
agree with it.

| Team | Owns | Examples |
|---|---|---|
| NAK — Nakamadevs | Engineering, shared infrastructure, internal developer tools, research, experiments, and technical guidance | Kaicho, Docs, SDLC, Bunko, Aspire integrations, Runner Fleet, Symphony, identity and authorization services |
| OPS — Operations | How NakamaDevs runs its business: hiring, interviews, finances, contracts, and business processes | Kahvi Tax, Kahvi Finance, SAT, Stingray; conducting interviews |
| APP — Apps | Application products intended for users or future commercialization | Amanuensis, Mensetsukan, Karekano, Bolsillo, WillisApp |
| GMD — GameDev | Games and tools specific to games | Pawys, Maraton, GameJam 2503 |

Building an internal developer tool belongs to NAK even when its name includes
"operations" or it runs infrastructure. Kahvi, SAT, and Stingray are explicitly
OPS tools for the company's own business. Building the Mensetsukan application
belongs to APP; using it to interview a candidate belongs to OPS. Track shared
engineering dependencies separately in NAK and relate the issues.

Documentation belongs to the work it explains. Cross-repository technical
documentation, SDLC guidance, research, shared skills, and engineering work
records belong to NAK. Product documentation stays with APP or GMD. Business
procedures belong to OPS. A documentation file does not require a DOC issue.
Keep the existing DOC team available for future repurposing; do not create new
documentation work there or delete its historical records.

## Repository and project routing

Use `repositories/ownership.json` as the routing inventory. A repository can
have multiple delivery projects when they represent distinct outcomes, such as
the two Aspire language integrations. Do not create duplicate projects merely
to change a team's ownership. Use stable project IDs to distinguish duplicates.

Every adopting repository records the correct team and project in both
`.branch-policy` and `.nakama/repository.toml`. Its instructions, examples,
templates, scripts, and CI must use that same routing. The shared checker
supports one exact project per profile. A repository with several delivery
projects must supply and test a repository-owned verifier for its exact project
set before claiming automated routing enforcement. Until then, its owner must
verify the selected project through Linear for each issue and record that
manual gate. Do not put a list in the singular `project` field or borrow another
repository's issue.

Use `<type>/<TEAM>-123-short-description` for new branches. The prefix comes
from the actual Linear issue, not an assumed repository category. Confirm the
issue exists in the expected project before creating the branch. Put `Fixes
TEAM-123` in a completing PR body or `Refs TEAM-123` for partial work.

## Migration procedure

The repository owner performs and records the following checks in a Linear
issue and a reviewable PR:

1. Read the project's stable ID, linked repository, teams, issues, and open PRs.
2. Save the original mapping. Add the destination team and set it as lead.
3. Move open issues individually, preserving status, assignee, project,
   milestones, and relations. Check team-specific labels and cycles because
   Linear can change them during a team move. Record old ID, UUID, and new ID.
4. Preserve completed and canceled issue identifiers as historical references.
   Keep the former team associated only while historical issues or explicitly
   recorded transition work require it. Lead ownership defines new intake.
5. Repair open PR links using the new issue identifier. Preserve branch names
   and reviewer discussions. Verify actual issue attachment and visible diff.
6. Adopt repository policy and exact legacy-branch exceptions before enforcing
   the new naming gate. Record consumer adoption status; a policy proposal is
   not evidence that the default branch has adopted it.
7. Retire duplicate projects only after their issues, links, and milestones are
   reconciled. Keep an audit trail and a pointer to the canonical project.

Changing project teams does not itself move issues or configure GitHub access.
Review automation is configured per team. See
[Linear and GitHub automation](https://github.com/NakamaDevs/Kaicho/blob/91ec23b858c2d6aa74a987901592c7493259e89d/docs/governance/linear-github-automation.md).

If a move fails, stop that migration, reread the exact target, and correct only
the demonstrated problem. Keep the original mapping for recovery. Do not remove
a former team if that would detach historical issues from their project.
