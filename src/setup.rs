use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

const SKILL: &str = include_str!("skill.md");
const AGENTS_BLOCK: &str = include_str!("agents_block.md");
const MARKER_BEGIN: &str = "<!-- deeds:begin -->";
const MARKER_END: &str = "<!-- deeds:end -->";

/// `deeds init`: create the tracker at cwd. Refuses under an existing one.
pub fn init(root: &Path) -> Result<()> {
    for dir in root.ancestors().skip(1) {
        if dir.join(".deeds").is_dir() {
            bail!(
                "tracker already exists above: {}",
                dir.join(".deeds").display()
            );
        }
    }
    fs::create_dir_all(root.join(".deeds/closed"))?;
    println!("initialized {}", root.join(".deeds").display());
    Ok(())
}

/// `deeds setup`: init + AGENTS.md pointer block + agent skill + Claude symlink.
pub fn setup(root: &Path) -> Result<()> {
    if !root.join(".deeds").is_dir() {
        init(root)?;
    }
    upsert_agents_block(&root.join("AGENTS.md"))?;

    let skill_dir = root.join(".agents/skills/deeds");
    fs::create_dir_all(&skill_dir)?;
    fs::write(skill_dir.join("SKILL.md"), SKILL)?;
    println!("wrote {}", skill_dir.join("SKILL.md").display());

    let claude_skills = root.join(".claude/skills");
    fs::create_dir_all(&claude_skills)?;
    let link = claude_skills.join("deeds");
    let target = Path::new("../../.agents/skills/deeds");
    match fs::symlink_metadata(&link) {
        Ok(meta) if meta.is_symlink() => {
            let dest = fs::read_link(&link).unwrap_or_default();
            if dest != target {
                eprintln!(
                    "warning: {} points elsewhere ({}); left alone",
                    link.display(),
                    dest.display()
                );
            }
        }
        Ok(_) => eprintln!(
            "warning: {} exists and is not a symlink; left alone",
            link.display()
        ),
        Err(_) => {
            std::os::unix::fs::symlink(target, &link)
                .with_context(|| format!("symlink {}", link.display()))?;
            println!("linked {}", link.display());
        }
    }
    Ok(())
}

fn upsert_agents_block(path: &Path) -> Result<()> {
    let current = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    if current.matches(MARKER_BEGIN).count() > 1 || current.matches(MARKER_END).count() > 1 {
        bail!("multiple deeds marker blocks in {}", path.display());
    }
    let updated = match (current.find(MARKER_BEGIN), current.find(MARKER_END)) {
        (Some(begin), Some(end)) if begin < end => {
            let after = end + MARKER_END.len();
            format!(
                "{}{}{}",
                &current[..begin],
                AGENTS_BLOCK.trim_end(),
                &current[after..]
            )
        }
        (None, None) if current.is_empty() => AGENTS_BLOCK.to_string(),
        (None, None) => format!("{}\n{}", current.trim_end(), AGENTS_BLOCK),
        _ => bail!("unpaired deeds markers in {}", path.display()),
    };
    if updated != current {
        fs::write(path, updated).with_context(|| format!("write {}", path.display()))?;
        println!("updated {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_block_upsert_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("deeds-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("AGENTS.md");

        upsert_agents_block(&path).unwrap();
        let fresh = fs::read_to_string(&path).unwrap();
        assert!(fresh.contains(MARKER_BEGIN));

        // Existing prose is preserved, block replaced in place, no growth.
        fs::write(&path, format!("# My repo\n\nprose\n\n{fresh}\ntrailer\n")).unwrap();
        upsert_agents_block(&path).unwrap();
        upsert_agents_block(&path).unwrap();
        let done = fs::read_to_string(&path).unwrap();
        assert_eq!(done.matches(MARKER_BEGIN).count(), 1);
        assert!(done.contains("prose") && done.contains("trailer"));

        fs::remove_dir_all(&dir).unwrap();
    }
}
