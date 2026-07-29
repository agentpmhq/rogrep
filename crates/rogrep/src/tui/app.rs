use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Terminal;
use rogrep_index::{parse_query, ConversationMatches, SearchIndex};
use rogrep_model::{build_exchanges, Conversation, Exchange, Role};
use rogrep_store::{ConversationRow, Store};
use std::time::Duration;

enum Screen {
    Search,
    Conversation,
    Stats,
}

/// How tool turns render in the conversation view (`t` cycles).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolDisplay {
    Full,
    Collapsed,
    Hidden,
}

impl ToolDisplay {
    fn next(self) -> ToolDisplay {
        match self {
            ToolDisplay::Full => ToolDisplay::Collapsed,
            ToolDisplay::Collapsed => ToolDisplay::Hidden,
            ToolDisplay::Hidden => ToolDisplay::Full,
        }
    }

    fn label(self) -> &'static str {
        match self {
            ToolDisplay::Full => "full",
            ToolDisplay::Collapsed => "collapsed",
            ToolDisplay::Hidden => "hidden",
        }
    }
}

pub struct App {
    store: Store,
    index: SearchIndex,
    screen: Screen,
    // Search screen.
    query: String,
    results: Vec<(ConversationMatches, Option<ConversationRow>)>,
    recent: Vec<ConversationRow>,
    selected: usize,
    status: String,
    // Conversation screen.
    conv: Option<Conversation>,
    conv_row: Option<ConversationRow>,
    exchanges: Vec<Exchange>,
    exchange_idx: usize,
    scroll_turn: usize,
    find: String,
    find_active: bool,
    find_matches: Vec<usize>,
    find_pos: usize,
    tool_display: ToolDisplay,
    // Stats screen.
    heatmap: [[u64; 24]; 7],
    top: Vec<rogrep_store::stats::TopExchange>,
}

impl App {
    pub fn new(store: Store, index: SearchIndex, initial_query: Option<String>) -> App {
        let mut app = App {
            store,
            index,
            screen: Screen::Search,
            query: initial_query.unwrap_or_default(),
            results: Vec::new(),
            recent: Vec::new(),
            selected: 0,
            status: String::new(),
            conv: None,
            conv_row: None,
            exchanges: Vec::new(),
            exchange_idx: 0,
            scroll_turn: 0,
            find: String::new(),
            find_active: false,
            find_matches: Vec::new(),
            find_pos: 0,
            tool_display: ToolDisplay::Full,
            heatmap: [[0; 24]; 7],
            top: Vec::new(),
        };
        app.recent = app.store.recent_conversations(50, None).unwrap_or_default();
        app.refresh_search();
        app
    }

    pub fn run<B: ratatui::backend::Backend>(mut self, terminal: &mut Terminal<B>) -> Result<()> {
        loop {
            terminal.draw(|f| self.draw(f))?;
            if !event::poll(Duration::from_millis(250))? {
                continue;
            }
            if let Event::Key(key) = event::read()? {
                if key.kind != event::KeyEventKind::Press {
                    continue;
                }
                if self.handle_key(key)? {
                    return Ok(());
                }
            }
        }
    }

    fn refresh_search(&mut self) {
        self.selected = 0;
        self.results.clear();
        let parsed = parse_query(&self.query);
        if parsed.is_empty() {
            self.status = format!("{} recent conversations — type to search", self.recent.len());
            return;
        }
        match self.index.search(&parsed, None, 300) {
            Ok(mut matches) => {
                // Recency-decayed relevance, same as the CLI.
                let now = jiff::Timestamp::now().as_millisecond();
                for m in &mut matches {
                    if let Some(ts) = m.last_ts {
                        let age_days = ((now - ts).max(0) as f64) / 86_400_000.0;
                        m.best_score *= 2f64.powf(-age_days / 30.0) as f32;
                    }
                }
                matches.sort_by(|a, b| b.best_score.total_cmp(&a.best_score));
                matches.truncate(50);
                self.status = format!(
                    "{} conversations — terms {:?} facets {:?}",
                    matches.len(),
                    parsed.terms,
                    parsed.facets
                );
                self.results = matches
                    .into_iter()
                    .map(|m| {
                        let row = self.store.conversation_row(&m.conversation_id).ok().flatten();
                        (m, row)
                    })
                    .collect();
            }
            Err(e) => self.status = format!("search error: {e}"),
        }
    }

    fn open_selected(&mut self) {
        let id = if self.results.is_empty() {
            self.recent.get(self.selected).map(|r| r.id.clone())
        } else {
            self.results.get(self.selected).map(|(m, _)| m.conversation_id.clone())
        };
        let Some(id) = id else { return };
        // Land on the best-matching turn when arriving from a search hit.
        let target_turn = self
            .results
            .get(self.selected)
            .and_then(|(m, _)| m.best.as_ref().map(|b| b.turn_index));
        if let Err(e) = self.open_conversation(&id, target_turn) {
            self.status = format!("open failed: {e}");
        }
    }

    /// Open one conversation directly (also the `rogrep tui rg_…` /
    /// `rogrep show --tui` entry point). `around` beats `exchange` (1-based
    /// ordinal) when both are given.
    pub fn open_conversation(&mut self, id: &str, around: Option<u32>) -> anyhow::Result<()> {
        self.open_conversation_at(id, around, None)
    }

    pub fn open_conversation_at(
        &mut self,
        id: &str,
        around: Option<u32>,
        exchange: Option<u32>,
    ) -> anyhow::Result<()> {
        let (row, conv) = crate::cmd::show::load_conversation(&self.store, id)?;
        self.exchanges = build_exchanges(&conv.turns);
        let around = around.or_else(|| {
            exchange.and_then(|e1| {
                self.exchanges
                    .iter()
                    .find(|e| e.ordinal + 1 == e1)
                    .map(|e| e.start_turn)
            })
        });
        self.exchange_idx = around
            .and_then(|t| self.exchanges.iter().position(|e| t >= e.start_turn && t < e.end_turn))
            .unwrap_or(0);
        self.scroll_turn = around
            .map(|t| t as usize)
            .unwrap_or_else(|| self.exchanges.get(self.exchange_idx).map(|e| e.start_turn as usize).unwrap_or(0));
        self.conv = Some(conv);
        self.conv_row = Some(row);
        self.find.clear();
        self.find_matches.clear();
        self.screen = Screen::Conversation;
        Ok(())
    }

    fn open_stats(&mut self) {
        let tz = jiff::tz::TimeZone::system();
        self.heatmap = rogrep_store::stats::heatmap(&self.store, &tz, None).unwrap_or([[0; 24]; 7]);
        self.top = rogrep_store::stats::top_exchanges(&self.store, rogrep_store::stats::TopBy::Tokens, 15, None)
            .unwrap_or_default();
        self.screen = Screen::Stats;
    }

    fn run_find(&mut self) {
        self.find_matches.clear();
        self.find_pos = 0;
        let needle = self.find.to_lowercase();
        if needle.is_empty() {
            return;
        }
        if let Some(conv) = &self.conv {
            self.find_matches = conv
                .turns
                .iter()
                .filter(|t| t.text.to_lowercase().contains(&needle))
                .map(|t| t.turn_index as usize)
                .collect();
            if let Some(&first) = self.find_matches.first() {
                self.jump_to_turn(first);
            }
        }
    }

    fn jump_to_turn(&mut self, turn: usize) {
        self.scroll_turn = turn;
        if let Some(idx) = self
            .exchanges
            .iter()
            .position(|e| (turn as u32) >= e.start_turn && (turn as u32) < e.end_turn)
        {
            self.exchange_idx = idx;
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Ok(true);
        }
        match self.screen {
            Screen::Search => match key.code {
                KeyCode::Esc => return Ok(true),
                KeyCode::Enter => self.open_selected(),
                KeyCode::Up => self.selected = self.selected.saturating_sub(1),
                KeyCode::Down => {
                    let len = if self.results.is_empty() { self.recent.len() } else { self.results.len() };
                    self.selected = (self.selected + 1).min(len.saturating_sub(1));
                }
                KeyCode::Char('t') if ctrl => self.open_stats(),
                KeyCode::Backspace => {
                    self.query.pop();
                    self.refresh_search();
                }
                KeyCode::Char(c) if !ctrl => {
                    self.query.push(c);
                    self.refresh_search();
                }
                _ => {}
            },
            Screen::Conversation => {
                if self.find_active {
                    match key.code {
                        KeyCode::Esc => {
                            self.find_active = false;
                            self.find.clear();
                        }
                        KeyCode::Enter => {
                            self.find_active = false;
                            self.run_find();
                        }
                        KeyCode::Backspace => {
                            self.find.pop();
                        }
                        KeyCode::Char(c) => self.find.push(c),
                        _ => {}
                    }
                    return Ok(false);
                }
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Search,
                    KeyCode::Char('t') => self.tool_display = self.tool_display.next(),
                    KeyCode::Char('/') => {
                        self.find_active = true;
                        self.find.clear();
                    }
                    KeyCode::Char('n') => {
                        if !self.find_matches.is_empty() {
                            self.find_pos = (self.find_pos + 1) % self.find_matches.len();
                            let t = self.find_matches[self.find_pos];
                            self.jump_to_turn(t);
                        }
                    }
                    KeyCode::Char('N') => {
                        if !self.find_matches.is_empty() {
                            self.find_pos = (self.find_pos + self.find_matches.len() - 1) % self.find_matches.len();
                            let t = self.find_matches[self.find_pos];
                            self.jump_to_turn(t);
                        }
                    }
                    KeyCode::Char('j') | KeyCode::Down => self.scroll(1),
                    KeyCode::Char('k') | KeyCode::Up => self.scroll(-1),
                    KeyCode::PageDown | KeyCode::Char('d') => self.scroll(10),
                    KeyCode::PageUp | KeyCode::Char('u') => self.scroll(-10),
                    KeyCode::Char('g') => self.jump_to_turn(0),
                    KeyCode::Char('G') => {
                        let last = self.conv.as_ref().map(|c| c.turns.len().saturating_sub(1)).unwrap_or(0);
                        self.jump_to_turn(last);
                    }
                    KeyCode::Char(']') => {
                        if self.exchange_idx + 1 < self.exchanges.len() {
                            self.exchange_idx += 1;
                            self.scroll_turn = self.exchanges[self.exchange_idx].start_turn as usize;
                        }
                    }
                    KeyCode::Char('[') => {
                        if self.exchange_idx > 0 {
                            self.exchange_idx -= 1;
                            self.scroll_turn = self.exchanges[self.exchange_idx].start_turn as usize;
                        }
                    }
                    _ => {}
                }
            }
            Screen::Stats => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Search,
                _ => {}
            },
        }
        Ok(false)
    }

    fn scroll(&mut self, delta: i64) {
        let max = self.conv.as_ref().map(|c| c.turns.len().saturating_sub(1)).unwrap_or(0);
        let next = (self.scroll_turn as i64 + delta).clamp(0, max as i64) as usize;
        self.jump_to_turn(next);
    }

    fn draw(&mut self, f: &mut ratatui::Frame<'_>) {
        match self.screen {
            Screen::Search => self.draw_search(f),
            Screen::Conversation => self.draw_conversation(f),
            Screen::Stats => self.draw_stats(f),
        }
    }

    fn draw_search(&mut self, f: &mut ratatui::Frame<'_>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(4), Constraint::Length(1)])
            .split(f.area());
        let input = Paragraph::new(self.query.as_str())
            .block(Block::default().borders(Borders::ALL).title(" search (Enter open · ↑↓ select · Ctrl-T stats · Esc quit) "));
        f.render_widget(input, chunks[0]);

        let items: Vec<ListItem> = if self.results.is_empty() {
            self.recent
                .iter()
                .map(|r| {
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("[{:<8}] ", r.provider), Style::default().fg(Color::Cyan)),
                        Span::raw(r.title.clone().unwrap_or_default().chars().take(90).collect::<String>()),
                        Span::styled(
                            format!("  {} turns", r.turn_count),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]))
                })
                .collect()
        } else {
            self.results
                .iter()
                .map(|(m, row)| {
                    let title = row
                        .as_ref()
                        .and_then(|r| r.title.clone())
                        .unwrap_or_else(|| m.conversation_id.clone());
                    let provider = row.as_ref().map(|r| r.provider.clone()).unwrap_or_default();
                    let excerpt = m
                        .best
                        .as_ref()
                        .map(|b| b.excerpt.chars().take(120).collect::<String>())
                        .unwrap_or_default();
                    ListItem::new(vec![
                        Line::from(vec![
                            Span::styled(format!("[{provider:<8}] "), Style::default().fg(Color::Cyan)),
                            Span::styled(
                                title.chars().take(90).collect::<String>(),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(format!("  ({} hits)", m.match_count), Style::default().fg(Color::DarkGray)),
                        ]),
                        Line::from(Span::styled(format!("    {excerpt}"), Style::default().fg(Color::Gray))),
                    ])
                })
                .collect()
        };
        let mut state = ListState::default();
        state.select(Some(self.selected));
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" conversations "))
            .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(list, chunks[1], &mut state);
        f.render_widget(
            Paragraph::new(self.status.as_str()).style(Style::default().fg(Color::DarkGray)),
            chunks[2],
        );
    }

    fn draw_conversation(&mut self, f: &mut ratatui::Frame<'_>) {
        let Some(conv) = &self.conv else { return };
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(4), Constraint::Length(1)])
            .split(f.area());
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(outer[0]);

        // Exchange sidebar.
        let items: Vec<ListItem> = self
            .exchanges
            .iter()
            .map(|e| {
                let preview = if e.user_preview.is_empty() {
                    "(preamble)".to_string()
                } else {
                    e.user_preview.chars().take(60).collect()
                };
                let mut flags = String::new();
                if e.signals.error {
                    flags.push('✗');
                }
                if e.signals.interrupted {
                    flags.push('⏻');
                }
                ListItem::new(Line::from(vec![
                    Span::styled(format!("#e{:<3}", e.ordinal + 1), Style::default().fg(Color::Yellow)),
                    Span::raw(format!(" {flags} {preview}")),
                ]))
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.exchange_idx));
        let title = conv.display_title().chars().take(40).collect::<String>();
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(format!(" {title} ")))
            .highlight_style(Style::default().bg(Color::DarkGray));
        f.render_stateful_widget(list, chunks[0], &mut state);

        // Turn view from scroll_turn.
        self.draw_turns(f, chunks[1], conv);

        let footer = if self.find_active {
            format!("find: {}▏", self.find)
        } else if !self.find_matches.is_empty() {
            format!(
                "match {}/{} for \"{}\" — n/N jump · t tools:{} · [ ] exchanges · j/k scroll · q back",
                self.find_pos + 1,
                self.find_matches.len(),
                self.find,
                self.tool_display.label()
            )
        } else {
            format!(
                "j/k scroll · d/u page · [ ] exchanges · / find · t tools:{} · g/G ends · q back",
                self.tool_display.label()
            )
        };
        f.render_widget(
            Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
            outer[1],
        );
    }

    fn draw_turns(&self, f: &mut ratatui::Frame<'_>, area: Rect, conv: &Conversation) {
        let mut lines: Vec<Line> = Vec::new();
        let needle = self.find.to_lowercase();
        let budget = area.height as usize * 3;
        let find_target = self.find_matches.get(self.find_pos).copied();
        let mut hidden_run: usize = 0;
        let flush_hidden = |run: &mut usize, lines: &mut Vec<Line>| {
            if *run > 0 {
                lines.push(Line::from(Span::styled(
                    format!("  · {} tool turn(s) hidden (t to show)", run),
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )));
                lines.push(Line::default());
                *run = 0;
            }
        };
        for t in conv.turns.iter().skip(self.scroll_turn) {
            if lines.len() > budget {
                break;
            }
            // Tool-turn display mode. The current find target always renders
            // in full so n/N jumps never land on nothing.
            let is_find_target = find_target == Some(t.turn_index as usize);
            if t.role == Role::Tool && !is_find_target {
                match self.tool_display {
                    ToolDisplay::Hidden => {
                        hidden_run += 1;
                        continue;
                    }
                    ToolDisplay::Collapsed => {
                        flush_hidden(&mut hidden_run, &mut lines);
                        let head: String =
                            t.text.replace('\n', " ").chars().take(100).collect();
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("[{:>4}] ", t.turn_index),
                                Style::default().fg(Color::DarkGray),
                            ),
                            Span::styled(
                                format!("{}: ", t.speaker),
                                Style::default().fg(Color::Magenta),
                            ),
                            Span::styled(head, Style::default().fg(Color::DarkGray)),
                        ]));
                        continue;
                    }
                    ToolDisplay::Full => {}
                }
            }
            flush_hidden(&mut hidden_run, &mut lines);
            let (color, label) = match t.role {
                Role::User => (Color::Green, "user"),
                Role::Assistant => (Color::Blue, "assistant"),
                Role::Tool => (Color::Magenta, "tool"),
                Role::System => (Color::DarkGray, "system"),
                Role::Event => (Color::DarkGray, "event"),
            };
            let speaker = if t.speaker.is_empty() || t.speaker == label {
                label.to_string()
            } else {
                format!("{label}/{}", t.speaker)
            };
            lines.push(Line::from(vec![
                Span::styled(format!("[{:>4}] ", t.turn_index), Style::default().fg(Color::DarkGray)),
                Span::styled(speaker, Style::default().fg(color).add_modifier(Modifier::BOLD)),
            ]));
            let text: String = t.text.chars().take(2000).collect();
            for raw_line in text.lines().take(24) {
                let matched = !needle.is_empty() && raw_line.to_lowercase().contains(&needle);
                let style = if matched {
                    Style::default().fg(Color::Black).bg(Color::Yellow)
                } else {
                    Style::default()
                };
                lines.push(Line::from(Span::styled(format!("  {raw_line}"), style)));
            }
            lines.push(Line::default());
        }
        flush_hidden(&mut hidden_run, &mut lines);
        let para = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(format!(
                " turns {}..{} · tools {} ",
                self.scroll_turn,
                conv.turns.len(),
                self.tool_display.label()
            )));
        f.render_widget(para, area);
    }

    #[cfg(test)]
    fn set_screen_for_test(&mut self, which: u8) {
        self.screen = match which {
            1 => Screen::Conversation,
            2 => Screen::Stats,
            _ => Screen::Search,
        };
    }

    fn draw_stats(&mut self, f: &mut ratatui::Frame<'_>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(11), Constraint::Min(4), Constraint::Length(1)])
            .split(f.area());

        let max = self.heatmap.iter().flatten().copied().max().unwrap_or(0).max(1);
        let shades = [' ', '·', '▂', '▄', '▆', '█'];
        let mut lines = vec![Line::from("        00    04    08    12    16    20")];
        for (d, name) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().enumerate() {
            let mut row = String::new();
            for h in 0..24 {
                let v = self.heatmap[d][h];
                let idx = if v == 0 { 0 } else { 1 + ((v * (shades.len() as u64 - 2)) / max) as usize };
                row.push(shades[idx.min(shades.len() - 1)]);
            }
            lines.push(Line::from(format!("{name}  |{row}|")));
        }
        f.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" activity by local hour ")),
            chunks[0],
        );

        let items: Vec<ListItem> = self
            .top
            .iter()
            .map(|t| {
                let dur = t
                    .duration_ms
                    .map(|ms| format!("{:>6.0}s", ms as f64 / 1000.0))
                    .unwrap_or_else(|| "     ?".into());
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}#e{:<3}", &t.conversation_id[..12.min(t.conversation_id.len())], t.ordinal + 1),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(format!(
                        " {dur} {:>4}t {:>8}tok  {}",
                        t.turns,
                        t.total_tokens,
                        t.user_preview.chars().take(70).collect::<String>()
                    )),
                ]))
            })
            .collect();
        f.render_widget(
            List::new(items).block(Block::default().borders(Borders::ALL).title(" top exchanges by tokens ")),
            chunks[1],
        );
        f.render_widget(
            Paragraph::new("q back").style(Style::default().fg(Color::DarkGray)),
            chunks[2],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    /// Headless render of all three screens against a real ingested fixture.
    #[test]
    fn all_screens_render() {
        let tmp = tempfile::tempdir().unwrap();
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../rogrep-parsers/fixtures/claude/basic_session.jsonl"
        );
        // The session must live at a real claude-shaped path: open_selected
        // re-parses from the stored source path.
        let proj = tmp.path().join("home/.claude/projects/-home-u-src-proj");
        std::fs::create_dir_all(&proj).unwrap();
        let session = proj.join("sess-1.jsonl");
        std::fs::copy(fixture, &session).unwrap();

        let mut store = Store::open(&tmp.path().join("db.sqlite3")).unwrap();
        let index = SearchIndex::open_or_create(&tmp.path().join("index")).unwrap();
        let provider = rogrep_parsers::provider_for_kind(rogrep_model::AgentKind::Claude).unwrap();
        let out = rogrep_parsers::parse_source(provider, &session, None).unwrap();
        let mut batch = index.writer().unwrap();
        batch.apply(&out).unwrap();
        batch.commit().unwrap();
        drop(batch);
        store.apply_parse(&out, 1, 1).unwrap();

        let mut app = App::new(store, index, Some("flaky".to_string()));
        assert!(!app.results.is_empty(), "search found the fixture");

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();

        app.open_selected();
        assert!(app.conv.is_some(), "conversation loaded from search hit");
        app.set_screen_for_test(1);
        terminal.draw(|f| app.draw(f)).unwrap();
        // In-conversation find jumps to a match.
        app.find = "offsets".into();
        app.run_find();
        assert!(!app.find_matches.is_empty());
        terminal.draw(|f| app.draw(f)).unwrap();

        app.open_stats();
        app.set_screen_for_test(2);
        terminal.draw(|f| app.draw(f)).unwrap();

        // Direct conversation entry (the `rogrep show --tui` path), landing
        // on a specific turn, then cycle tool display through all modes.
        let id = app.conv_row.as_ref().unwrap().id.clone();
        app.open_conversation(&id, Some(3)).unwrap();
        assert_eq!(app.scroll_turn, 3);
        let rendered_modes: Vec<&str> = (0..3)
            .map(|_| {
                app.tool_display = app.tool_display.next();
                terminal.draw(|f| app.draw(f)).unwrap();
                app.tool_display.label()
            })
            .collect();
        assert_eq!(rendered_modes, vec!["collapsed", "hidden", "full"]);

        // Exchange-ordinal entry (`rogrep tui rg_…#e2`).
        app.open_conversation_at(&id, None, Some(2)).unwrap();
        assert_eq!(app.exchange_idx, 1);
        assert_eq!(app.scroll_turn as u32, app.exchanges[1].start_turn);
    }
}
