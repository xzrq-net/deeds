# deeds

Repo-local task graph for one human and his coding agents. One markdown
file per issue in `.deeds/`; the VCS is the database, the merge engine, and
the archive. See `docs/design.md` for rationale.

```
deeds init               # .deeds/ + AGENTS.md block; rerun to refresh
deeds skill              # print the agent guide
deeds new <title>        # mint an issue (body from stdin), print its path
deeds note <id>          # append stdin to the issue's ## Log, dated
deeds ready              # open issues with no open blockers
deeds waiting            # what open issues ask of the human (## For you)
deeds show <id> [--tree] # body + both edge directions
deeds close <id> -m "…"  # log the outcome, move to closed/
deeds list / check / gc  # inventory / lint / reap old closed
deeds commit             # standalone: commit pending edits
deeds tui                # browse: blocker tree, ready, recently closed
```

Issues are markdown with optional frontmatter (`blocked-by: [id, …]`,
`tier: objective|baseline|nit|freezer`). Location is status: `.deeds/*.md`
open, `.deeds/closed/*.md` closed. One edge type; a milestone is an issue
blocked by its parts. Query commands take a global `--json`.

Standalone trackers live outside any project, in a git repo of their own:
`DEEDS_DIR=/abs/path deeds init` creates one, and with `DEEDS_DIR` set every
call uses it over any `.deeds/` (saying so on stderr). A tracker that is the
root of its own repo commits itself: each write first commits pending direct
edits as a snapshot, then its change with the command line as message;
`deeds commit` does just the snapshot. `deeds skill` adapts to the mode.

Build: `nix build`, or `cargo build` in the devshell.
