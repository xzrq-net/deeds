# deeds

<!-- inline -->

Work is a graph of markdown files: one issue per file at `.deeds/<id>.md`,
closed issues at `.deeds/closed/<id>.md`. Location is status; there is no other
state.

<!-- standalone -->

Work is a graph of markdown files in a tracker outside the project, a git repo
of its own (`$DEEDS_DIR`): one issue per file at `<id>.md`, closed issues at
`closed/<id>.md`. Location is status; there is no other state.

<!-- end -->

The `deeds` CLI answers graph questions, and writes new issues and log entries;
everything else is reading and editing files.

## Issues

A `# Title` heading, then body. Optional frontmatter, only for blockers and
non-default tier:

```
---
blocked-by: [x9f2mk]
tier: nit
---
# Adopt the new config loader

The current state: what, why, decisions so far.

## For you

- [ ] Pick the config format.

## Log

- 2026-11-01 app@eng-config: Loader in; the old one stays as a fallback.
```

- The body is the current state; rewrite it freely as things change.
- `## Log` is the last section, append-only. `deeds note` adds dated entries
  stamped with the checkout they came from.
- Write with `deeds new <<'EOF'` (the whole file, as above) and
  `deeds note <id> <<'EOF'`; edit the file directly to rewrite the body.

```
deeds new <<'EOF'
---
tier: nit
---
# Adopt the new config loader

The current state: what, why, decisions so far.
EOF
```

A milestone is an ordinary issue blocked by its parts.

## Tiers

Tier is attention, not urgency: where the issue sits in the information
hierarchy for whoever reads `ready` next.

- `objective` — the user asked for it: requests, bug reports.
- `baseline` — everything else the project needs. The default.
- `nit` — drive-by nits and nice-to-haves.
- `freezer` — parked; don't pick these up unprompted.

Tiers never constrain edges: anything may block anything. Autonomy follows
tiering: objectives carry the user's words — don't reinterpret them without
asking; baseline is shared ground — file and adjust freely, but surface big
reshapes; nit and freezer are yours to file, retier, and consolidate at will.

## "For you"

What an issue needs from the user beyond the current conversation (decisions,
actions, reviews) goes in its `## For you` section. Tooling like `deeds waiting`
presents this content to the user. Format each item as a - [ ] markdown
checkbox. The user will respond in the conversation or inline in the file.
Remove completed items from the section and absorb them.

## Sizing

- An issue can represent a variety of scopes. The boundaries are a judgment
  call.
  - Tasks: issues are roughly VCS commit / GitHub PR in size.
  - Semantic fencing: distinct pieces of work deserve separate issues.
  - Session continuity: if work might get handed off between sessions, it's an
    issue.
  - Organizational: milestones and epics are issues blocked by their parts —
    split as parts become clear, not up front.
  - Attention saver: I noticed this unrelated thing, I filed it as an issue, I
    don't have to think about it anymore. A later context can consolidate.
- Filing autonomy follows clarity of scope: if you can state it in the title,
  file it without asking. Fuzzy or strategic scope, or a pile of related
  findings — ask first.
- Harness task lists and `temp/` notes are disposable intra-session state;
  anything durable gets spilled to issues or docs so progress can't slip
  backward.

## Commands

```
deeds ready              # unblocked open issues, tier order — pick from the top
deeds waiting            # open issues' `## For you` sections: the user's slate
deeds new                # mint an issue from the file on stdin; print its path
deeds note <id>          # append stdin to its ## Log
deeds show <id>          # body plus blocked-by and blocks; --tree for the DAG
deeds close <id> -m "…"  # log "Closed: …", move to closed/
deeds list               # open issues; --closed to include closed
deeds check              # lint: parse errors, duplicate ids, cycles, unknown blockers
```

## Routing: where writing goes

<!-- inline -->

- `.deeds/` — anything that changes what to do next; an issue's progress goes in
  its `## Log`.

<!-- standalone -->

- The tracker — anything that changes what to do next; an issue's progress goes
  in its `## Log`. The rest below belongs to the project you are working in.

<!-- end -->

- `docs/log.md` — the project's append-only narrative: what happened and why,
  for its later readers. An issue's `## Log` leaves with the issue; what the
  project should keep goes here. When it grows long, graduate to one file per
  entry under `docs/log/`.
- `temp/` — scratch for finishing the current session; disposable.
- Timeless reference goes under `docs/` — pick or extend a layout that fits the
  project. Propose, don't proliferate.

## Discipline

- Issue bodies are working notes for a future reader with zero session context:
  self-contained statements, no transcripts, no "as discussed above".
- File follow-ups the moment you notice them; don't carry them in the
  conversation.

<!-- inline -->

- Close an issue in the same VCS change as the work that satisfies it.

<!-- standalone -->

- deeds commits the tracker itself, your direct edits included; issues never go
  in project commits.
- Where feasible, include a durable commit ID (CL, main branch git rev; not
  git-colocated jj rev) in a closing note.
- References run one way: the tracker names project commits and files; the
  project's code, docs and commit messages never name an issue.

<!-- end -->

- Parallel agents don't pick work independently — whoever spawns them assigns
  specific issues.
