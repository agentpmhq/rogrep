//! The rogrep TUI: three screens.
//!
//! Search — live query over the corpus, results with excerpts.
//! Conversation — exchange sidebar + turn view with in-conversation find.
//! Stats — activity heatmap + top exchanges.
//!
//! Never launch from a non-interactive session (the CLI checks isatty).

pub(crate) mod app;

use anyhow::Result;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;

/// Where the TUI opens.
pub enum Entry {
    /// Search screen, optionally pre-filled.
    Search(Option<String>),
    /// Straight into one conversation.
    Conversation {
        id: String,
        /// Land on this turn (its exchange is selected too).
        around: Option<u32>,
        /// Or land on this exchange (1-based ordinal).
        exchange: Option<u32>,
    },
}

pub fn run(entry: Entry) -> Result<()> {
    if !crossterm::tty::IsTty::is_tty(&std::io::stdout()) {
        anyhow::bail!("rogrep tui needs an interactive terminal; use `rogrep search`/`show --json` from scripts and agents");
    }
    let (layout, _config, store) = crate::cmd::sync::sync_now(false, true)?;
    let index = crate::cmd::index::open_search_index(&layout)?;

    enable_raw_mode()?;
    std::io::stdout().execute(EnterAlternateScreen)?;
    // Restore the terminal even on panic.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = std::io::stdout().execute(LeaveAlternateScreen);
        original_hook(info);
    }));

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;
    let result = (|| {
        let app = match entry {
            Entry::Search(query) => app::App::new(store, index, query),
            Entry::Conversation { id, around, exchange } => {
                let mut app = app::App::new(store, index, None);
                app.open_conversation_at(&id, around, exchange)?;
                app
            }
        };
        app.run(&mut terminal)
    })();

    disable_raw_mode()?;
    std::io::stdout().execute(LeaveAlternateScreen)?;
    result
}
