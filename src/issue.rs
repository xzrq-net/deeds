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

/// A `## <name>` section's text, up to the next `#` or `##` heading;
/// headings inside code fences don't count.
fn section(text: &str, name: &str) -> Option<String> {
    let mut fenced = false;
    let mut body: Option<Vec<&str>> = None;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        let heading = !fenced && (line.starts_with("# ") || line.starts_with("## "));
        if heading && body.is_some() {
            break;
        }
        match &mut body {
            Some(b) => b.push(line),
            None if heading => {
                let title = line.trim_start_matches('#').trim();
                if line.starts_with("## ") && title.eq_ignore_ascii_case(name) {
                    body = Some(vec![]);
                }
            }
            None => {}
        }
    }
    let body = body?.join("\n");
    let body = body.trim();
    (!body.is_empty()).then(|| body.to_string())
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
