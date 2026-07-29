//! Per-conversation git timeline: one op per tool call that produced git
//! evidence, with output pairing (PR-create numbers, commit confirmations).

use crate::facets::{command_from_shell_turn, shell_command_facets};
use crate::git;
use rogrep_model::{Conversation, ToolDirection, ToolStatus, Turn, UnixMillis};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize)]
pub struct GitOp {
    pub turn_index: u32,
    pub ts: Option<UnixMillis>,
    /// The command text (trimmed).
    pub command: String,
    /// All git_* facets, including output-mined ones.
    pub facets: Vec<String>,
    pub mutating: bool,
    pub status: ToolStatus,
    /// Head of the paired tool output.
    pub output_head: String,
    pub pr_numbers: Vec<u64>,
    /// Set when this op CREATED the PR (number mined from output).
    pub created_pr: Option<u64>,
    pub branches: Vec<String>,
    pub commits: Vec<String>,
}

pub fn git_ops_for_conversation(conv: &Conversation) -> Vec<GitOp> {
    // Pair outputs to calls.
    let mut output_by_pair: HashMap<&str, &Turn> = HashMap::new();
    for t in &conv.turns {
        if let Some(info) = &t.tool {
            if info.direction == Some(ToolDirection::Output) {
                if let Some(pair) = &info.pair_id {
                    output_by_pair.entry(pair.as_str()).or_insert(t);
                }
            }
        }
    }

    let mut ops = Vec::new();
    for t in &conv.turns {
        let Some(info) = &t.tool else { continue };
        if info.direction != Some(ToolDirection::Use) {
            continue;
        }
        let Some(command) = command_from_shell_turn(t) else { continue };
        let mut facets: Vec<String> = shell_command_facets(&command)
            .into_iter()
            .filter(|f| f.starts_with("git_"))
            .collect();
        if facets.is_empty() {
            continue;
        }
        let output = info
            .pair_id
            .as_deref()
            .and_then(|p| output_by_pair.get(p))
            .copied();
        if let Some(out) = output {
            facets.extend(git::mine_output(&out.text, &facets));
        }
        facets.sort();
        facets.dedup();

        let created_pr = facets.iter().find_map(|f| {
            f.strip_prefix("git_pr:create-num:").and_then(|n| n.parse().ok())
        });
        let mut pr_numbers: Vec<u64> = facets
            .iter()
            .filter_map(|f| f.strip_prefix("git_pr_num:").and_then(|n| n.parse().ok()))
            .collect();
        if let Some(n) = created_pr {
            pr_numbers.push(n);
        }
        pr_numbers.sort_unstable();
        pr_numbers.dedup();

        let mutating = facets.iter().filter_map(|f| git::is_mutating_facet(f)).any(|m| m);
        let status = output.map(|o| o.tool.as_ref().map(|i| i.status).unwrap_or_default()).unwrap_or(info.status);

        ops.push(GitOp {
            turn_index: t.turn_index,
            ts: t.ts,
            command: command.trim().chars().take(200).collect(),
            branches: facets
                .iter()
                .filter_map(|f| f.strip_prefix("git_branch:").map(|s| s.to_string()))
                .collect(),
            commits: facets
                .iter()
                .filter_map(|f| f.strip_prefix("git_commit:").map(|s| s.to_string()))
                .collect(),
            facets,
            mutating,
            status,
            output_head: output
                .map(|o| o.text.trim().chars().take(240).collect::<String>().replace('\n', " "))
                .unwrap_or_default(),
            pr_numbers,
            created_pr,
        });
    }
    ops
}

impl GitOp {
    /// Strict touch test (agentpm opTouchesPR): only structured evidence
    /// counts — never bare substring matching, which false-matches token
    /// counts and offsets.
    pub fn touches_pr(&self, n: u64) -> bool {
        self.pr_numbers.contains(&n)
            || self.output_head.contains(&format!("/pull/{n}"))
            || self.command.contains(&format!("/pull/{n}"))
            || self.command.contains(&format!("#{n} "))
    }

    pub fn touches_branch(&self, branch: &str) -> bool {
        self.branches.iter().any(|b| b == branch)
    }

    pub fn touches_commit(&self, sha7: &str) -> bool {
        self.commits.iter().any(|c| c == sha7)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rogrep_model::{Role, ToolInfo};
    use std::collections::BTreeMap;

    fn shell_pair(idx: u32, command: &str, output: &str) -> Vec<Turn> {
        let mut input_fields = BTreeMap::new();
        input_fields.insert("command".to_string(), serde_json::json!(command));
        vec![
            Turn {
                turn_index: idx,
                role: Role::Tool,
                speaker: "Bash".into(),
                text: command.into(),
                tool: Some(ToolInfo {
                    direction: Some(ToolDirection::Use),
                    name: "Bash".into(),
                    pair_id: Some(format!("p{idx}")),
                    input_fields,
                    ..Default::default()
                }),
                ..Default::default()
            },
            Turn {
                turn_index: idx + 1,
                role: Role::Tool,
                speaker: "tool_result".into(),
                text: output.into(),
                tool: Some(ToolInfo {
                    direction: Some(ToolDirection::Output),
                    name: "tool_result".into(),
                    pair_id: Some(format!("p{idx}")),
                    status: ToolStatus::Succeeded,
                    ..Default::default()
                }),
                ..Default::default()
            },
        ]
    }

    fn conv(turns: Vec<Turn>) -> Conversation {
        Conversation {
            id: rogrep_model::ConversationId::from_source_path("/x.jsonl"),
            agent: rogrep_model::AgentKind::Claude,
            source_path: "/x.jsonl".into(),
            title: None,
            model: None,
            project: String::new(),
            normalized_project: "home".into(),
            cwd: None,
            first_seen: None,
            last_seen: None,
            tokens: Default::default(),
            malformed_lines: 0,
            origin: Default::default(),
            subagent: None,
            turns,
        }
    }

    #[test]
    fn pr_create_origin_from_output() {
        let mut turns = shell_pair(0, "gh pr create --fill", "https://github.com/o/r/pull/62");
        turns.extend(shell_pair(2, "git status", "clean"));
        let ops = git_ops_for_conversation(&conv(turns));
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].created_pr, Some(62));
        assert!(ops[0].touches_pr(62));
        assert!(ops[0].mutating);
        assert!(!ops[1].mutating, "git status is read-only");
    }

    #[test]
    fn commit_confirmation_pairs() {
        let turns = shell_pair(0, "git commit -m 'fix'", "[main abc1234] fix\n 1 file changed");
        let ops = git_ops_for_conversation(&conv(turns));
        assert_eq!(ops[0].commits, vec!["abc1234"]);
        assert!(ops[0].touches_commit("abc1234"));
    }

    #[test]
    fn no_git_no_ops() {
        let turns = shell_pair(0, "cargo test", "ok");
        assert!(git_ops_for_conversation(&conv(turns)).is_empty());
    }
}
