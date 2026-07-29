//! Bundled agent skill: embedded in the binary, installed with an atomic
//! temp-dir + rename (mirroring agentpm's proven mechanism).

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use std::path::PathBuf;

pub const SKILL_NAME: &str = "rogrep";
pub const SKILL_MD: &str = include_str!("../../assets/SKILL.md");

#[derive(Args)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub action: SkillAction,
}

#[derive(Subcommand)]
pub enum SkillAction {
    /// Install the skill for local agents (~/.agents/skills, ~/.claude/skills).
    Install,
    /// Remove installed copies.
    Uninstall,
    /// Print the skill to stdout.
    Show,
}

/// Install targets: the cross-agent convention always; per-agent dirs only
/// when that agent is present.
fn install_dirs() -> Vec<PathBuf> {
    let home = rogrep_model::paths::home_dir();
    let mut dirs = vec![home.join(".agents/skills").join(SKILL_NAME)];
    for agent_dir in [".claude", ".codex", ".grok"] {
        if home.join(agent_dir).is_dir() {
            dirs.push(home.join(agent_dir).join("skills").join(SKILL_NAME));
        }
    }
    dirs
}

fn install_to(dir: &PathBuf) -> Result<()> {
    let parent = dir.parent().context("skill dir has a parent")?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".{SKILL_NAME}.tmp.{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    std::fs::write(tmp.join("SKILL.md"), SKILL_MD)?;
    let _ = std::fs::remove_dir_all(dir);
    std::fs::rename(&tmp, dir).with_context(|| format!("installing skill to {}", dir.display()))?;
    Ok(())
}

pub fn run(args: SkillArgs) -> Result<()> {
    match args.action {
        SkillAction::Install => {
            for dir in install_dirs() {
                install_to(&dir)?;
                println!("installed {}", dir.display());
            }
            Ok(())
        }
        SkillAction::Uninstall => {
            for dir in install_dirs() {
                if dir.is_dir() {
                    std::fs::remove_dir_all(&dir)?;
                    println!("removed {}", dir.display());
                }
            }
            Ok(())
        }
        SkillAction::Show => {
            print!("{SKILL_MD}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_frontmatter_is_valid() {
        assert!(SKILL_MD.starts_with("---\nname: rogrep\n"));
        assert!(SKILL_MD.contains("description:"));
        // The facet list in the skill must only name keys the grammar knows.
        for key in ["tool_cmd", "tool_status", "git_pr_num", "git_branch", "provider"] {
            assert!(
                rogrep_index::query::KNOWN_FACET_KEYS.contains(&key),
                "skill documents facet {key} that the grammar dropped"
            );
            assert!(SKILL_MD.contains(&format!("{key}:")), "skill omits facet {key}");
        }
    }
}
