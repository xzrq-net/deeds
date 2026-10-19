mod graph;
mod issue;
mod setup;
mod tui;

use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{ArgValueCompleter, CompleteEnv, CompletionCandidate};
use serde_json::json;

use graph::Graph;
use issue::{DEFAULT_TIER, Issue, find_deeds_dir, load_all, mint_id, tier_name};

const GUIDE: &str = include_str!("guide.md");

#[derive(Parser)]
#[command(about = "repo-local task graph; see docs/design.md", version)]
struct Cli {
    /// JSON output (query commands)
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create or refresh .deeds/ and the AGENTS.md block
    Init,
    /// Print the agent guide: workflow, tiers, note routing
    Skill,
    /// Mint an issue and print its path
    New {
        /// Title words
        #[arg(required = true)]
        title: Vec<String>,
        /// Blocker ids
        #[arg(short, long = "blocked-by", value_delimiter = ',')]
        #[arg(add = ArgValueCompleter::new(open_issue_ids))]
        blocked_by: Vec<String>,
        /// Attention tier; default baseline
        #[arg(short, long, value_parser = clap::builder::PossibleValuesParser::new(issue::TIERS))]
        tier: Option<String>,
    },
    /// Open issues with no open blockers
    Ready,
    /// All open issues; --closed to include closed
    List {
        #[arg(long)]
        closed: bool,
    },
    /// Print an issue plus both edge directions
    Show {
        #[arg(add = ArgValueCompleter::new(|current: &OsStr| issue_ids(current, true)))]
        id: String,
        /// Recursive blocker DAG instead of the flat footer
        #[arg(long)]
        tree: bool,
    },
    /// Move to closed/; -m appends an outcome line first
    Close {
        #[arg(add = ArgValueCompleter::new(open_issue_ids))]
        id: String,
        /// Outcome, appended to the body as "Closed: …"
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Lint the graph; nonzero exit on errors
    ///
    /// Errors: malformed frontmatter, bad tier, malformed blocker ids,
    /// duplicate ids across open/closed, blocked-by cycles.
    /// Warnings: missing title, unknown blocker ids (treated as satisfied).
    Check,
    /// Delete closed issues untouched for --days; history is the archive
    Gc {
        #[arg(long, default_value_t = 30)]
        days: u64,
    },
    /// Browse issues interactively (for humans)
    Tui,
}

fn main() -> Result<()> {
    CompleteEnv::with_factory(Cli::command).complete();
    let cli = Cli::parse();
    let json = cli.json;
    match cli.cmd {
        Cmd::Init => setup::init(&std::env::current_dir()?),
        Cmd::Skill => {
            print!("{GUIDE}");
            Ok(())
        }
        Cmd::New {
            title,
            blocked_by,
            tier,
        } => new(&title.join(" "), &blocked_by, tier.as_deref(), json),
        Cmd::Ready => ready(json),
        Cmd::List { closed } => list(closed, json),
        Cmd::Show { id, tree } => show(&id, tree, json),
        Cmd::Close { id, message } => close(&id, message.as_deref()),
        Cmd::Check => check(json),
        Cmd::Gc { days } => gc(days),
        Cmd::Tui => tui::run(),
    }
}

fn open_issue_ids(current: &OsStr) -> Vec<CompletionCandidate> {
    issue_ids(current, false)
}

fn issue_ids(current: &OsStr, with_closed: bool) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return vec![];
    };
    // Completion stays quiet outside a tracker or when its files can't be read.
    let Ok(issues) = find_deeds_dir().and_then(|dir| load_all(&dir)) else {
        return vec![];
    };
    issues
        .into_iter()
        .filter(|i| (with_closed || !i.closed) && i.id.starts_with(prefix))
        .map(|i| CompletionCandidate::new(i.id).help(Some(i.title.into())))
        .collect()
}

fn new(title: &str, blocked_by: &[String], tier: Option<&str>, json: bool) -> Result<()> {
    let deeds = find_deeds_dir()?;
    for id in blocked_by {
        if id.len() != issue::ID_LEN || !id.bytes().all(|b| issue::ID_ALPHABET.contains(&b)) {
            bail!("malformed blocker id: {id:?}");
        }
    }
    // Unknown blockers stay satisfied-with-warning everywhere else; filing
    // time is the cheapest moment to catch the typo.
    let existing = load_all(&deeds)?;
    let known = Graph::new(&existing);
    for id in blocked_by {
        if !known.by_id.contains_key(id.as_str()) {
            eprintln!("warning: unknown blocker {id} (gc'd or typo), treated as satisfied");
        }
    }

    let mut frontmatter = String::new();
    if !blocked_by.is_empty() {
        frontmatter += &format!("blocked-by: [{}]\n", blocked_by.join(", "));
    }
    if let Some(t) = tier
        && t != tier_name(DEFAULT_TIER)
    {
        frontmatter += &format!("tier: {t}\n");
    }
    let body = if frontmatter.is_empty() {
        format!("# {title}\n")
    } else {
        format!("---\n{frontmatter}---\n# {title}\n")
    };

    // O_EXCL retry handles same-dir collisions; closed/ is checked so a
    // closed issue's id is not reused while it is still visible.
    let (id, path) = loop {
        let id = mint_id();
        let path = deeds.join(format!("{id}.md"));
        if deeds.join("closed").join(format!("{id}.md")).exists() {
            continue;
        }
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(body.as_bytes())?;
                break (id, path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("create {}", path.display())),
        }
    };

    let path = relative(&path);
    if json {
        println!("{}", json!({"id": id, "path": path}));
    } else {
        println!("{id} {path}");
    }
    Ok(())
}

/// Cosmetic lints don't affect the graph; everything else might.
fn edge_relevant(problem: &str) -> bool {
    !problem.starts_with("no '# Title'")
}

/// Graph readers must not silently act on files they misread: surface parse
/// problems on stderr instead of waiting for an explicit `check` run.
fn warn_problems(issues: &[Issue]) {
    for issue in issues {
        for problem in issue.problems.iter().filter(|p| edge_relevant(p)) {
            eprintln!("warning: {}: {problem}", issue.id);
        }
    }
}

fn ready(json: bool) -> Result<()> {
    let issues = load_all(&find_deeds_dir()?)?;
    warn_problems(&issues);
    let g = Graph::new(&issues);
    let ready = g.ready();
    for issue in &ready {
        warn_unknown(&g, issue);
    }
    if json {
        let rows: Vec<_> = ready.iter().map(|i| issue_json(i, &g)).collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        print_table(&ready, &g);
    }
    Ok(())
}

fn list(with_closed: bool, json: bool) -> Result<()> {
    let issues = load_all(&find_deeds_dir()?)?;
    warn_problems(&issues);
    let g = Graph::new(&issues);
    let mut rows: Vec<&Issue> = issues.iter().filter(|i| with_closed || !i.closed).collect();
    rows.sort_by_key(|i| (i.closed, i.tier, i.id.clone()));
    if json {
        let rows: Vec<_> = rows.iter().map(|i| issue_json(i, &g)).collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        print_table(&rows, &g);
    }
    Ok(())
}

fn show(id: &str, tree: bool, json: bool) -> Result<()> {
    let issues = load_all(&find_deeds_dir()?)?;
    warn_problems(&issues);
    let g = Graph::new(&issues);
    let Some(&issue) = g.by_id.get(id) else {
        bail!("no issue {id}; gc'd issues live in VCS history");
    };
    warn_unknown(&g, issue);

    if json {
        let mut row = issue_json(issue, &g);
        row["text"] = json!(issue.text);
        if tree {
            let mut out = String::new();
            g.tree(id, &mut out);
            row["tree"] = json!(out);
        }
        println!("{}", serde_json::to_string_pretty(&row)?);
        return Ok(());
    }
    print!("{}", issue.text);
    println!("--");
    println!("{} [{}] {}", issue.id, state(issue), relative(&issue.path));
    if tree {
        let mut out = String::new();
        g.tree(id, &mut out);
        print!("{out}");
    } else if !issue.blocked_by.is_empty() {
        let fmt = |id: &String| match g.by_id.get(id.as_str()) {
            Some(b) => format!("{id} [{}]", state(b)),
            None => format!("{id} [gone]"),
        };
        println!(
            "blocked-by: {}",
            issue
                .blocked_by
                .iter()
                .map(fmt)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let blocks = g.blocks(id);
    if !blocks.is_empty() {
        println!(
            "blocks: {}",
            blocks
                .iter()
                .map(|b| format!("{} [{}]", b.id, state(b)))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

fn close(id: &str, message: Option<&str>) -> Result<()> {
    let deeds = find_deeds_dir()?;
    let issues = load_all(&deeds)?;
    let g = Graph::new(&issues);
    let Some(&issue) = g.by_id.get(id) else {
        bail!("no issue {id}")
    };
    if issue.closed {
        bail!("{id} already closed");
    }

    let open = g.open_blockers(issue);
    if !open.is_empty() {
        let ids: Vec<_> = open.iter().map(|b| b.id.as_str()).collect();
        eprintln!(
            "warning: closing {id} with open blockers: {}",
            ids.join(", ")
        );
    }

    if let Some(msg) = message {
        // Append-mode: concurrent edits since load_all are not clobbered.
        let mut f = fs::OpenOptions::new().append(true).open(&issue.path)?;
        let lead = if issue.text.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        f.write_all(format!("{lead}Closed: {msg}\n").as_bytes())?;
    }
    let dest = deeds.join("closed").join(format!("{id}.md"));
    if dest.exists() {
        bail!("{} already exists; duplicate id", relative(&dest));
    }
    fs::create_dir_all(deeds.join("closed"))?;
    fs::rename(&issue.path, &dest).with_context(|| format!("move to {}", dest.display()))?;
    // mtime is gc's "time since close"; a plain rename would keep the old one.
    fs::File::options()
        .append(true)
        .open(&dest)?
        .set_modified(std::time::SystemTime::now())?;
    println!("closed {id} -> {}", relative(&dest));
    Ok(())
}

fn check(json: bool) -> Result<()> {
    let issues = load_all(&find_deeds_dir()?)?;
    let g = Graph::new(&issues);
    let mut errors: Vec<String> = vec![];
    let mut warnings: Vec<String> = vec![];

    let mut seen = std::collections::HashMap::new();
    for issue in &issues {
        if let Some(prev) = seen.insert(&issue.id, issue) {
            errors.push(format!(
                "duplicate id {}: {} and {}",
                issue.id,
                relative(&prev.path),
                relative(&issue.path)
            ));
        }
        for problem in &issue.problems {
            let dest = if edge_relevant(problem) {
                &mut errors
            } else {
                &mut warnings
            };
            dest.push(format!("{}: {problem}", issue.id));
        }
        if issue.id.len() != issue::ID_LEN
            || !issue.id.bytes().all(|b| issue::ID_ALPHABET.contains(&b))
        {
            warnings.push(format!("{}: filename not a minted id", issue.id));
        }
        if !issue.closed {
            for unknown in g.unknown_blockers(issue) {
                warnings.push(format!(
                    "{}: unknown blocker {unknown} (gc'd or typo), treated as satisfied",
                    issue.id
                ));
            }
        }
    }
    if let Some(cycle) = g.find_cycle() {
        errors.push(format!("blocked-by cycle: {}", cycle.join(" -> ")));
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"errors": errors, "warnings": warnings}))?
        );
    } else {
        for e in &errors {
            println!("error: {e}");
        }
        for w in &warnings {
            println!("warning: {w}");
        }
        if errors.is_empty() && warnings.is_empty() {
            println!("ok: {} issues", issues.len());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        std::process::exit(1)
    }
}

fn gc(days: u64) -> Result<()> {
    let deeds = find_deeds_dir()?;
    let issues = load_all(&deeds)?;
    // The reference guard below reads parsed edges, so parse problems in open
    // issues could hide a textual reference. Refuse to delete under lints.
    warn_problems(&issues);
    if issues
        .iter()
        .any(|i| !i.closed && i.problems.iter().any(|p| edge_relevant(p)))
    {
        bail!("open issues have lints; run `deeds check` and fix before gc");
    }
    // Closed issues still named by an open issue's blocked-by stay resolvable
    // until the dependent closes or drops the edge.
    let referenced: std::collections::HashSet<&str> = issues
        .iter()
        .filter(|i| !i.closed)
        .flat_map(|i| i.blocked_by.iter().map(String::as_str))
        .collect();
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86400);
    let mut reaped = 0;
    for issue in issues.iter().filter(|i| i.closed) {
        if issue.path.metadata()?.modified()? >= cutoff {
            continue;
        }
        if referenced.contains(issue.id.as_str()) {
            println!("kept {} (blocker of an open issue)", issue.id);
            continue;
        }
        fs::remove_file(&issue.path)?;
        println!("reaped {}", relative(&issue.path));
        reaped += 1;
    }
    if reaped == 0 {
        println!("nothing to reap");
    }
    Ok(())
}

fn state(issue: &Issue) -> &'static str {
    if issue.closed { "closed" } else { "open" }
}

fn warn_unknown(g: &Graph, issue: &Issue) {
    for unknown in g.unknown_blockers(issue) {
        eprintln!(
            "warning: {}: unknown blocker {unknown} (gc'd or typo), treated as satisfied",
            issue.id
        );
    }
}

fn issue_json(issue: &Issue, g: &Graph) -> serde_json::Value {
    json!({
        "id": issue.id,
        "title": issue.title,
        "tier": tier_name(issue.tier),
        "closed": issue.closed,
        "blocked_by": issue.blocked_by,
        "blocks": g.blocks(&issue.id).iter().map(|b| b.id.clone()).collect::<Vec<_>>(),
        "path": relative(&issue.path),
    })
}

fn print_table(rows: &[&Issue], g: &Graph) {
    for issue in rows {
        let open_blockers = g.open_blockers(issue);
        let mut flags = String::new();
        if issue.tier != DEFAULT_TIER {
            flags += &format!(" {}", tier_name(issue.tier));
        }
        if issue.closed {
            flags += " closed";
        } else if !open_blockers.is_empty() {
            flags += &format!(
                " blocked({})",
                open_blockers
                    .iter()
                    .map(|b| b.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        println!("{}{flags:<12} {}", issue.id, issue.title);
    }
}

fn relative(path: &Path) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    path.strip_prefix(&cwd)
        .unwrap_or(path)
        .display()
        .to_string()
}
