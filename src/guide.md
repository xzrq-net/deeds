# deeds

Work is a graph of markdown files: one issue per file at `.deeds/<id>.md`,
closed issues at `.deeds/closed/<id>.md`. Location is status; there is no other
state. The `deeds` CLI answers graph questions; everything else is reading and
editing files.

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

- `objective` — the user asked for it: requests, bug reports.
- `baseline` — everything else the project needs. The default.
- `nit` — drive-by nits and nice-to-haves.
- `freezer` — parked; don't pick these up unprompted.

Tiers never constrain edges: anything may block anything. Autonomy follows
tiering: objectives carry the user's words — don't reinterpret them without
asking; baseline is shared ground — file and adjust freely, but surface big
reshapes; nit and freezer are yours to file, retier, and consolidate at will.

## For you

What the user can answer now, ask in chat. What outlives the conversation
you're in goes in the issue under a `## For you` heading: a decision, an
action you can't take, a look at UX, a review of work product, when the user
is away, the answer waits on something, or the work is still landing. Write
it for them to read cold: what you need, why, your recommendation if you
have one, and where the thing to look at is. A checklist (`- [ ]`) is fine.
`deeds waiting` prints every open issue's section: that is the user's slate.
When you file one, say so in your reply, a line naming the issue.

- Asking doesn't stop the work: go on with what doesn't depend on the
  answer, or with your recommendation where the user's recorded say-so
  covers it.
- `## For you` holds only what is still asked. The user answers freeform,
  often to several at once: record each answer in the notes of the issue it
  settles, quoted and dated, outside the section, and remove what it settles
  from the section; drop the heading when nothing is left. An item that goes
  moot is removed the same way, with a line in the notes saying why.
- When the user grants autonomy ("your call", "this kind of project needs
  no review"), record their words, dated, where the grant's scope lives: the
  notes of the issue it covers, or the project's AGENTS.md for a class of
  work. Then don't ask what it covers.
- Spawned agents don't write `## For you`: they report what they'd ask, and
  whoever spawned them files it.

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
  home for session-context leaks. When it grows long, graduate to one file per
  entry under `docs/log/`.
- `temp/` — scratch for finishing the current session; disposable.
- Timeless reference goes under `docs/` — pick or extend a layout that fits the
  project. Propose, don't proliferate.

## Discipline

- Issue bodies are working notes for a future reader with zero session context:
  self-contained statements, no transcripts, no "as discussed above".
- File follow-ups the moment you notice them; don't carry them in the
  conversation.
- When to commit `.deeds/` changes is your call. Don't close an issue whose
  `## For you` still asks something: it would drop off the slate.
- Parallel agents don't pick work independently — whoever spawns them assigns
  specific issues.
- To resurrect a gc'd issue, search VCS history for `.deeds/closed/<id>.md`.
