use anyhow::Result;
use clap::Args;
use rogrep_tooltree::git_ops_for_conversation;

#[derive(Args)]
pub struct GitArgs {
    /// Conversation id (rg_… or unique prefix).
    pub id: String,
    /// Only ops matching this PR number.
    #[arg(long)]
    pub pr: Option<u64>,
    /// Only ops touching this branch.
    #[arg(long)]
    pub branch: Option<String>,
    /// Only ops touching this commit (sha or 7-char prefix).
    #[arg(long, visible_alias = "sha")]
    pub commit: Option<String>,
    /// Only mutating ops (commits, pushes, merges, PR create/edit…).
    #[arg(long)]
    pub mutating_only: bool,
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: GitArgs) -> Result<()> {
    let (_layout, _config, store) = super::sync::sync_now(false, true)?;
    let (row, conv) = super::show::load_conversation(&store, &args.id)?;
    let mut ops = git_ops_for_conversation(&conv);

    if args.mutating_only {
        ops.retain(|op| op.mutating);
    }
    if let Some(pr) = args.pr {
        ops.retain(|op| op.touches_pr(pr));
    }
    if let Some(branch) = &args.branch {
        ops.retain(|op| op.touches_branch(branch));
    }
    if let Some(commit) = &args.commit {
        let sha7 = rogrep_tooltree::git::short_sha(commit);
        ops.retain(|op| op.touches_commit(&sha7));
    }

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "schema": "rogrep/v1",
                "command": "git",
                "conversation_id": row.id,
                "ops": ops,
            })
        );
        return Ok(());
    }

    if ops.is_empty() {
        println!(
            "no indexed git/gh activity matched; this does not rule out filesystem, shell, SSH, or unparsed work."
        );
        return Ok(());
    }
    println!("{} git op(s) in {}:", ops.len(), row.id);
    for op in &ops {
        let when = op
            .ts
            .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
            .map(|ts| {
                ts.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "?".into());
        let mut tags = Vec::new();
        if let Some(n) = op.created_pr {
            tags.push(format!("created PR #{n}"));
        } else if !op.pr_numbers.is_empty() {
            tags.push(format!(
                "PR {}",
                op.pr_numbers.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(",")
            ));
        }
        if !op.commits.is_empty() {
            tags.push(op.commits.join(","));
        }
        if !op.branches.is_empty() {
            tags.push(op.branches.join(","));
        }
        let marker = if op.mutating { "*" } else { " " };
        println!(
            "  [{:>4}]{} {} {} {}  ({})",
            op.turn_index,
            marker,
            when,
            op.command.chars().take(90).collect::<String>(),
            if tags.is_empty() { String::new() } else { format!("→ {}", tags.join(" ")) },
            op.status.as_str(),
        );
    }
    println!("(* = mutating)  inspect: rogrep show {} --around <TURN> --limit 20", row.id);
    Ok(())
}
