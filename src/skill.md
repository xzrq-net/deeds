---
name: deeds
description: Task tracking in .deeds/ and this repo's note-routing policy. Read before starting, finishing, or filing work.
---

# deeds

Work is a graph of markdown files: one issue per file at `.deeds/<id>.md`,
closed issues at `.deeds/closed/<id>.md`. Location is status; there is no
other state. The `deeds` CLI answers graph questions; everything else is
reading and editing files.

## Issues

A `# Title` heading, then body. Optional frontmatter, only for blockers and
non-default tier:

```
---
blocked-by: [x9f2mk]
tier: nit
---
# Adopt the new config loader

Notes accumulate here as work proceeds.
```

A milestone is an ordinary issue blocked by its parts.

## Tiers

Tier is attention, not urgency: where the issue sits in the information
hierarchy for whoever reads `ready` next.

- `objective` — the human asked for it: requests, bug reports.
- `baseline` — everything else the project needs. The default.
- `nit` — drive-by nits and nice-to-haves.
- `freezer` — parked; don't pick these up unprompted.

Tiers never constrain edges: anything may block anything.

## Sizing

- Right-sized: one VCS change can close it. Bigger than that is a milestone —
  file it blocked by its parts, splitting as parts become clear, not up front.
- Smaller than that — a step of the current task — is conversation or `temp/`,
  not an issue. File only what a future session would act on.
- Clear scope you can state in the title: file it without asking. Fuzzy or
  strategic scope, or a pile of related findings: ask before filing.

## Commands

```
deeds ready              # unblocked open issues, tier order — pick from the top
deeds new <title>        # mint an issue, print its path; edit the file from there
deeds show <id>          # body plus blocked-by and blocks; --tree for the DAG
deeds close <id> -m "…"  # append "Closed: …", move to closed/
deeds list               # open issues; --closed to include closed
deeds check              # lint: parse errors, duplicate ids, cycles, unknown blockers
deeds gc                 # delete issues closed >30 days ago; VCS history retains them
```

## Routing: where writing goes

- `.deeds/` — anything that changes what to do next.
- `docs/log.md` — append-only narrative of what happened. The only sanctioned
  home for session-context leaks.
- `temp/` — scratch for finishing the current session; disposable.
- Timeless reference goes under `docs/` — pick or extend a layout that fits
  the project. Propose, don't proliferate.

## Discipline

- Issue bodies are working notes for a future reader with zero session
  context: self-contained statements, no transcripts, no "as discussed above".
- File follow-ups the moment you notice them; don't carry them in the
  conversation.
- Close an issue in the same VCS change as the work that satisfies it.
- Parallel agents don't pick work independently — whoever spawns them assigns
  specific issues.
- To resurrect a gc'd issue, search VCS history for `.deeds/closed/<id>.md`.
