use anyhow::Result;
use crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::io::stdout;

pub trait LineHandler {
    fn handle(&mut self, line: &str) -> Vec<String>;
    fn header(&self) -> String {
        "openbuild".into()
    }
}

pub fn run<H: LineHandler>(handler: &mut H, alt_screen: bool) -> Result<()> {
    enable_raw_mode()?;
    if alt_screen {
        execute!(stdout(), EnterAlternateScreen)?;
    }
    let backend = ratatui::backend::CrosstermBackend::new(stdout());
    let mut term = Terminal::new(backend)?;

    let mut history: Vec<String> = Vec::new();
    let mut input = String::new();
    let mut quit = false;

    while !quit {
        term.draw(|f: &mut Frame| draw(f, &history, &input, handler))?;
        if event::poll(std::time::Duration::from_millis(250))? {
            if let event::Event::Key(KeyEvent {
                code,
                kind: KeyEventKind::Press,
                ..
            }) = event::read()?
            {
                match code {
                    KeyCode::Esc => quit = true,
                    KeyCode::Char('c')
                        if event::KeyModifiers::CONTROL.bits()
                            & crossterm::event::KeyModifiers::CONTROL.bits()
                            != 0 =>
                    {
                        quit = true;
                    }
                    KeyCode::Enter => {
                        let line = std::mem::take(&mut input);
                        let trimmed = line.trim();
                        if trimmed == "/quit" || trimmed == "/exit" {
                            quit = true;
                        } else if !trimmed.is_empty() {
                            history.push(format!("> {trimmed}"));
                            let responses = handler.handle(trimmed);
                            history.extend(responses);
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

    if alt_screen {
        execute!(stdout(), LeaveAlternateScreen).ok();
    }
    disable_raw_mode().ok();
    Ok(())
}

fn draw<H: LineHandler>(f: &mut Frame, history: &[String], input: &str, handler: &H) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(area);

    let header = Paragraph::new(Span::styled(
        handler.header(),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    f.render_widget(header, chunks[0]);

    let lines: Vec<Line> = history.iter().map(|h| Line::from(h.as_str())).collect();
    let body = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title("history"));
    f.render_widget(body, chunks[1]);

    let prompt = Paragraph::new(input).block(
        Block::default()
            .borders(Borders::ALL)
            .title("input (esc to quit, /quit, /help)"),
    );
    f.render_widget(prompt, chunks[2]);
}
