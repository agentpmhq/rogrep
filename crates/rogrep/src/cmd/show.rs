use anyhow::{bail, Context, Result};
use clap::Args;
use rogrep_model::ids::{ConversationId, ExchangeRef};
use rogrep_model::paths::DataLayout;
use rogrep_model::{build_exchanges, Conversation, Role};
use rogrep_store::Store;

#[derive(Args)]
pub struct ShowArgs {
    /// Conversation id (rg_…), id prefix, or exchange ref (rg_…#eN).
    pub id: String,
    /// Show one specific turn.
    #[arg(long)]
    pub turn: Option<u32>,
    /// Center the window on this turn.
    #[arg(long)]
    pub around: Option<u32>,
    /// Show one exchange (1-based ordinal).
    #[arg(long)]
    pub exchange: Option<u32>,
    /// Start offset in turns.
    #[arg(long, default_value_t = 0)]
    pub offset: u32,
    /// Max turns to print.
    #[arg(long, default_value_t = 40)]
    pub limit: u32,
    /// Print full turn bodies without truncation/summarization.
    #[arg(long)]
    pub raw: bool,
    #[arg(long)]
    pub json: bool,
    /// Open the conversation in the interactive TUI at this position.
    #[arg(long, conflicts_with = "json")]
    pub tui: bool,
}

const MAX_TURN_TEXT_BYTES: usize = 2400;

/// Load a conversation by re-parsing its source file (fast: ~160MB/s) so
/// `show` renders exact normalized text without storing it twice.
pub fn load_conversation(store: &Store, id: &str) -> Result<(rogrep_store::ConversationRow, Conversation)> {
    let row = store
        .conversation_row(id)?
        .or(store.conversation_by_prefix(id)?)
        .with_context(|| format!("conversation {id} not found; try `rogrep ls`"))?;
    let provider_kind = rogrep_model::AgentKind::parse(&row.provider)
        .with_context(|| format!("unknown provider {}", row.provider))?;
    let provider = rogrep_parsers::provider_for_kind(provider_kind).context("provider missing")?;
    let out = rogrep_parsers::parse_source(provider, std::path::Path::new(&row.source_path), None)
        .with_context(|| format!("re-reading {}", row.source_path))?;
    Ok((row, out.conversation))
}

pub fn show_by_ref(store: &Store, _layout: &DataLayout, reference: &str, json: bool) -> Result<()> {
    if let Some(exref) = ExchangeRef::parse(reference) {
        return run_inner(
            store,
            ShowArgs {
                id: exref.conversation.to_string(),
                turn: None,
                around: None,
                exchange: Some(exref.ordinal),
                offset: 0,
                limit: 200,
                raw: false,
                json,
                tui: false,
            },
        );
    }
    run_inner(
        store,
        ShowArgs {
            id: reference.to_string(),
            turn: None,
            around: None,
            exchange: None,
            offset: 0,
            limit: 40,
            raw: false,
            json,
            tui: false,
        },
    )
}

pub fn run(args: ShowArgs) -> Result<()> {
    // Accept `rg_…#eN` in the positional id too.
    let mut args = args;
    if let Some(exref) = ExchangeRef::parse(&args.id) {
        args.id = exref.conversation.to_string();
        args.exchange.get_or_insert(exref.ordinal);
    }
    if args.tui {
        return crate::tui::run(crate::tui::Entry::Conversation {
            id: args.id,
            around: args.around.or(args.turn),
            exchange: args.exchange,
        });
    }
    let (_layout, _config, store) = super::sync::sync_now(false, true)?;
    run_inner(&store, args)
}

fn run_inner(store: &Store, args: ShowArgs) -> Result<()> {
    if !ConversationId::looks_like_id(&args.id) && !args.id.starts_with(rogrep_model::ids::ID_PREFIX) {
        bail!("{} does not look like a conversation id; try `rogrep ls`", args.id);
    }
    let (row, conv) = load_conversation(store, &args.id)?;
    let exchanges = build_exchanges(&conv.turns);

    // Resolve the turn window.
    let (start, end) = if let Some(t) = args.turn {
        (t, t + 1)
    } else if let Some(a) = args.around {
        (a.saturating_sub(args.limit / 2), a.saturating_sub(args.limit / 2) + args.limit)
    } else if let Some(e1) = args.exchange {
        let ex = exchanges
            .iter()
            .find(|e| e.ordinal + 1 == e1)
            .with_context(|| format!("exchange #e{e1} not found (conversation has {})", exchanges.len()))?;
        (ex.start_turn, ex.end_turn)
    } else {
        (args.offset, args.offset + args.limit)
    };
    let turns: Vec<_> = conv
        .turns
        .iter()
        .filter(|t| t.turn_index >= start && t.turn_index < end)
        .collect();

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "schema": "rogrep/v1",
                "command": "show",
                "conversation": row,
                "window": {"start": start, "end": end},
                "exchanges": exchanges,
                "turns": turns,
            })
        );
        return Ok(());
    }

    let paint = crate::color::Painter::auto();
    println!(
        "{}  [{}] {}",
        paint.paint(crate::color::CYAN, row.id.as_str()),
        paint.provider(&row.provider),
        paint.paint(crate::color::BOLD, &conv.display_title())
    );
    println!(
        "{}",
        paint.paint(
            crate::color::DIM,
            &format!(
                "project {}  turns {}  exchanges {}  source {}",
                row.normalized_project,
                conv.turns.len(),
                exchanges.len(),
                row.source_path
            )
        )
    );
    println!();
    let mut last_exchange: Option<u32> = None;
    for t in turns {
        let ex = exchanges
            .iter()
            .find(|e| t.turn_index >= e.start_turn && t.turn_index < e.end_turn)
            .map(|e| e.ordinal);
        if ex != last_exchange {
            if let Some(e) = ex.and_then(|o| exchanges.get(o as usize)) {
                println!(
                    "{} {}",
                    paint.paint(crate::color::BOLD_YELLOW, &format!("── #e{} ──", e.ordinal + 1)),
                    paint.paint(crate::color::YELLOW, &e.user_preview.chars().take(90).collect::<String>())
                );
            }
            last_exchange = ex;
        }
        let (role, role_color) = match t.role {
            Role::User => ("user", crate::color::BOLD_GREEN),
            Role::Assistant => ("assistant", crate::color::BOLD_BLUE),
            Role::Tool => ("tool", crate::color::MAGENTA),
            Role::System => ("system", crate::color::DIM),
            Role::Event => ("event", crate::color::DIM),
        };
        let speaker = if t.speaker.is_empty() || t.speaker == role {
            role.to_string()
        } else {
            format!("{role}/{}", t.speaker)
        };
        let mut text = t.text.clone();
        let mut truncated = false;
        if !args.raw && text.len() > MAX_TURN_TEXT_BYTES {
            let mut cut = MAX_TURN_TEXT_BYTES;
            while cut > 0 && !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            truncated = true;
        }
        let mut indented = text.replace('\n', "\n    ");
        if truncated {
            indented.push_str(&format!(
                "\n    {}",
                paint.paint(crate::color::DIM, "[truncated; use --json or --raw for exact turn text]")
            ));
        }
        println!(
            "{} {} {indented}",
            paint.paint(crate::color::DIM, &format!("[{:>4}]", t.turn_index)),
            paint.paint(role_color, &format!("{speaker}:")),
        );
    }
    Ok(())
}
