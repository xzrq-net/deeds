# deeds design

A repo-local task graph for one human and his coding agents. Reaction to
Steve Yegge's beads: keep the one good idea, delete the infrastructure that
accreted around it.

## The kernel

A task graph that is co-versioned with the code, branches and merges with it,
and supports one query: what's unblocked. That is the thing `TODO.md` cannot
do (no structure, whole-file rewrites, merge magnet) and hosted trackers
cannot do (not branch-aware, network and auth ceremony, invisible to grep).
Agent memory across sessions falls out for free: the frontier is the memory
that matters.

Every candidate feature faces one test: does it earn a query that consumes
it? Beads' memories, mail, compaction, roles, and four of its five link types
all fail it.

## Decisions

**Plain files, no database.** One markdown file per issue in `.deeds/`. The
corpus is 10^2–10^4 issues; scanning frontmatter is milliseconds. Beads'
SQLite cache, export daemon, and eventual Dolt migration are all consequences
of one line-per-issue JSONL making the VCS merge at the wrong granularity.
Per-issue files make the VCS the merge engine at the right granularity.

**Branch-local, deliberately.** Issue state merges atomically with the code
that satisfies it: close an issue in the change that fixes it and trunk never
claims an unlanded fix. The cost — an issue filed on a branch is invisible
elsewhere until merge — is accepted; trunk-worthy discoveries get filed on
trunk. No sync mechanism exists because the VCS is the sync mechanism.

**Location is status.** `.deeds/*.md` is open, `.deeds/closed/*.md` is
closed. No status field, no stored-state-vs-location skew, nothing to lint.
Timestamps live in VCS history, not frontmatter.

**One edge type.** `blocked-by` is the only structural relation; it is the
sole input to `ready`. Containment collapsed into it: a milestone is an issue
blocked by its parts, and the rollup tree is the blocker DAG, which is the
truer picture anyway ("what does finishing this require" includes external
blockers). relates-to/duplicates/supersedes are markdown links in prose.
Unknown blocker IDs count as satisfied, with a warning: the referenced issue
was either GC'd or mistyped, and both are locally visible in `show`.

**Random 6-char IDs.** Minted with O_EXCL and retry against both open and
closed namespaces; 30^6 keeps cross-branch collisions and reuse of GC'd IDs
negligible over a personal tracker's lifetime (~0.1 expected reuses across
10^4 issues). No sequence counter to
conflict on, no hierarchy encoded in the ID (beads' dotted IDs make
reparenting break identity).

**Tiers, not priorities.** The one ordering signal is attention:
`objective` (the human asked) / `baseline` (default) / `nit` / `freezer`,
sorted in `ready`. Verbal, descriptive levels because "did the human ask for
this" is checkable while a numeric urgency scale invites the priority
inflation models learned from SWEs. Tiers never constrain edges; anything
may block anything.

**Decay instead of compaction.** Three tiers of retrieval cost, all plain
text: open issues in `.deeds/`, closed in `closed/` (one step of navigation
away from a browsing model), GC'd issues in VCS history only (deliberate
archaeology). `gc` is `rm`, lossless because the repo is the database — which
assumes closed files have entered history; jj colocation's continuous
snapshotting makes that automatic, plain git relies on routine commits. `gc`
keeps closed issues still named by an open issue's `blocked-by`, so live
edges stay resolvable. The directory layout is the attention policy — the
hot set stays small and salient without lossy summarization.

**Pull, not push.** No hooks, no session injection, no settings.json.
Tracker state never belongs in the prompt; the routing policy does, and it is
static: a one-line markered block upserted into AGENTS.md points at one SKILL.md
(`.agents/skills/deeds/`, symlinked into `.claude/skills/`). Two tiers with
different content — trigger vs. policy — so there is nothing to drift. Beads
pushes context at SessionStart, which is the root of its five-copies-of-
instructions sprawl: hooks per tool, fallback blocks for when hooks fail,
skills for tools without hooks, dedup between the layers.

**No claims, no assignees.** Concurrency is partitioned by worktree; two
agents taking the same issue surfaces as a one-file merge conflict at
integration, which is the correct failure: visible, textual, rare when work
is partitioned sanely. Within a worktree, one agent hierarchy coordinates in
conversation.

## Note routing (the practice half)

The skill, not the binary, carries the repo's writing policy. Four surfaces
by mutation pattern and reader distance:

| surface       | mutation                | reader                | decay             |
|---------------|-------------------------|-----------------------|-------------------|
| `.deeds/`     | append during the task  | future session        | close → gc → VCS  |
| `docs/log.md` | append-only             | future-you, narrative | never             |
| `temp/`       | overwrite freely        | this session          | discarded         |
| `docs/…`      | edit in place           | anyone, timeless      | never             |

`.deeds/` absorbs ad-hoc per-task note files — that pattern was a
proto-tracker missing its lifecycle. `log.md` is the sole sanctioned home for
session-context leaks; issue bodies must read standalone. Durable docs are a
category, not a fixed layout: projects differ, agents propose.

## Non-goals

Multiplayer, roles, claims, memories, mail, compaction, hosted-tracker sync,
web UI, MCP (the CLI over shell is the agent interface), any state outside
`.deeds/`.
