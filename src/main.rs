mod graph;
mod issue;
mod setup;
mod tui;
mod vcs;

use std::ffi::OsStr;
use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{ArgValueCompleter, CompleteEnv, CompletionCandidate};
use serde_json::json;

use graph::Graph;
use issue::{DEFAULT_TIER, Issue, find_deeds_dir, load_all, locate, mint_id, tier_name};

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
    /// Create or refresh .deeds/ and the AGENTS.md block; with DEEDS_DIR
    /// set, create that tracker and its git repo instead
    Init,
    /// Print the agent guide: workflow, tiers, note routing
    Skill,
    /// Mint an issue and print its path; the body comes from stdin unless
    /// it is a terminal
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
    /// What open issues ask of the human: their `## For you` sections
    Waiting,
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
    /// Log the close (with -m's outcome) and move to closed/
    Close {
        #[arg(add = ArgValueCompleter::new(open_issue_ids))]
        id: String,
        /// Outcome, logged as "Closed: …"
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Append stdin to the issue's `## Log` as a dated, stamped entry
    Note {
        #[arg(add = ArgValueCompleter::new(|current: &OsStr| issue_ids(current, true)))]
        id: String,
    },
    /// Commit pending tracker edits; a no-op unless the tracker is its own repo
    Commit,
    /// Lint the graph; nonzero exit on errors
    ///
    /// Errors: malformed frontmatter, bad tier, malformed blocker ids,
    /// duplicate ids across open/closed, blocked-by cycles.
    /// Warnings: missing title, unknown blocker ids (treated as satisfied).
    Check,
    /// Browse issues interactively (for humans)
    Tui,
}

fn main() -> Result<()> {
    CompleteEnv::with_factory(Cli::command).complete();
    let cli = Cli::parse();
    let json = cli.json;
    match cli.cmd {
        Cmd::Init => match issue::env_deeds_dir()? {
            Some(dir) => setup::init_standalone(&dir),
            None => setup::init(&std::env::current_dir()?),
        },
        Cmd::Skill => {
            // No tracker, no policy: an agent asking here learns deeds isn't
            // in use, rather than reading a guide for a tracker that isn't
            // there. Not "run `deeds init`": that's the user's call.
            let deeds = find_deeds_dir().map_err(|e| match issue::env_deeds_dir() {
                Ok(None) => {
                    anyhow::anyhow!("no tracker: no .deeds/ here or above, and no DEEDS_DIR")
                }
                _ => e,
            })?;
            print!("{}", guide(standalone(&deeds)));
            Ok(())
        }
        Cmd::New {
            title,
            blocked_by,
            tier,
        } => {
            let deeds = find_deeds_dir()?;
            // Stdin is read before mutate takes the lock: a writer that never
            // closes it must not block other calls.
            let body = stdin_text()?.unwrap_or_default();
            let title = title.join(" ");
            mutate(&deeds, || {
                new(&deeds, &title, &body, &blocked_by, tier.as_deref(), json)
            })
        }
        Cmd::Note { id } => {
            let deeds = find_deeds_dir()?;
            let Some(text) = stdin_text()? else {
                bail!("the note comes from stdin: deeds note {id} <<'EOF'");
            };
            mutate(&deeds, || note(&deeds, &id, &text, json))
        }
        Cmd::Close { id, message } => {
            let deeds = find_deeds_dir()?;
            mutate(&deeds, || close(&deeds, &id, message.as_deref()))
        }
        Cmd::Commit => {
            if let Some(message) = vcs::commit_pending(&find_deeds_dir()?)? {
                println!("{message}");
            }
            Ok(())
        }
        // Reads don't snapshot: they stay side-effect free and lock-free (the
        // tui polls, completion runs per keystroke), and pending edits are
        // committed by the next write or the turn-end `deeds commit`.
        Cmd::Ready => ready(json),
        Cmd::Waiting => waiting(json),
        Cmd::List { closed } => list(closed, json),
        Cmd::Show { id, tree } => show(&id, tree, json),
        Cmd::Check => check(json),
        Cmd::Tui => tui::run(),
    }
}

/// Run a write under the tracker's autocommit, if it has one: commit what
/// is pending, make the change, commit it with the command line as message.
fn mutate(deeds: &Path, run: impl FnOnce() -> Result<()>) -> Result<()> {
    let Some(session) = vcs::Autocommit::begin(deeds)? else {
        return run();
    };
    run()?;
    session.finish(&command_line())
}

fn command_line() -> String {
    let quote = |arg: String| {
        if arg.is_empty() || arg.contains(char::is_whitespace) {
            format!("\"{arg}\"")
        } else {
            arg
        }
    };
    let args: Vec<String> = std::env::args().skip(1).map(quote).collect();
    format!("deeds {}", args.join(" "))
}

/// Standalone: the tracker lives outside the project, a repo of its own.
/// Paths and commit advice both follow `DEEDS_DIR`; one not at a repo root
/// (which `deeds init` would have made) gets the advice wrong.
fn standalone(deeds: &Path) -> bool {
    std::env::var_os("DEEDS_DIR").is_some_and(|v| !v.is_empty()) || vcs::autocommits(deeds)
}

/// The guide for one mode: blocks between `<!-- inline -->` or
/// `<!-- standalone -->` and `<!-- end -->` belong to that mode only. The
/// markers are HTML comments so markdown formatters leave them on their own
/// lines; they want a blank line on each side, so the blank after a marker
/// goes with it.
fn guide(standalone: bool) -> String {
    let mut keep = true;
    let mut after_marker = false;
    let mut out = String::new();
    for line in GUIDE.lines() {
        let marker = match line.trim() {
            "<!-- inline -->" => Some(!standalone),
            "<!-- standalone -->" => Some(standalone),
            "<!-- end -->" => Some(true),
            _ => None,
        };
        if let Some(mode_keeps) = marker {
            keep = mode_keeps;
            after_marker = true;
            continue;
        }
        let skip = after_marker && line.trim().is_empty();
        after_marker = false;
        if keep && !skip {
            out += line;
            out.push('\n');
        }
    }
    out
}

/// Stdin's text, or None when it is a terminal.
fn stdin_text() -> Result<Option<String>> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Ok(None);
    }
    let mut text = String::new();
    stdin.read_to_string(&mut text).context("read stdin")?;
    Ok(Some(text))
}

fn open_issue_ids(current: &OsStr) -> Vec<CompletionCandidate> {
    issue_ids(current, false)
}

/// Closed issues offered by completion: the latest logged, by count rather
/// than age, so the candidates don't change while nothing is closed.
const COMPLETE_CLOSED: usize = 10;

fn issue_ids(current: &OsStr, with_closed: bool) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return vec![];
    };
    // Completion stays quiet outside a tracker or when its files can't be read.
    let Ok(issues) = locate().and_then(|(dir, _)| load_all(&dir)) else {
        return vec![];
    };
    let (open, mut closed): (Vec<Issue>, Vec<Issue>) = issues.into_iter().partition(|i| !i.closed);
    // By the last log date, which `close` writes; same-day ties by id.
    closed.sort_by(|a, b| b.last_logged.cmp(&a.last_logged).then(a.id.cmp(&b.id)));
    closed.truncate(if with_closed { COMPLETE_CLOSED } else { 0 });
    open.into_iter()
        .chain(closed)
        .filter(|i| i.id.starts_with(prefix))
        .map(|i| CompletionCandidate::new(i.id).help(Some(i.title.into())))
        .collect()
}

fn new(
    deeds: &Path,
    title: &str,
    body: &str,
    blocked_by: &[String],
    tier: Option<&str>,
    json: bool,
) -> Result<()> {
    for id in blocked_by {
        if id.len() != issue::ID_LEN || !id.bytes().all(|b| issue::ID_ALPHABET.contains(&b)) {
            bail!("malformed blocker id: {id:?}");
        }
    }
    // Unknown blockers stay satisfied-with-warning everywhere else; filing
    // time is the cheapest moment to catch the typo.
    let existing = load_all(deeds)?;
    let known = Graph::new(&existing);
    for id in blocked_by {
        if !known.by_id.contains_key(id.as_str()) {
            eprintln!("warning: unknown blocker {id}, treated as satisfied");
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
    let mut text = if frontmatter.is_empty() {
        format!("# {title}\n")
    } else {
        format!("---\n{frontmatter}---\n# {title}\n")
    };
    let body = body.trim_end().trim_start_matches(['\n', '\r']);
    if !body.is_empty() {
        text += &format!("\n{body}\n");
    }

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
                f.write_all(text.as_bytes())?;
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
        // The id is the file stem; printing it separately would break $(deeds new).
        println!("{path}");
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

fn waiting(json: bool) -> Result<()> {
    let issues = load_all(&find_deeds_dir()?)?;
    warn_problems(&issues);
    let g = Graph::new(&issues);
    let mut rows: Vec<&Issue> = issues
        .iter()
        .filter(|i| !i.closed && i.for_you.is_some())
        .collect();
    rows.sort_by_key(|i| (i.tier, i.id.clone()));
    if json {
        let rows: Vec<_> = rows.iter().map(|i| issue_json(i, &g)).collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("nothing waits on you");
    }
    let width = flags_width(&rows, &g);
    for (k, issue) in rows.iter().enumerate() {
        if k > 0 {
            println!();
        }
        print_row(issue, &g, width);
        for line in issue.for_you.as_deref().unwrap_or_default().lines() {
            println!("  {line}");
        }
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
        bail!("no issue {id}");
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

fn note(deeds: &Path, id: &str, text: &str, json: bool) -> Result<()> {
    if text.trim().is_empty() {
        bail!("empty note");
    }
    let issues = load_all(deeds)?;
    let Some(issue) = issues.iter().find(|i| i.id == id) else {
        bail!("no issue {id}");
    };
    append_log(issue, text)?;
    let path = relative(&issue.path);
    if json {
        println!("{}", json!({"id": id, "path": path}));
    } else {
        // The id is the file stem; printing it separately would break $(deeds new).
        println!("{path}");
    }
    Ok(())
}

/// Add a stamped entry to the issue's `## Log`. When the log is already last
/// this only appends, so an edit made since the issue was read survives.
fn append_log(issue: &Issue, text: &str) -> Result<()> {
    let origin = vcs::origin(&std::env::current_dir()?);
    let entry = issue::log_entry(&vcs::today()?, origin.as_deref(), text);
    let updated = issue::with_log_entry(&issue.text, &entry);
    let path = &issue.path;
    match updated.strip_prefix(issue.text.as_str()) {
        Some(tail) => fs::OpenOptions::new()
            .append(true)
            .open(path)
            .and_then(|mut f| f.write_all(tail.as_bytes())),
        None => {
            // Moving the log rewrites the file: refuse if it changed since read.
            let now =
                fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
            if now != issue.text {
                bail!("{} changed while being updated; retry", path.display());
            }
            fs::write(path, &updated)
        }
    }
    .with_context(|| format!("write {}", path.display()))
}

fn close(deeds: &Path, id: &str, message: Option<&str>) -> Result<()> {
    let issues = load_all(deeds)?;
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

    if issue.for_you.is_some() {
        eprintln!(
            "warning: closing {id} with its '## {}' section still asking",
            issue::FOR_YOU
        );
    }

    // Always logged: the entry's date is the close date (completion, tui).
    let entry = match message {
        Some(msg) => format!("Closed: {msg}"),
        None => "Closed.".to_string(),
    };
    append_log(issue, &entry)?;
    let dest = deeds.join("closed").join(format!("{id}.md"));
    if dest.exists() {
        bail!("{} already exists; duplicate id", relative(&dest));
    }
    fs::create_dir_all(deeds.join("closed"))?;
    fs::rename(&issue.path, &dest).with_context(|| format!("move to {}", dest.display()))?;
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
        if issue.closed && issue.for_you.is_some() {
            warnings.push(format!(
                "{}: closed but still has a '## {}' section",
                issue.id,
                issue::FOR_YOU
            ));
        }
        if !issue.closed {
            for unknown in g.unknown_blockers(issue) {
                warnings.push(format!(
                    "{}: unknown blocker {unknown}, treated as satisfied",
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

fn state(issue: &Issue) -> &'static str {
    if issue.closed { "closed" } else { "open" }
}

fn warn_unknown(g: &Graph, issue: &Issue) {
    for unknown in g.unknown_blockers(issue) {
        eprintln!(
            "warning: {}: unknown blocker {unknown}, treated as satisfied",
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
        "for_you": issue.for_you,
    })
}

fn print_table(rows: &[&Issue], g: &Graph) {
    let width = flags_width(rows, g);
    for issue in rows {
        print_row(issue, g, width);
    }
}

/// The flags column fits its rows' tiers and marks; a long blocker list
/// overflows its own row only.
fn flags_width(rows: &[&Issue], g: &Graph) -> usize {
    let widest = rows.iter().map(|i| flags(i, g).len()).max().unwrap_or(0);
    widest.clamp(12, 20)
}

fn print_row(issue: &Issue, g: &Graph, width: usize) {
    println!("{}{:<width$} {}", issue.id, flags(issue, g), issue.title);
}

fn flags(issue: &Issue, g: &Graph) -> String {
    let open_blockers = g.open_blockers(issue);
    let mut flags = String::new();
    if issue.tier != DEFAULT_TIER {
        flags += &format!(" {}", tier_name(issue.tier));
    }
    if !issue.closed && issue.for_you.is_some() {
        flags += " for-you";
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
    flags
}

fn relative(path: &Path) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    path.strip_prefix(&cwd)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::guide;

    #[test]
    fn guide_drops_markers_and_their_blank_lines() {
        for standalone in [false, true] {
            let text = guide(standalone);
            assert!(!text.contains("<!-- "), "{text}");
            assert!(!text.contains("\n\n\n"), "{text}");
        }
    }
}
