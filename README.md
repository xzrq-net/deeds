# deeds

Repo-local task graph for one human and his coding agents. One markdown
file per issue in `.deeds/`; the VCS is the database, the merge engine, and
the archive. See `docs/design.md` for rationale.

```
deeds init               # .deeds/ + AGENTS.md block; rerun to refresh
deeds skill              # print the agent guide
deeds new <title>        # mint an issue, print its path
deeds ready              # open issues with no open blockers
deeds show <id> [--tree] # body + both edge directions
deeds close <id> -m "…"  # append outcome, move to closed/
deeds list / check / gc  # inventory / lint / reap old closed
```

Issues are markdown with optional frontmatter (`blocked-by: [id, …]`,
`tier: objective|baseline|nit|freezer`). Location is status: `.deeds/*.md`
open, `.deeds/closed/*.md` closed. One edge type; a milestone is an issue
blocked by its parts. Query commands take a global `--json`.

Build: `nix build`, or `cargo build` in the devshell.
