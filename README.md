# deeds

Repo-local task graph for one human and their coding agents. One markdown
file per issue in `.deeds/`; the VCS is the database, the merge engine, and
the archive. See `docs/design.md` for rationale.

```
deeds init               # mkdir .deeds/closed
deeds setup              # AGENTS.md block + agent skill
deeds new <title>        # mint an issue, print its path
deeds ready              # open issues with no open blockers
deeds show <id> [--tree] # body + both edge directions
deeds close <id> -m "…"  # append outcome, move to closed/
deeds list / check / gc  # inventory / lint / reap old closed
```

Issues are markdown with optional frontmatter (`blocked-by: [id, …]`,
`priority: 1-9`). Location is status: `.deeds/*.md` open,
`.deeds/closed/*.md` closed. One edge type; a milestone is an issue blocked
by its parts.

Build: `nix build`, or `cargo build` in the devshell.
