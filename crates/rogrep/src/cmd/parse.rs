use anyhow::{bail, Context};
use clap::Args;
use rogrep_model::build_exchanges;
use std::path::PathBuf;

#[derive(Args)]
pub struct ParseArgs {
    /// Rollout file to parse.
    pub file: PathBuf,
    /// Emit the full normalized conversation as JSON.
    #[arg(long)]
    pub json: bool,
    /// Force a provider instead of path-based detection
    /// (claude|codex|cursor|grok|hermes|opencode|generic).
    #[arg(long)]
    pub provider: Option<String>,
}

pub fn run(args: ParseArgs) -> anyhow::Result<()> {
    let path = args.file.canonicalize().context("resolving file path")?;
    let path_str = path.to_string_lossy().to_string();
    let provider = match &args.provider {
        Some(name) => {
            let kind = rogrep_model::AgentKind::parse(name)
                .with_context(|| format!("unknown provider {name}"))?;
            rogrep_parsers::provider_for_kind(kind)
                .with_context(|| format!("provider {name} not registered"))?
        }
        None => match rogrep_parsers::provider_for_path(&path_str) {
            Some(p) => p,
            None => bail!(
                "no provider claims {path_str}; pass --provider generic to force the fallback"
            ),
        },
    };
    let out = rogrep_parsers::parse_source(provider, &path, None)?;
    let conv = &out.conversation;

    if args.json {
        serde_json::to_writer_pretty(std::io::stdout().lock(), conv)?;
        println!();
        return Ok(());
    }

    let exchanges = build_exchanges(&conv.turns);
    println!("id:       {}", conv.id);
    println!("agent:    {}", conv.agent);
    println!("title:    {}", conv.display_title());
    println!("project:  {}", conv.normalized_project);
    if let Some(cwd) = &conv.cwd {
        println!("cwd:      {cwd}");
    }
    if let Some(model) = &conv.model {
        println!("model:    {model}");
    }
    println!(
        "turns:    {} ({} exchanges, {} malformed lines)",
        conv.turns.len(),
        exchanges.len(),
        conv.malformed_lines
    );
    let t = conv.tokens;
    println!(
        "tokens:   in={} out={} cache_r={} cache_w={} (est {})",
        t.input, t.output, t.cache_read, t.cache_creation, t.estimated
    );
    for ex in exchanges.iter() {
        let dur = ex
            .duration_ms()
            .map(|ms| format!("{:.0}s", ms as f64 / 1000.0))
            .unwrap_or_else(|| "?".into());
        let preview = if ex.user_preview.is_empty() {
            "(preamble)".to_string()
        } else {
            ex.user_preview.chars().take(72).collect()
        };
        let mut flags = String::new();
        if ex.signals.error {
            flags.push('✗');
        }
        if ex.signals.interrupted {
            flags.push('⏻');
        }
        if ex.signals.compacted {
            flags.push('§');
        }
        println!(
            "  #e{:<3} turns {:>3}..{:<3} {:>6} tools={:<3} {} {}",
            ex.ordinal + 1,
            ex.start_turn,
            ex.end_turn,
            dur,
            ex.tool_calls,
            flags,
            preview
        );
    }
    Ok(())
}
