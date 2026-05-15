use anyhow::Result;
use async_trait::async_trait;
use crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use openbuild_core::Event as CoreEvent;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::io::stdout;
use tokio::sync::mpsc;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Clone)]
pub enum Block_ {
    User(String),
    Assistant(String),
    Thinking(String),
    Tool { name: String, body: String },
    Error(String),
    System(String),
}

#[derive(Default)]
pub struct Status {
    pub provider: String,
    pub model: String,
    pub mode: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub turn: u32,
}

#[async_trait]
pub trait Backend: Send {
    async fn send(&mut self, prompt: String, out: mpsc::Sender<CoreEvent>);
    fn slash(&mut self, _cmd: &str, _arg: &str) -> Option<String> {
        None
    }
    fn status(&self) -> Status {
        Status::default()
    }
}

pub async fn run_streaming<B: Backend>(backend: &mut B, alt_screen: bool) -> Result<()> {
    enable_raw_mode()?;
    if alt_screen {
        execute!(stdout(), EnterAlternateScreen)?;
    }
    let result = run_loop(backend).await;
    if alt_screen {
        execute!(stdout(), LeaveAlternateScreen).ok();
    }
    disable_raw_mode().ok();
    result
}

#[derive(Default)]
struct UiState {
    history: Vec<Block_>,
    input: String,
    streaming: Option<String>,
    thinking: Option<String>,
    tool_active: Option<String>,
    spinner_idx: usize,
    scroll: u16,
    quit: bool,
    show_help: bool,
}

async fn run_loop<B: Backend>(backend: &mut B) -> Result<()> {
    let backend_term = ratatui::backend::CrosstermBackend::new(stdout());
    let mut term = Terminal::new(backend_term)?;
    let mut st = UiState::default();

    splash(&mut st);

    while !st.quit {
        let status = backend.status();
        term.draw(|f: &mut Frame| draw(f, &status, &st))?;

        if event::poll(std::time::Duration::from_millis(80))? {
            handle_key(&mut st, backend, &mut term, &status).await?;
        }
        st.spinner_idx = (st.spinner_idx + 1) % SPINNER.len();
    }
    Ok(())
}

fn splash(st: &mut UiState) {
    st.history.push(Block_::System(String::from(
        "openbuild — model-agnostic agent shell\nany model. any agent's config. zero phone-home.\n/help for commands · enter to send · shift+enter newline · esc to quit",
    )));
}

async fn handle_key<B: Backend>(
    st: &mut UiState,
    backend: &mut B,
    term: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    status: &Status,
) -> Result<()> {
    let ev = event::read()?;
    let event::Event::Key(KeyEvent {
        code,
        kind: KeyEventKind::Press,
        modifiers,
        ..
    }) = ev
    else {
        return Ok(());
    };

    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('c') => {
                st.quit = true;
                return Ok(());
            }
            KeyCode::Char('l') => {
                st.history.clear();
                return Ok(());
            }
            KeyCode::Char('u') => {
                st.input.clear();
                return Ok(());
            }
            _ => {}
        }
    }

    match code {
        KeyCode::Esc => st.quit = true,
        KeyCode::PageUp => st.scroll = st.scroll.saturating_add(5),
        KeyCode::PageDown => st.scroll = st.scroll.saturating_sub(5),
        KeyCode::F(1) => st.show_help = !st.show_help,
        KeyCode::Enter if modifiers.contains(KeyModifiers::SHIFT) => {
            st.input.push('\n');
        }
        KeyCode::Enter => {
            let line = std::mem::take(&mut st.input);
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return Ok(());
            }
            if let Some(rest) = trimmed.strip_prefix('/') {
                run_slash(st, backend, rest);
                return Ok(());
            }
            st.history.push(Block_::User(trimmed.into()));
            st.streaming = Some(String::new());
            st.thinking = None;
            st.tool_active = None;
            stream_turn(st, backend, term, status, trimmed.into()).await?;
        }
        KeyCode::Backspace => {
            st.input.pop();
        }
        KeyCode::Char(c) => st.input.push(c),
        _ => {}
    }
    Ok(())
}

fn run_slash<B: Backend>(st: &mut UiState, backend: &mut B, rest: &str) {
    let mut split = rest.splitn(2, char::is_whitespace);
    let cmd = split.next().unwrap_or("");
    let arg = split.next().unwrap_or("");
    let full = format!("/{cmd}");
    st.history.push(Block_::User(format!("/{rest}")));
    match full.as_str() {
        "/quit" | "/exit" => {
            st.quit = true;
        }
        "/clear" => {
            st.history.clear();
        }
        "/help" => {
            st.history.push(Block_::System(slash_help()));
        }
        _ => {
            if let Some(resp) = backend.slash(&full, arg) {
                st.history.push(Block_::System(resp));
            } else {
                st.history
                    .push(Block_::Error(format!("unknown slash: /{cmd} — try /help")));
            }
        }
    }
}

fn slash_help() -> String {
    String::from(
        "slash commands\n  /help          show this\n  /quit /exit    exit\n  /clear         clear history (ctrl+l)\n  /cost          show token usage\n  /model NAME    switch model\n  /agent NAME    switch agent\n\nkeys\n  enter          send\n  shift+enter    newline\n  ctrl+c         quit\n  ctrl+l         clear\n  ctrl+u         clear input\n  page up/down   scroll history\n  f1             toggle help",
    )
}

async fn stream_turn<B: Backend>(
    st: &mut UiState,
    backend: &mut B,
    term: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    status: &Status,
    prompt: String,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel(64);
    backend.send(prompt, tx).await;
    loop {
        let recv = tokio::select! {
            ev = rx.recv() => ev,
            _ = tokio::time::sleep(std::time::Duration::from_millis(120)) => {
                st.spinner_idx = (st.spinner_idx + 1) % SPINNER.len();
                term.draw(|f: &mut Frame| draw(f, status, st))?;
                continue;
            }
        };
        let Some(ev) = recv else { break };
        match ev {
            CoreEvent::TextDelta { text } => {
                if let Some(s) = &mut st.streaming {
                    s.push_str(&text);
                }
            }
            CoreEvent::ThinkingDelta { text } => {
                st.thinking.get_or_insert_with(String::new).push_str(&text);
            }
            CoreEvent::ToolCallStart { name, .. } => {
                if let Some(active) = st.tool_active.take() {
                    st.history.push(Block_::Tool {
                        name: active,
                        body: String::new(),
                    });
                }
                st.tool_active = Some(name);
            }
            CoreEvent::ToolCallEnd { .. } => {
                if let Some(active) = st.tool_active.take() {
                    st.history.push(Block_::Tool {
                        name: active,
                        body: "(running)".into(),
                    });
                }
            }
            CoreEvent::Done(_) => break,
            CoreEvent::Error(e) => {
                st.history.push(Block_::Error(format!("{e:?}")));
                break;
            }
            _ => {}
        }
        term.draw(|f: &mut Frame| draw(f, status, st))?;
    }
    if let Some(thinking) = st.thinking.take() {
        if !thinking.trim().is_empty() {
            st.history.push(Block_::Thinking(thinking));
        }
    }
    if let Some(text) = st.streaming.take() {
        if !text.trim().is_empty() {
            st.history.push(Block_::Assistant(text));
        }
    }
    Ok(())
}

fn draw(f: &mut Frame, status: &Status, st: &UiState) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(input_height(&st.input)),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, chunks[0], status);
    draw_history(f, chunks[1], st);
    draw_input(f, chunks[2], &st.input);
    draw_footer(f, chunks[3], status, st);
}

fn input_height(input: &str) -> u16 {
    let lines = input.lines().count().max(1) as u16 + 2;
    lines.clamp(3, 10)
}

fn draw_header(f: &mut Frame, area: Rect, status: &Status) {
    let title = format!(
        " openbuild · {} · {} · {} ",
        if status.provider.is_empty() {
            "(no provider)"
        } else {
            &status.provider
        },
        if status.model.is_empty() {
            "(no model)"
        } else {
            &status.model
        },
        if status.mode.is_empty() {
            "default"
        } else {
            &status.mode
        }
    );
    let widget = Paragraph::new(Span::styled(
        title,
        Style::default()
            .bg(Color::Rgb(255, 100, 0))
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    ))
    .alignment(Alignment::Left);
    f.render_widget(widget, area);
}

fn draw_history(f: &mut Frame, area: Rect, st: &UiState) {
    let mut lines: Vec<Line> = Vec::new();
    for block in &st.history {
        render_block(block, &mut lines);
        lines.push(Line::raw(""));
    }
    if let Some(thinking) = &st.thinking {
        if !thinking.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("{} thinking…", SPINNER[st.spinner_idx]),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
            for l in thinking.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {l}"),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
            lines.push(Line::raw(""));
        }
    }
    if let Some(active) = &st.tool_active {
        lines.push(Line::from(Span::styled(
            format!("{} running tool: {active}", SPINNER[st.spinner_idx]),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::raw(""));
    }
    if let Some(streaming) = &st.streaming {
        if !streaming.is_empty() {
            lines.push(Line::from(Span::styled(
                "● assistant",
                Style::default()
                    .fg(Color::Rgb(120, 220, 255))
                    .add_modifier(Modifier::BOLD),
            )));
            render_markdown(streaming, &mut lines, Color::White);
        } else {
            lines.push(Line::from(Span::styled(
                format!("{} thinking…", SPINNER[st.spinner_idx]),
                Style::default().fg(Color::Rgb(255, 180, 0)),
            )));
        }
    }
    let total = lines.len() as u16;
    let view = area.height.saturating_sub(2);
    let max_scroll = total.saturating_sub(view);
    let scroll = max_scroll.saturating_sub(st.scroll);
    let body = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(60, 60, 60)))
                .title(" chat "),
        );
    f.render_widget(body, area);
}

fn render_block(block: &Block_, lines: &mut Vec<Line>) {
    match block {
        Block_::User(text) => {
            lines.push(Line::from(Span::styled(
                "▍ you",
                Style::default()
                    .fg(Color::Rgb(120, 220, 255))
                    .add_modifier(Modifier::BOLD),
            )));
            for l in text.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {l}"),
                    Style::default().fg(Color::Rgb(180, 220, 255)),
                )));
            }
        }
        Block_::Assistant(text) => {
            lines.push(Line::from(Span::styled(
                "● assistant",
                Style::default()
                    .fg(Color::Rgb(255, 200, 100))
                    .add_modifier(Modifier::BOLD),
            )));
            render_markdown(text, lines, Color::White);
        }
        Block_::Thinking(text) => {
            lines.push(Line::from(Span::styled(
                "◐ thinking",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )));
            for l in text.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {l}"),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
        }
        Block_::Tool { name, body } => {
            lines.push(Line::from(Span::styled(
                format!("⏵ {name}"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            for l in body.lines().take(8) {
                lines.push(Line::from(Span::styled(
                    format!("  {l}"),
                    Style::default().fg(Color::Rgb(180, 180, 100)),
                )));
            }
            if body.lines().count() > 8 {
                lines.push(Line::from(Span::styled(
                    "  …",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        Block_::Error(text) => {
            lines.push(Line::from(Span::styled(
                "✗ error",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
            for l in text.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {l}"),
                    Style::default().fg(Color::LightRed),
                )));
            }
        }
        Block_::System(text) => {
            for l in text.lines() {
                lines.push(Line::from(Span::styled(
                    l.to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }
}

fn render_markdown(text: &str, lines: &mut Vec<Line>, base: Color) {
    let mut in_code = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(Color::Rgb(120, 120, 120)),
            )));
            continue;
        }
        if in_code {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(Color::Rgb(180, 220, 180)),
            )));
            continue;
        }
        if trimmed.starts_with("# ") || trimmed.starts_with("## ") {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(base).add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("- ") {
            lines.push(Line::from(vec![
                Span::styled("  • ", Style::default().fg(Color::Rgb(255, 180, 0))),
                Span::styled(rest.to_string(), Style::default().fg(base)),
            ]));
            continue;
        }
        lines.push(Line::from(Span::styled(
            format!("  {line}"),
            Style::default().fg(base),
        )));
    }
}

fn draw_input(f: &mut Frame, area: Rect, input: &str) {
    let widget = Paragraph::new(input.to_string())
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(255, 100, 0)))
                .title(Span::styled(
                    " > ",
                    Style::default()
                        .fg(Color::Rgb(255, 100, 0))
                        .add_modifier(Modifier::BOLD),
                )),
        );
    f.render_widget(widget, area);
}

fn draw_footer(f: &mut Frame, area: Rect, status: &Status, st: &UiState) {
    let busy = st.streaming.is_some() || st.thinking.is_some() || st.tool_active.is_some();
    let left = if busy {
        format!("{} working", SPINNER[st.spinner_idx])
    } else {
        "idle".to_string()
    };
    let right = format!(
        "in {} · out {} · turn {} · /help · ctrl+c quit",
        status.input_tokens, status.output_tokens, status.turn
    );
    let bar = format!(
        " {left}{:>pad$}{right} ",
        "",
        pad = pad_width(&left, &right, area.width)
    );
    let widget = Paragraph::new(Span::styled(
        bar,
        Style::default()
            .fg(Color::DarkGray)
            .bg(Color::Rgb(20, 20, 20)),
    ));
    f.render_widget(widget, area);
}

fn pad_width(left: &str, right: &str, total: u16) -> usize {
    use unicode_width::UnicodeWidthStr;
    let used = left.width() + right.width() + 2;
    (total as usize).saturating_sub(used)
}
