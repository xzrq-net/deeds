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
// 6 chars: 30^6 = 729M keeps lifetime reuse-after-gc negligible (expected
// collisions ~0.1 across 10^4 lifetime issues).
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
        problems: vec![],
    };

    let mut lines = text.lines().peekable();
    if lines.peek().map(|l| l.trim()) == Some("---") {
        lines.next();
        let mut terminated = false;
        for line in lines.by_ref() {
            if line.trim() == "---" {
                terminated = true;
                break;
            }
            let Some((key, value)) = line.split_once(':') else {
                if !line.trim().is_empty() {
                    issue
                        .problems
                        .push(format!("frontmatter line without ':': {line:?}"));
                }
                continue;
            };
            match key.trim() {
                // Repeated keys extend rather than replace: a merge that
                // leaves two blocked-by lines must not drop edges.
                "blocked-by" | "blocked_by" => {
                    let ids = parse_id_list(value, &mut issue.problems);
                    issue.blocked_by.extend(ids);
                }
                "tier" => match tier_rank(value.trim()) {
                    Some(r) => issue.tier = r,
                    None => issue.problems.push(format!(
                        "tier not one of {}: {:?}",
                        TIERS.join("/"),
                        value.trim()
                    )),
                },
                // Unknown keys are advisory (wip: etc.) and carry no semantics.
                _ => {}
            }
        }
        if !terminated {
            issue.problems.push("unterminated frontmatter".into());
        }
    }

    match lines.find_map(|l| l.strip_prefix("# ")) {
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
        if let Some(c) = ['`', '~'].into_iter().find(|&c| near && run(c) >= 3) {
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

/// Whether the body's last section is `## <name>`, empty or not: text
/// appended to the file would land in it.
pub fn ends_in(text: &str, name: &str) -> bool {
    headings(text).last().is_some_and(|h| is_section(h, name))
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
            // counting (gc's guard relies on it).
            ids.push(token.to_string());
        } else {
            problems.push(format!("malformed blocker id: {token:?}"));
        }
    }
    ids
}

/// Locate the tracker by walking up from cwd.
pub fn find_deeds_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    for dir in cwd.ancestors() {
        let candidate = dir.join(".deeds");
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }
    bail!("no .deeds directory here or above; run `deeds init`");
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
    fn ends_in_a_section() {
        assert!(ends_in("# T\n## For you\n\n", FOR_YOU));
        assert!(!ends_in("# T\n## For you\nx\n## Log\n", FOR_YOU));
        assert!(!ends_in("# T\n", FOR_YOU));
    }

    #[test]
    fn lints_recorded() {
        let i = p("---\ntier: p1\nblocked-by: [BAD!]\n# no terminator\n");
        assert!(i.problems.iter().any(|p| p.contains("tier")));
        assert!(i.problems.iter().any(|p| p.contains("malformed blocker")));
        assert!(i.problems.iter().any(|p| p.contains("unterminated")));
        assert!(i.problems.iter().any(|p| p.contains("no '# Title'")));
    }
}
