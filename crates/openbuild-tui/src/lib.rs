use anyhow::Result;
use async_trait::async_trait;
use crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use openbuild_core::Event as CoreEvent;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::io::stdout;
use tokio::sync::mpsc;

#[async_trait]
pub trait Backend: Send {
    async fn send(&mut self, prompt: String, out: mpsc::Sender<CoreEvent>);
    fn slash(&mut self, cmd: &str, _arg: &str) -> Option<String> {
        match cmd {
            "/help" => Some("/quit  /help  /cost  /clear  /agent NAME  /model NAME".into()),
            _ => None,
        }
    }
    fn header(&self) -> String {
        "openbuild".into()
    }
}

pub async fn run_streaming<B: Backend>(backend: &mut B, alt_screen: bool) -> Result<()> {
    enable_raw_mode()?;
    if alt_screen {
        execute!(stdout(), EnterAlternateScreen)?;
    }
    let result = run_loop(backend, alt_screen).await;
    if alt_screen {
        execute!(stdout(), LeaveAlternateScreen).ok();
    }
    disable_raw_mode().ok();
    result
}

async fn run_loop<B: Backend>(backend: &mut B, _alt: bool) -> Result<()> {
    let backend_term = ratatui::backend::CrosstermBackend::new(stdout());
    let mut term = Terminal::new(backend_term)?;
    let mut history: Vec<String> = Vec::new();
    let mut input = String::new();
    let mut streaming: Option<String> = None;
    let mut quit = false;

    while !quit {
        let header = backend.header();
        term.draw(|f: &mut Frame| draw(f, &header, &history, streaming.as_deref(), &input))?;

        if event::poll(std::time::Duration::from_millis(80))? {
            if let event::Event::Key(KeyEvent {
                code,
                kind: KeyEventKind::Press,
                ..
            }) = event::read()?
            {
                match code {
                    KeyCode::Esc => quit = true,
                    KeyCode::Enter => {
                        let line = std::mem::take(&mut input);
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if let Some(rest) = trimmed.strip_prefix('/') {
                            let mut split = rest.splitn(2, char::is_whitespace);
                            let cmd = split.next().unwrap_or("");
                            let arg = split.next().unwrap_or("");
                            let full_cmd = format!("/{cmd}");
                            history.push(format!("> {trimmed}"));
                            if full_cmd == "/quit" || full_cmd == "/exit" {
                                quit = true;
                                continue;
                            }
                            if full_cmd == "/clear" {
                                history.clear();
                                continue;
                            }
                            if let Some(resp) = backend.slash(&full_cmd, arg) {
                                history.push(resp);
                            } else {
                                history.push(format!("(unknown slash: /{cmd})"));
                            }
                            continue;
                        }
                        history.push(format!("> {trimmed}"));
                        streaming = Some(String::new());
                        let (tx, mut rx) = mpsc::channel(64);
                        backend.send(trimmed.to_string(), tx).await;
                        while let Some(ev) = rx.recv().await {
                            match ev {
                                CoreEvent::TextDelta { text } => {
                                    if let Some(s) = &mut streaming {
                                        s.push_str(&text);
                                    }
                                }
                                CoreEvent::ToolCallStart { name, .. } => {
                                    if let Some(s) = &mut streaming {
                                        s.push_str(&format!("\n[{name}]"));
                                    }
                                }
                                CoreEvent::Done(_) => break,
                                CoreEvent::Error(e) => {
                                    if let Some(s) = &mut streaming {
                                        s.push_str(&format!("\n[error] {e:?}"));
                                    }
                                    break;
                                }
                                _ => {}
                            }
                            term.draw(|f: &mut Frame| {
                                draw(f, &header, &history, streaming.as_deref(), &input)
                            })?;
                        }
                        if let Some(s) = streaming.take() {
                            history.push(s);
                        }
                    }
                    KeyCode::Backspace => {
                        input.pop();
                    }
                    KeyCode::Char(c) => input.push(c),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn draw(f: &mut Frame, header: &str, history: &[String], streaming: Option<&str>, input: &str) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(area);

    let header_p = Paragraph::new(Span::styled(
        header,
        Style::default().add_modifier(Modifier::BOLD),
    ));
    f.render_widget(header_p, chunks[0]);

    let mut lines: Vec<Line> = history.iter().map(|h| Line::from(h.as_str())).collect();
    if let Some(s) = streaming {
        lines.push(Line::from(Span::styled(
            s,
            Style::default().fg(Color::Cyan),
        )));
    }
    let body = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title("history"));
    f.render_widget(body, chunks[1]);

    let prompt = Paragraph::new(input).block(
        Block::default()
            .borders(Borders::ALL)
            .title("input (esc to quit, /help)"),
    );
    f.render_widget(prompt, chunks[2]);
}
