use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

const AGENTS_BLOCK: &str = include_str!("agents_block.md");
const MARKER_BEGIN: &str = "<!-- deeds:begin -->";
const MARKER_END: &str = "<!-- deeds:end -->";

/// `deeds init`: create or refresh the tracker at cwd — `.deeds/` and the
/// AGENTS.md pointer block. Idempotent; rerun after upgrading deeds.
/// Refuses under an existing tracker.
pub fn init(root: &Path) -> Result<()> {
    for dir in root.ancestors().skip(1) {
        if dir.join(".deeds").is_dir() {
            bail!(
                "tracker already exists above: {}",
                dir.join(".deeds").display()
            );
        }
    }
    if !root.join(".deeds").is_dir() {
        fs::create_dir_all(root.join(".deeds/closed"))?;
        println!("initialized {}", root.join(".deeds").display());
    }
    upsert_agents_block(&root.join("AGENTS.md"))?;
    remove_legacy_skill(root)
}

/// Earlier versions installed the guide as a per-repo agent skill; `deeds
/// skill` replaced it. Remove what init used to write.
fn remove_legacy_skill(root: &Path) -> Result<()> {
    let skill = root.join(".agents/skills/deeds/SKILL.md");
    if skill.exists() {
        fs::remove_file(&skill).with_context(|| format!("remove {}", skill.display()))?;
        println!("removed {}", skill.display());
    }
    let link = root.join(".claude/skills/deeds");
    if fs::read_link(&link).is_ok_and(|dest| dest == Path::new("../../.agents/skills/deeds")) {
        fs::remove_file(&link).with_context(|| format!("remove {}", link.display()))?;
        println!("removed {}", link.display());
    }
    // remove_dir fails on non-empty dirs, which leaves anyone else's files be.
    for dir in [
        ".agents/skills/deeds",
        ".agents/skills",
        ".agents",
        ".claude/skills",
    ] {
        let _ = fs::remove_dir(root.join(dir));
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

    #[test]
    fn init_creates_tracker_and_removes_legacy_skill() {
        let dir = std::env::temp_dir().join(format!(
            "deeds-test-init-{}-{}",
            std::process::id(),
            fastrand::u64(..)
        ));
        fs::create_dir_all(dir.join(".agents/skills/deeds")).unwrap();
        fs::create_dir_all(dir.join(".claude/skills")).unwrap();
        fs::write(dir.join(".agents/skills/deeds/SKILL.md"), "stale\n").unwrap();
        std::os::unix::fs::symlink(
            "../../.agents/skills/deeds",
            dir.join(".claude/skills/deeds"),
        )
        .unwrap();
        fs::write(dir.join(".claude/settings.json"), "{}\n").unwrap();
        fs::write(
            dir.join("AGENTS.md"),
            format!("{MARKER_BEGIN}\nstale\n{MARKER_END}\n"),
        )
        .unwrap();

        init(&dir).unwrap();
        assert!(dir.join(".deeds/closed").is_dir());
        assert_eq!(
            fs::read_to_string(dir.join("AGENTS.md")).unwrap(),
            AGENTS_BLOCK
        );
        assert!(!dir.join(".agents").exists());
        assert!(fs::symlink_metadata(dir.join(".claude/skills")).is_err());
        assert!(dir.join(".claude/settings.json").exists());

        fs::remove_dir_all(&dir).unwrap();
    }
}
