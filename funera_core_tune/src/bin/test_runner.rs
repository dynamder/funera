use std::time::Instant;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use tokio::runtime::Runtime;

// ---- Scenario registry ----

struct Scenario {
    name: &'static str,
    description: &'static str,
    run: fn() -> Result<String>,
}

static SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "multi_turn_conversation",
        description: "Multi-turn conversation with ReActLoop and tool registration",
        run: || {
            Runtime::new()?.block_on(
                funera_core_tune::scenarios::complex_conversation::multi_turn_conversation(),
            )
        },
    },
    Scenario {
        name: "session_state_transitions",
        description: "FuneraSession Idle->Running->Idle state machine transitions",
        run: || {
            Runtime::new()?.block_on(
                funera_core_tune::scenarios::complex_conversation::session_state_transitions(),
            )
        },
    },
    Scenario {
        name: "multiple_tool_calls",
        description: "Create multiple tools and execute them via ToolExecutor",
        run: || {
            Runtime::new()?.block_on(funera_core_tune::scenarios::tool_chain::multiple_tool_calls())
        },
    },
    Scenario {
        name: "tool_registry_operations",
        description: "ToolRegistry add / remove / query / available_tools_json",
        run: || {
            Runtime::new()?
                .block_on(funera_core_tune::scenarios::tool_chain::tool_registry_operations())
        },
    },
    Scenario {
        name: "tool_execution_error",
        description: "Tool that returns ToolExecutionError during ReActLoop run",
        run: || {
            Runtime::new()?
                .block_on(funera_core_tune::scenarios::error_recovery::tool_execution_error())
        },
    },
    Scenario {
        name: "tool_not_found_error",
        description: "ReActLoop with empty registry - tool not found path",
        run: || {
            Runtime::new()?
                .block_on(funera_core_tune::scenarios::error_recovery::tool_not_found_error())
        },
    },
    Scenario {
        name: "env_watcher_tracks_changes",
        description: "FuneraEnvWatcher has_changed / watch_tool / set_model",
        run: || {
            Runtime::new()?
                .block_on(funera_core_tune::scenarios::error_recovery::env_watcher_tracks_changes())
        },
    },
];

// ---- TUI state ----

enum TestStatus {
    Pending,
    Running,
    Passed(String),
    Failed(String),
}

struct App {
    list_state: ListState,
    statuses: Vec<TestStatus>,
    log: Vec<String>,
    should_quit: bool,
}

impl App {
    fn new() -> Self {
        let count = SCENARIOS.len();
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            list_state,
            statuses: (0..count).map(|_| TestStatus::Pending).collect(),
            log: vec!["Test Runner started. Use \u{2191}/\u{2193} to navigate, Enter to run, 'a' to run all, 'q' to quit.".into()],
            should_quit: false,
        }
    }

    fn next(&mut self) {
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state
            .select(Some((i + 1).min(SCENARIOS.len() - 1)));
    }

    fn prev(&mut self) {
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(i.saturating_sub(1)));
    }

    fn selected_index(&self) -> usize {
        self.list_state.selected().unwrap_or(0)
    }

    fn run_selected(&mut self) {
        self.run_scenario(self.selected_index());
    }

    fn run_all(&mut self) {
        for idx in 0..SCENARIOS.len() {
            self.run_scenario(idx);
        }
    }

    fn run_scenario(&mut self, idx: usize) {
        let scenario = &SCENARIOS[idx];
        if matches!(self.statuses[idx], TestStatus::Running) {
            return;
        }
        self.statuses[idx] = TestStatus::Running;
        self.log
            .push(format!("\u{25b6} Running [{}] {} ...", idx, scenario.name));
        let start = Instant::now();

        match (scenario.run)() {
            Ok(msg) => {
                let elapsed = start.elapsed();
                self.statuses[idx] = TestStatus::Passed(msg.clone());
                self.log.push(format!(
                    "  \u{2713} [{}] {} \u{2014} {:?}",
                    idx, scenario.name, elapsed
                ));
                self.log.push(format!("    result: {}", msg));
            }
            Err(e) => {
                let elapsed = start.elapsed();
                self.statuses[idx] = TestStatus::Failed(format!("{:#}", e));
                self.log.push(format!(
                    "  \u{2717} [{}] {} \u{2014} {:?}",
                    idx, scenario.name, elapsed
                ));
                self.log.push(format!("    error: {:#}", e));
            }
        }
    }
}

// ---- UI rendering ----

fn ui(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(frame.area());

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[1]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(0)])
        .split(bottom[1]);

    render_title(frame, chunks[0]);
    render_scenario_list(frame, bottom[0], app);
    render_description(frame, right[0], app);
    render_log(frame, right[1], app);
}

fn render_title(frame: &mut Frame, area: Rect) {
    let title = Paragraph::new("funera_core_tune \u{2014} Test Runner")
        .style(Style::new().bold().fg(Color::Cyan))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Controls ")
                .title_alignment(ratatui::layout::Alignment::Center),
        );
    frame.render_widget(title, area);
}

fn render_scenario_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let items: Vec<ListItem> = SCENARIOS
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let status = &app.statuses[i];
            let (prefix, style) = match status {
                TestStatus::Pending => (" \u{25cb}", Style::default().fg(Color::White)),
                TestStatus::Running => (
                    " \u{25c9}",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                TestStatus::Passed(_) => (" \u{2713}", Style::default().fg(Color::Green)),
                TestStatus::Failed(_) => (" \u{2717}", Style::default().fg(Color::Red)),
            };
            let content = Line::from(Span::styled(format!("{}  {}", prefix, s.name), style));
            ListItem::new(content)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Scenarios ")
                .title_alignment(ratatui::layout::Alignment::Center),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut app.list_state);
}

fn render_description(frame: &mut Frame, area: Rect, app: &mut App) {
    let idx = app.selected_index();
    let s = &SCENARIOS[idx];
    let status = &app.statuses[idx];

    let status_line = match status {
        TestStatus::Pending => Line::from(Span::styled(
            "Status: Pending",
            Style::default().fg(Color::White),
        )),
        TestStatus::Running => Line::from(Span::styled(
            "Status: Running ...",
            Style::default().fg(Color::Yellow),
        )),
        TestStatus::Passed(msg) => Line::from(Span::styled(
            format!("Status: Passed \u{2014} {}", msg),
            Style::default().fg(Color::Green),
        )),
        TestStatus::Failed(e) => {
            let short = e.lines().next().unwrap_or(e);
            Line::from(Span::styled(
                format!("Status: Failed \u{2014} {}", short),
                Style::default().fg(Color::Red),
            ))
        }
    };

    let text = Text::from(vec![
        Line::from(Span::styled(
            format!("Scenario: {}", s.name),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            s.description,
            Style::default().fg(Color::Gray),
        )),
        status_line,
    ]);

    let p = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Details ")
            .title_alignment(ratatui::layout::Alignment::Center),
    );
    frame.render_widget(p, area);
}

fn render_log(frame: &mut Frame, area: Rect, app: &mut App) {
    let log_text: Vec<Line> = app
        .log
        .iter()
        .rev()
        .take(100)
        .map(|l| {
            let style = if l.starts_with("  \u{2713}") {
                Style::default().fg(Color::Green)
            } else if l.starts_with("  \u{2717}") {
                Style::default().fg(Color::Red)
            } else if l.starts_with("\u{25b6}") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };
            Line::from(Span::styled(l.clone(), style))
        })
        .collect();

    let paragraph = Paragraph::new(Text::from(log_text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Log ")
                .title_alignment(ratatui::layout::Alignment::Center),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

// ---- Main ----

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;

    let mut app = App::new();
    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(e) = res {
        eprintln!("Error: {:#}", e);
    }

    println!("\n=== Test Summary ===");
    for (i, s) in SCENARIOS.iter().enumerate() {
        let status = &app.statuses[i];
        match status {
            TestStatus::Passed(_) => println!("  \u{2713} {} \u{2014} PASSED", s.name),
            TestStatus::Failed(e) => println!("  \u{2717} {} \u{2014} FAILED\n     {}", s.name, e),
            TestStatus::Pending => println!("  \u{25cb} {} \u{2014} PENDING", s.name),
            TestStatus::Running => println!("  \u{25c9} {} \u{2014} RUNNING", s.name),
        }
    }

    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()>
where
    <B as ratatui::backend::Backend>::Error: std::error::Error + Send + Sync + 'static,
{
    while !app.should_quit {
        terminal.draw(|f| ui(f, app))?;

        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') => app.should_quit = true,
                    KeyCode::Up => app.prev(),
                    KeyCode::Down => app.next(),
                    KeyCode::Enter => app.run_selected(),
                    KeyCode::Char('a') => app.run_all(),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
