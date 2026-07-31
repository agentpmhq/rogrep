//! Per-turn facet token assembly.

use crate::git;
use crate::shell::{base_name, split_segments};
use rogrep_model::{SpecialTurn, ToolDirection, Turn};

/// Tool names that run shell commands (facet extraction digs into their
/// command input).
fn is_shell_tool(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "bash" | "shell" | "exec_command" | "run_terminal_cmd" | "terminal" | "execute" | "run"
    )
}

fn command_from_input(turn: &Turn) -> Option<String> {
    let info = turn.tool.as_ref()?;
    for key in ["command", "cmd"] {
        if let Some(v) = info.input_fields.get(key) {
            if let Some(s) = v.as_str() {
                if !s.trim().is_empty() {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

/// The shell command of a shell-tool call turn, if any.
pub fn command_from_shell_turn(turn: &Turn) -> Option<String> {
    let info = turn.tool.as_ref()?;
    if !is_shell_tool(&info.name) {
        return None;
    }
    command_from_input(turn)
}

/// Slug a facet value into the indexed vocabulary: lowercase, `/` and `.`
/// preserved (branches, versions), every other punctuation run collapsed
/// to a single `-`. Applied on the query side too, so `Feature_X` and
/// `feature-x` meet at the same token.
pub fn facet_slug(value: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in value.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '/' {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    out
}

/// git_cmd tokens that imply a read-only segment (used when a segment has
/// git facets but no classifier subtype).
fn git_tokens_read_only(tokens: &[String]) -> bool {
    tokens.iter().any(|t| {
        matches!(
            t.as_str(),
            "git_cmd:status" | "git_cmd:diff" | "git_cmd:log" | "git_cmd:show" | "git_cmd:remote"
                | "git_cmd:rev-parse" | "git_cmd:ls-remote" | "git_cmd:merge-base"
        )
    })
}

/// Facet tokens for the command line of a shell tool call.
pub fn shell_command_facets(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    for segment in split_segments(command) {
        // Segments after a pipe are post-filters (`| head`), not commands.
        if segment.operator_before == "|" {
            continue;
        }
        let effective = crate::shell::expand_wrapper(&segment.text).unwrap_or(segment.text.clone());
        let f = crate::shell::fields(&effective);
        let Some(exe) = f.first() else { continue };
        let exe = base_name(exe);
        if exe.is_empty() || exe == "cd" || exe == "export" {
            continue;
        }
        // sudo/env wrappers: facet the wrapped executable too.
        let (exe, rest) = if matches!(exe.as_str(), "sudo" | "env" | "time" | "nice") {
            match f.iter().skip(1).find(|a| !a.contains('=') && !a.starts_with('-')) {
                Some(inner) => (base_name(inner), effective.clone()),
                None => (exe, effective.clone()),
            }
        } else {
            (exe, effective.clone())
        };
        out.push(format!("tool_cmd:{}", facet_slug(&exe)));
        let git_tokens = git::git_facets_for_segment(&rest);
        let cls = crate::classify::classify_shell_command(&effective);
        // agentpm parity: unclassified segments with no git evidence get
        // only their tool_cmd token.
        if cls.subtype.is_none() && git_tokens.is_empty() {
            continue;
        }
        if let Some(subtype) = cls.subtype {
            out.push(format!("tool_type:{subtype}"));
        }
        let read_only = if cls.subtype.is_some() {
            cls.read_only
        } else {
            git_tokens_read_only(&git_tokens)
        };
        out.push(format!("tool_location:{}", if cls.remote { "remote" } else { "local" }));
        out.push(format!("tool_mutability:{}", if read_only { "read-only" } else { "mutating" }));
        if cls.privileged {
            out.push("tool_privilege:privileged".to_string());
        }
        out.extend(git_tokens);
    }
    out.sort();
    out.dedup();
    out
}

/// agentpm's normalizeTool slugs: canonical `tool_type:` values for
/// non-shell tools; unknown names fall back to `facet_slug`.
fn normalized_tool_slug(name: &str) -> String {
    match name.to_lowercase().as_str() {
        "" | "tool_result" | "tool" => "tool".into(),
        "bash" => "bash".into(),
        "exec" | "exec_command" | "shell" | "terminal" | "command" => "exec".into(),
        "list_dir" | "listdir" => "list-dir".into(),
        "read_file" | "readfile" | "read" => "read-file".into(),
        "write_file" | "writefile" | "write" => "write-file".into(),
        "edit_file" | "editfile" | "edit" => "edit-file".into(),
        "multi_edit" | "multiedit" => "multi-edit".into(),
        "notebook_edit" | "notebookedit" => "notebook-edit".into(),
        "grep" => "grep".into(),
        "apply_patch" => "apply-patch".into(),
        "write_stdin" => "write-stdin".into(),
        "request_user_input" => "request-user-input".into(),
        "update_plan" => "update-plan".into(),
        "spawn_agent" => "spawn-agent".into(),
        "send_input" => "send-input".into(),
        "wait_agent" => "wait-agent".into(),
        "close_agent" => "close-agent".into(),
        "resume_agent" => "resume-agent".into(),
        other => facet_slug(other),
    }
}

/// Read-only decision for a non-shell tool: a known read-only tool, or one
/// whose file accesses are all read-mode.
fn non_shell_read_only(slug: &str, refs: &[(String, &'static str)]) -> bool {
    if matches!(
        slug,
        "read-file" | "list-dir" | "grep" | "request-user-input" | "update-plan" | "spawn-agent"
            | "wait-agent"
    ) {
        return true;
    }
    !refs.is_empty() && refs.iter().all(|(_, mode)| *mode == "read")
}

/// All facet tokens for one turn.
pub fn facet_tokens_for_turn(turn: &Turn) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(info) = &turn.tool {
        match info.direction {
            Some(ToolDirection::Use) => {
                let name = info.name.to_lowercase();
                out.push(format!("tool:{name}"));
                // mcp__server__tool → mcp:server + tool facet on the leaf.
                if let Some(rest) = name.strip_prefix("mcp__") {
                    if let Some((server, _tool)) = rest.split_once("__") {
                        out.push(format!("mcp:{server}"));
                    }
                }
                if name == "skill" {
                    if let Some(skill) = info.input_fields.get("skill").and_then(|v| v.as_str()) {
                        out.push(format!("skill:{}", skill.to_lowercase()));
                    }
                }
                if is_shell_tool(&info.name) {
                    if let Some(command) = command_from_input(turn) {
                        let cmd_facets = shell_command_facets(&command);
                        let mutating = cmd_facets
                            .iter()
                            .filter_map(|f| git::is_mutating_facet(f))
                            .any(|m| m);
                        out.extend(cmd_facets);
                        if mutating {
                            out.push("tool_mutating:true".to_string());
                        }
                    }
                } else {
                    let slug = normalized_tool_slug(&info.name);
                    let read_only = non_shell_read_only(&slug, &file_refs_for_turn(turn));
                    out.push(format!("tool_type:{slug}"));
                    out.push("tool_location:local".to_string());
                    out.push(format!(
                        "tool_mutability:{}",
                        if read_only { "read-only" } else { "mutating" }
                    ));
                }
                out.push(format!("tool_status:{}", info.status.as_str()));
            }
            Some(ToolDirection::Output) => {
                out.push(format!("tool_status:{}", info.status.as_str()));
            }
            None => {}
        }
    }
    if turn.provider_meta.get("content_image").and_then(|v| v.as_bool()) == Some(true) {
        out.push("content:image".to_string());
    }
    match &turn.special {
        Some(SpecialTurn::TurnAborted { .. }) => out.push("is:interrupted".to_string()),
        Some(SpecialTurn::CompactBoundary) => out.push("is:compacted".to_string()),
        Some(SpecialTurn::TaskNotification { .. }) => out.push("is:notification".to_string()),
        Some(SpecialTurn::ScheduledPrompt { .. }) => out.push("is:scheduled".to_string()),
        _ => {}
    }
    out.sort();
    out.dedup();
    out
}

/// Output-turn facets that need the paired command's facets (PR-create
/// number, commit confirmations).
pub fn output_facet_tokens(output_text: &str, command_facets: &[String]) -> Vec<String> {
    git::mine_output(output_text, command_facets)
}

/// File paths touched by a tool call (for the `file:` facet), from
/// well-known input fields. Relative paths resolve against the turn cwd.
pub fn file_refs_for_turn(turn: &Turn) -> Vec<(String, &'static str)> {
    let Some(info) = &turn.tool else { return vec![] };
    if info.direction != Some(ToolDirection::Use) {
        return vec![];
    }
    let mode = match info.name.to_lowercase().as_str() {
        "edit" | "write" | "notebookedit" | "apply_patch" | "create_file" | "str_replace_editor" => "mutate",
        _ => "read",
    };
    let mut out = Vec::new();
    for key in ["file_path", "path", "notebook_path"] {
        if let Some(p) = info.input_fields.get(key).and_then(|v| v.as_str()) {
            let p = p.trim();
            if p.is_empty() {
                continue;
            }
            let abs = if p.starts_with('/') {
                p.to_string()
            } else if let Some(cwd) = &turn.cwd {
                format!("{}/{}", cwd.trim_end_matches('/'), p)
            } else {
                continue;
            };
            out.push((abs, mode));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rogrep_model::{Role, ToolInfo, ToolStatus};
    use std::collections::BTreeMap;

    fn bash_turn(command: &str) -> Turn {
        let mut input_fields = BTreeMap::new();
        input_fields.insert("command".to_string(), serde_json::json!(command));
        Turn {
            role: Role::Tool,
            speaker: "Bash".into(),
            text: command.into(),
            tool: Some(ToolInfo {
                direction: Some(ToolDirection::Use),
                name: "Bash".into(),
                status: ToolStatus::Unknown,
                input_fields,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn bash_git_commit_facets() {
        let f = facet_tokens_for_turn(&bash_turn("git add -A && git commit -m 'x' && git push origin main"));
        assert!(f.contains(&"tool:bash".to_string()));
        assert!(f.contains(&"tool_cmd:git".to_string()));
        assert!(f.contains(&"git_cmd:commit".to_string()));
        assert!(f.contains(&"git_cmd:push".to_string()));
        assert!(f.contains(&"git_branch:main".to_string()));
        assert!(f.contains(&"tool_mutating:true".to_string()));
    }

    #[test]
    fn pipe_tail_produces_no_tool_cmd() {
        let f = facet_tokens_for_turn(&bash_turn("git log --oneline | head -5"));
        assert!(f.contains(&"tool_cmd:git".to_string()));
        assert!(!f.contains(&"tool_cmd:head".to_string()));
    }

    #[test]
    fn mcp_tool_facets() {
        let mut turn = bash_turn("x");
        turn.tool.as_mut().unwrap().name = "mcp__posthog__query".into();
        let f = facet_tokens_for_turn(&turn);
        assert!(f.contains(&"tool:mcp__posthog__query".to_string()));
        assert!(f.contains(&"mcp:posthog".to_string()));
    }

    #[test]
    fn file_refs_modes() {
        let mut input_fields = BTreeMap::new();
        input_fields.insert("file_path".to_string(), serde_json::json!("src/lib.rs"));
        let turn = Turn {
            role: Role::Tool,
            cwd: Some("/home/u/proj".into()),
            tool: Some(ToolInfo {
                direction: Some(ToolDirection::Use),
                name: "Edit".into(),
                input_fields,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            file_refs_for_turn(&turn),
            vec![("/home/u/proj/src/lib.rs".to_string(), "mutate")]
        );
    }
}
