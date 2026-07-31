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
    use rogrep_index::query::{DATE_FACET_KEYS, KNOWN_FACET_KEYS};

    const QUERY_SYNTAX_MD: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/query-syntax.md"));

    /// Backticked `key:` spans that are deliberately NOT facet keys:
    /// grammar-explanation examples, agentpm-only keys named in the
    /// deviations section, and output-format labels.
    const NON_FACET_EXAMPLES: &[&str] =
        &["key", "data", "http", "https", "owner", "user", "agent_id", "tag", "inspect"];

    #[test]
    fn skill_frontmatter_is_valid() {
        assert!(SKILL_MD.starts_with("---\nname: rogrep\n"));
        assert!(SKILL_MD.contains("description:"));
    }

    /// Every facet key mentioned in a doc as a backticked `key:` span.
    /// The backtick-delimited form is the enforcement contract: document a
    /// facet as `` `key:` `` (or `` `key:value` ``) and this extracts it.
    fn documented_facet_keys(doc: &str) -> Vec<String> {
        // Drop fenced code blocks — their content is example output, not
        // facet documentation.
        let unfenced: String = doc
            .split("```")
            .step_by(2)
            .collect::<Vec<_>>()
            .join(" ");
        let mut out = Vec::new();
        for (i, span) in unfenced.split('`').enumerate() {
            if i % 2 == 0 {
                continue;
            }
            let Some((key, _)) = span.split_once(':') else { continue };
            if key.is_empty()
                || !key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
            {
                continue;
            }
            let key = key.replace('-', "_");
            if !NON_FACET_EXAMPLES.contains(&key.as_str()) {
                out.push(key);
            }
        }
        out.sort();
        out.dedup();
        out
    }

    #[test]
    fn facet_docs_stay_in_sync() {
        for (name, doc) in [("SKILL.md", SKILL_MD), ("docs/query-syntax.md", QUERY_SYNTAX_MD)] {
            // Guard against the extractor silently matching nothing.
            assert!(
                documented_facet_keys(doc).len() >= 20,
                "{name}: facet-key extraction looks broken"
            );
            // Forward: every key the grammar knows is documented.
            for key in KNOWN_FACET_KEYS.iter().chain(DATE_FACET_KEYS) {
                assert!(
                    doc.contains(&format!("`{key}:")),
                    "{name} does not document facet key `{key}:`"
                );
            }
            // Reverse: every documented key exists in the grammar.
            for key in documented_facet_keys(doc) {
                assert!(
                    KNOWN_FACET_KEYS.contains(&key.as_str())
                        || DATE_FACET_KEYS.contains(&key.as_str()),
                    "{name} documents `{key}:` but the grammar does not know it \
                     (add it to KNOWN_FACET_KEYS or NON_FACET_EXAMPLES)"
                );
            }
        }
    }
}
