use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Attention tiers, most to least salient; index is the sort rank.
/// Verbal and descriptive on purpose: "did the human ask for this" is
/// checkable, "how important is this" invites inflation.
pub const TIERS: [&str; 4] = ["objective", "baseline", "nit", "freezer"];
pub const DEFAULT_TIER: u8 = 1; // baseline

pub fn tier_rank(name: &str) -> Option<u8> {
    TIERS.iter().position(|t| *t == name).map(|p| p as u8)
}

pub fn tier_name(rank: u8) -> &'static str {
    TIERS[rank as usize]
}

/// Lowercase, no confusables (i l o u / 0 1 excluded).
pub const ID_ALPHABET: &[u8] = b"abcdefghjkmnpqrstvwxyz23456789";
// 6 chars: 30^6 = 729M keeps cross-branch collisions negligible (expected
// ~0.1 across 10^4 lifetime issues).
pub const ID_LEN: usize = 6;

#[derive(Debug)]
pub struct Issue {
    pub id: String,
    pub title: String,
    pub blocked_by: Vec<String>,
    pub tier: u8,
    pub closed: bool,
    pub path: PathBuf,
    pub text: String,
    /// The body's `## For you` section: what the issue needs from the
    /// human, written for them. None when absent or empty.
    pub for_you: Option<String>,
    /// The latest date among `## Log` entries (`YYYY-MM-DD`): when the issue
    /// last moved, and for a closed one, about when it closed.
    pub last_logged: Option<String>,
    /// Lint findings from parsing; surfaced by `check`, ignored elsewhere.
    pub problems: Vec<String>,
}

pub fn parse(id: &str, path: &Path, text: &str, closed: bool) -> Issue {
    let mut issue = Issue {
        id: id.to_string(),
        title: String::new(),
        blocked_by: vec![],
        tier: DEFAULT_TIER,
        closed,
        path: path.to_path_buf(),
        text: text.to_string(),
        for_you: section(text, FOR_YOU),
        last_logged: section(text, LOG).and_then(|log| last_logged(&log)),
        problems: vec![],
    };

    // Problems found on a line name it, 1-based, so `new` and `check` can
    // point at the fix.
    let mut lines = text.lines().enumerate().map(|(n, l)| (n + 1, l)).peekable();
    if lines.peek().map(|(_, l)| l.trim()) == Some("---") {
        lines.next();
        let mut terminated = false;
        for (n, line) in lines.by_ref() {
            if line.trim() == "---" {
                terminated = true;
                break;
            }
            let mut found = vec![];
            match line.split_once(':') {
                None if line.trim().is_empty() => {}
                None => found.push(format!("frontmatter line without ':': {line:?}")),
                Some((key, value)) => match key.trim() {
                    // Repeated keys extend rather than replace: a merge that
                    // leaves two blocked-by lines must not drop edges.
                    "blocked-by" | "blocked_by" => {
                        let ids = parse_id_list(value, &mut found);
                        issue.blocked_by.extend(ids);
                    }
                    "tier" => match tier_rank(value.trim()) {
                        Some(r) => issue.tier = r,
                        None => found.push(format!(
                            "tier not one of {}: {:?}",
                            TIERS.join("/"),
                            value.trim()
                        )),
                    },
                    // Unknown keys are advisory (wip: etc.) and carry no semantics.
                    _ => {}
                },
            }
            let found = found.into_iter().map(|p| format!("line {n}: {p}"));
            issue.problems.extend(found);
        }
        if !terminated {
            issue
                .problems
                .push("line 1: frontmatter opened here is never closed".into());
        }
    }

    match lines.find_map(|(_, l)| l.strip_prefix("# ")) {
        Some(t) if !t.trim().is_empty() => issue.title = t.trim().to_string(),
        _ => {
            issue.title = id.to_string();
            issue.problems.push("no '# Title' heading".into());
        }
    }
    issue
}

pub const FOR_YOU: &str = "For you";

/// The body's `#` and `##` headings as (line index, level, title), past
/// code fences: ``` or ~~~, up to three spaces in, closed by a run of the
/// same character at least as long.
fn headings(text: &str) -> Vec<(usize, usize, &str)> {
    let mut fence: Option<(char, usize)> = None;
    let mut out = vec![];
    for (k, line) in text.lines().enumerate() {
        let rest = line.trim_start_matches(' ');
        let near = line.len() - rest.len() < 4;
        let run = |c: char| rest.chars().take_while(|&b| b == c).count();
        if let Some((c, n)) = fence {
            if near && run(c) >= n && rest.trim_start_matches(c).trim().is_empty() {
                fence = None;
            }
            continue;
        }
        // A ``` line with more backticks after the run is a code span.
        let opens = |c: char| {
            near && run(c) >= 3 && (c == '~' || !rest.trim_start_matches(c).contains('`'))
        };
        if let Some(c) = ['`', '~'].into_iter().find(|&c| opens(c)) {
            fence = Some((c, run(c)));
            continue;
        }
        let level = line.chars().take_while(|&c| c == '#').count();
        if (level == 1 || level == 2) && line[level..].starts_with(' ') {
            let title = line[level..].trim();
            // An optional closing run of #s, as in `## For you ##`.
            let bare = title.trim_end_matches('#');
            let title = if bare.is_empty() || bare.ends_with(' ') {
                bare.trim()
            } else {
                title
            };
            out.push((k, level, title));
        }
    }
    out
}

fn is_section(heading: &(usize, usize, &str), name: &str) -> bool {
    heading.1 == 2 && heading.2.eq_ignore_ascii_case(name)
}

/// A `## <name>` section's text, up to the next `#` or `##` heading. None
/// when absent, or holding nothing but whitespace and HTML comments.
fn section(text: &str, name: &str) -> Option<String> {
    let heads = headings(text);
    let at = heads.iter().position(|h| is_section(h, name))?;
    let start = heads[at].0 + 1;
    let end = heads.get(at + 1).map_or(usize::MAX, |h| h.0);
    let lines: Vec<&str> = text.lines().take(end).skip(start).collect();
    let body = lines.join("\n");
    let body = body.trim();
    (!without_comments(body).trim().is_empty()).then(|| body.to_string())
}

pub const LOG: &str = "Log";

/// The latest entry date in a `## Log` section's text. Entries are
/// unindented `- YYYY-MM-DD …` items; the latest rather than the last, so a
/// hand-reordered log still reads right.
fn last_logged(log: &str) -> Option<String> {
    log.lines()
        .filter_map(|line| line.strip_prefix("- ")?.get(..10))
        .filter(|date| {
            date.bytes().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
        })
        .max()
        .map(String::from)
}

/// `text` with `entry` appended to its `## Log`, which ends up the last
/// section: created if missing, moved to the end if anything follows it.
/// Only the move rewrites; otherwise `text` is a prefix of the result.
pub fn with_log_entry(text: &str, entry: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let heads = headings(text);
    let Some(at) = heads.iter().position(|h| is_section(h, LOG)) else {
        let lead = match text {
            "" => "",
            t if t.ends_with("\n\n") => "",
            t if t.ends_with('\n') => "\n",
            _ => "\n\n",
        };
        return format!("{text}{lead}## {LOG}\n\n{entry}");
    };
    if at + 1 == heads.len() {
        let lead = if text.ends_with('\n') { "" } else { "\n" };
        return format!("{text}{lead}{entry}");
    }
    let start = heads[at].0;
    let end = heads[at + 1].0;
    let mut rest = [&lines[..start], &lines[end..]].concat();
    let mut log = lines[start..end].to_vec();
    trim_blank_tail(&mut rest);
    trim_blank_tail(&mut log);
    if log.len() == 1 {
        log.push("");
    }
    let head = if rest.is_empty() {
        String::new()
    } else {
        rest.join("\n") + "\n\n"
    };
    head + &log.join("\n") + "\n" + entry
}

fn trim_blank_tail(lines: &mut Vec<&str>) {
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
}

/// A `## Log` list item: date, the checkout it came from, then the text,
/// its later lines indented under the first.
pub fn log_entry(date: &str, origin: Option<&str>, text: &str) -> String {
    let stamp = match origin {
        Some(origin) => format!("{date} {origin}"),
        None => date.to_string(),
    };
    // Leading blank lines go, leading indentation stays: every line shifts
    // by the same two columns.
    let text = text.trim_end().trim_start_matches(['\n', '\r']);
    // A fence opening after the stamp would close on an indented line, which
    // `headings` reads as a new fence: start such a note on its own line.
    let first = text.lines().next().unwrap_or_default().trim_start();
    let mut out = if first.starts_with("```") || first.starts_with("~~~") {
        format!("- {stamp}:\n")
    } else {
        String::new()
    };
    let mut lines = text.lines();
    if out.is_empty() {
        out = format!("- {stamp}: {}\n", lines.next().unwrap_or_default());
    }
    for line in lines {
        if line.trim().is_empty() {
            out.push('\n');
        } else {
            out += &format!("  {line}\n");
        }
    }
    out
}

fn without_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find("<!--") {
        out += &rest[..open];
        match rest[open..].find("-->") {
            Some(close) => rest = &rest[open + close + 3..],
            None => return out + &rest[open..],
        }
    }
    out + rest
}

fn parse_id_list(value: &str, problems: &mut Vec<String>) -> Vec<String> {
    let mut ids = vec![];
    for token in value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split([',', ' '])
    {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if token.bytes().all(|b| ID_ALPHABET.contains(&b)) {
            if token.len() != ID_LEN {
                problems.push(format!("blocker id wrong length: {token:?}"));
            }
            // Still an edge either way: a textual reference must keep
            // counting.
            ids.push(token.to_string());
        } else {
            problems.push(format!("malformed blocker id: {token:?}"));
        }
    }
    ids
}

/// The tracker `DEEDS_DIR` names, if set: an absolute path to the tracker
/// directory itself.
pub fn env_deeds_dir() -> Result<Option<PathBuf>> {
    let Some(dir) = std::env::var_os("DEEDS_DIR").filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let dir = PathBuf::from(dir);
    if !dir.is_absolute() {
        bail!("DEEDS_DIR must be an absolute path: {}", dir.display());
    }
    Ok(Some(dir))
}

/// The tracker for this process and, when `DEEDS_DIR` overrides it, the
/// `.deeds/` the walk up from cwd would have found.
pub fn locate() -> Result<(PathBuf, Option<PathBuf>)> {
    let cwd = std::env::current_dir()?;
    let walked = cwd
        .ancestors()
        .map(|dir| dir.join(".deeds"))
        .find(|candidate| candidate.is_dir());
    let Some(dir) = env_deeds_dir()? else {
        return match walked {
            Some(dir) => Ok((dir, None)),
            None => bail!("no .deeds directory here or above; run `deeds init`"),
        };
    };
    if !dir.is_dir() {
        bail!(
            "DEEDS_DIR is not a directory: {}; `deeds init` creates it",
            dir.display()
        );
    }
    let same = |other: &PathBuf| fs::canonicalize(other).ok() == fs::canonicalize(&dir).ok();
    let shadowed = walked.filter(|w| !same(w));
    Ok((dir, shadowed))
}

/// `locate`, saying on stderr when `DEEDS_DIR` shadows a `.deeds/`.
pub fn find_deeds_dir() -> Result<PathBuf> {
    let (dir, shadowed) = locate()?;
    if let Some(walked) = shadowed {
        eprintln!(
            "deeds: using DEEDS_DIR={}, not {}",
            dir.display(),
            walked.display()
        );
    }
    Ok(dir)
}

pub fn load_all(deeds_dir: &Path) -> Result<Vec<Issue>> {
    let mut issues = vec![];
    load_dir(deeds_dir, false, &mut issues)?;
    let closed = deeds_dir.join("closed");
    if closed.is_dir() {
        load_dir(&closed, true, &mut issues)?;
    }
    issues.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(issues)
}

fn load_dir(dir: &Path, closed: bool, issues: &mut Vec<Issue>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if !path.is_file() || path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let id = path.file_stem().unwrap().to_string_lossy().to_string();
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        issues.push(parse(&id, &path, &text, closed));
    }
    Ok(())
}

pub fn mint_id() -> String {
    (0..ID_LEN)
        .map(|_| ID_ALPHABET[fastrand::usize(..ID_ALPHABET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Issue {
        parse("ab2c", Path::new("ab2c.md"), text, false)
    }

    #[test]
    fn plain_markdown_is_an_issue() {
        let i = p("# Fix the thing\n\nbody\n");
        assert_eq!(i.title, "Fix the thing");
        assert!(i.blocked_by.is_empty());
        assert_eq!(i.tier, DEFAULT_TIER);
        assert!(i.problems.is_empty());
    }

    #[test]
    fn frontmatter_forms() {
        let i = p("---\nblocked-by: [x9f2mk, m3k2qw]\ntier: nit\n---\n# T\n");
        assert_eq!(i.blocked_by, ["x9f2mk", "m3k2qw"]);
        assert_eq!(i.tier, tier_rank("nit").unwrap());
        // Bare and underscore variants also accepted.
        assert_eq!(
            p("---\nblocked_by: x9f2mk\n---\n# T\n").blocked_by,
            ["x9f2mk"]
        );
    }

    #[test]
    fn advisory_keys_ignored() {
        let i = p("---\nwip: refactor-agent\n---\n# T\n");
        assert!(i.problems.is_empty());
    }

    #[test]
    fn for_you_section() {
        let i = p(
            "# T\n\nnotes\n\n## For you\n\n- [ ] Try it on a phone\n### Detail\nmore\n\n## Log\nlater\n",
        );
        assert_eq!(
            i.for_you.as_deref(),
            Some("- [ ] Try it on a phone\n### Detail\nmore")
        );
        // Last section, any case.
        assert_eq!(
            p("# T\n## for YOU\nPick one.\n").for_you.as_deref(),
            Some("Pick one.")
        );
        // Absent, empty, or only inside a code fence: nothing asked.
        assert_eq!(p("# T\nbody\n").for_you, None);
        assert_eq!(p("# T\n## For you\n\n## Next\nx\n").for_you, None);
        assert_eq!(p("# T\n```\n## For you\nx\n```\n").for_you, None);
        // A deeper heading of that name isn't the section.
        assert_eq!(p("# T\n### For you\nx\n").for_you, None);
        // Closing hashes; a comment alone asks nothing.
        assert_eq!(p("# T\n## For you ##\nx\n").for_you.as_deref(), Some("x"));
        assert_eq!(p("# T\n## For you\n<!-- none -->\n").for_you, None);
        assert_eq!(
            p("# T\n## For you\n<!-- note -->\nx\n").for_you.as_deref(),
            Some("<!-- note -->\nx")
        );
    }

    #[test]
    fn fences_hide_headings() {
        // A ~~~ block's heading is code, and the ask goes on past it.
        let i = p("# T\n## For you\nRun:\n~~~sh\n## a comment\n~~~\nthen tell me.\n");
        assert_eq!(
            i.for_you.as_deref(),
            Some("Run:\n~~~sh\n## a comment\n~~~\nthen tell me.")
        );
        // A ```` fence holds ``` lines, and its headings stay quoted.
        assert_eq!(
            p("# T\n````md\n```\n## For you\nx\n```\n````\n").for_you,
            None
        );
        // Indented four spaces, ``` is code, not a fence.
        assert_eq!(
            p("# T\n    ```\n## For you\nx\n").for_you.as_deref(),
            Some("x")
        );
    }

    #[test]
    fn log_entries_go_last() {
        let entry = "- d: x\n";
        // Created when missing, after a blank line.
        assert_eq!(
            with_log_entry("# T\nbody\n", entry),
            "# T\nbody\n\n## Log\n\n- d: x\n"
        );
        // Appended in place when last: the old text is a prefix.
        let last = "# T\n\n## Log\n\n- d: a\n";
        assert_eq!(with_log_entry(last, entry), format!("{last}- d: x\n"));
        // Moved to the end when a section follows it.
        assert_eq!(
            with_log_entry("# T\n## Log\n- d: a\n\n## For you\n- [ ] y\n", entry),
            "# T\n## For you\n- [ ] y\n\n## Log\n- d: a\n- d: x\n"
        );
        // A fenced `## Log` is code, not the section.
        assert_eq!(
            with_log_entry("# T\n```\n## Log\n```\n", entry),
            "# T\n```\n## Log\n```\n\n## Log\n\n- d: x\n"
        );
    }

    #[test]
    fn log_entries_append_after_blank_lines() {
        // Trailing blank lines don't force a rewrite.
        let last = "# T\n## Log\n- d: a\n\n";
        assert_eq!(with_log_entry(last, "- d: x\n"), format!("{last}- d: x\n"));
        assert_eq!(
            with_log_entry("# T", "- d: x\n"),
            "# T\n\n## Log\n\n- d: x\n"
        );
    }

    #[test]
    fn fenced_note_keeps_later_headings() {
        let entry = log_entry("d", None, "```sh\necho\n```");
        assert_eq!(entry, "- d:\n  ```sh\n  echo\n  ```\n");
        let text = with_log_entry("# T\n", &entry) + "\n## For you\n\n- [ ] y\n";
        assert_eq!(p(&text).for_you.as_deref(), Some("- [ ] y"));
        // Indented fences and one-line code spans stay balanced too.
        for note in ["  ```\n  x\n  ```", "```code``` span\nmore"] {
            let entry = log_entry("d", None, note);
            let text = with_log_entry("# T\n", &entry) + "\n## For you\n\ny\n";
            assert_eq!(p(&text).for_you.as_deref(), Some("y"), "{note:?}");
        }
    }

    #[test]
    fn last_logged_is_the_latest_entry_date() {
        let text = "# t\n\n## Log\n\n- 2026-03-02 app@x: b\n  - 2027-01-01 nested\n- 2026-01-05: a\n- not a date\n";
        let issue = parse("aaaaaa", Path::new("x"), text, false);
        assert_eq!(issue.last_logged.as_deref(), Some("2026-03-02"));
        let issue = parse(
            "aaaaaa",
            Path::new("x"),
            "# t\n\n- 2026-03-02 outside\n",
            false,
        );
        assert_eq!(issue.last_logged, None);
    }

    #[test]
    fn log_entry_format() {
        assert_eq!(
            log_entry("2026-11-01", Some("app@ws"), "x\n"),
            "- 2026-11-01 app@ws: x\n"
        );
        assert_eq!(log_entry("2026-11-01", None, "x"), "- 2026-11-01: x\n");
        assert_eq!(
            log_entry("d", None, "\nfirst\n\n  code\nlast\n\n"),
            "- d: first\n\n    code\n  last\n"
        );
    }

    #[test]
    fn lints_recorded() {
        let i = p("---\ntier: p1\nblocked-by: [BAD!]\n# no terminator\n");
        assert!(i.problems.iter().any(|p| p.contains("tier")));
        assert!(i.problems.iter().any(|p| p.contains("malformed blocker")));
        assert!(
            i.problems
                .iter()
                .any(|p| p == "line 1: frontmatter opened here is never closed")
        );
        assert!(i.problems.iter().any(|p| p.starts_with("line 2: tier")));
        assert!(i.problems.iter().any(|p| p.contains("no '# Title'")));
    }
}
