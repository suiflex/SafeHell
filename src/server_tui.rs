use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use zeroize::Zeroizing;

use crate::{AuthArg, ServerInput, ServerScope, add_server, config, remove_server};

const ACCENT: Color = Color::Rgb(74, 222, 128);
const MUTED: Color = Color::Rgb(148, 163, 184);
const PANEL: Color = Color::Rgb(30, 41, 59);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Global,
    Project,
}

impl Scope {
    fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }

    fn toggle(self) -> Self {
        match self {
            Self::Global => Self::Project,
            Self::Project => Self::Global,
        }
    }
}

#[derive(Clone)]
struct ServerEntry {
    alias: String,
    server: config::Server,
    scope: Scope,
}

#[derive(Clone)]
struct AddForm {
    scope: Scope,
    alias: String,
    host: String,
    port: String,
    username: String,
    auth: AuthArg,
    password: Zeroizing<String>,
}

impl Default for AddForm {
    fn default() -> Self {
        Self {
            scope: Scope::Project,
            alias: String::new(),
            host: String::new(),
            port: "22".to_owned(),
            username: String::new(),
            auth: AuthArg::Password,
            password: Zeroizing::new(String::new()),
        }
    }
}

impl AddForm {
    fn field_count(&self) -> usize {
        if matches!(self.auth, AuthArg::Password) {
            7
        } else {
            6
        }
    }

    fn value_mut(&mut self, field: usize) -> Option<&mut String> {
        match field {
            1 => Some(&mut self.alias),
            2 => Some(&mut self.host),
            3 => Some(&mut self.port),
            4 => Some(&mut self.username),
            6 if matches!(self.auth, AuthArg::Password) => Some(&mut self.password),
            _ => None,
        }
    }

    fn field_label(&self, field: usize) -> &'static str {
        match field {
            0 => "Scope",
            1 => "Alias",
            2 => "Host",
            3 => "Port",
            4 => "Username",
            5 => "Authentication",
            6 => "Password",
            _ => "",
        }
    }

    fn field_value(&self, field: usize) -> String {
        match field {
            0 => self.scope.label().to_owned(),
            1 => self.alias.clone(),
            2 => self.host.clone(),
            3 => self.port.clone(),
            4 => self.username.clone(),
            5 => match self.auth {
                AuthArg::Password => "password".to_owned(),
                AuthArg::SshAgent => "ssh-agent".to_owned(),
            },
            6 => "*".repeat(self.password.len().min(32)),
            _ => String::new(),
        }
    }
}

enum Screen {
    List,
    Add { form: AddForm, focus: usize },
    ConfirmDelete { index: usize },
}

enum Action {
    None,
    Quit,
    OpenAdd,
    CancelAdd,
    CancelDelete,
    ConfirmDelete(usize),
    Save(AddForm, usize),
    Delete(usize),
}

struct App {
    current: PathBuf,
    project_label: String,
    servers: Vec<ServerEntry>,
    selected: usize,
    screen: Screen,
    status: Option<String>,
}

impl App {
    fn new(current: &Path) -> Result<Self> {
        let mut app = Self {
            current: current.to_owned(),
            project_label: "No project selected".to_owned(),
            servers: Vec::new(),
            selected: 0,
            screen: Screen::List,
            status: None,
        };
        app.reload()?;
        Ok(app)
    }

    fn reload(&mut self) -> Result<()> {
        let global = config::load_global()?;
        let project = match config::discover(&self.current) {
            Ok(project) => Some(project),
            Err(error)
                if self
                    .current
                    .ancestors()
                    .any(|directory| directory.join(config::CONFIG_NAME).is_file()) =>
            {
                return Err(error);
            }
            Err(_) => None,
        };
        self.servers = global
            .servers
            .into_iter()
            .map(|(alias, server)| ServerEntry {
                alias,
                server,
                scope: Scope::Global,
            })
            .chain(project.into_iter().flat_map(|project| {
                project
                    .config
                    .servers
                    .into_iter()
                    .map(|(alias, server)| ServerEntry {
                        alias,
                        server,
                        scope: Scope::Project,
                    })
            }))
            .collect();
        self.selected = self.selected.min(self.servers.len().saturating_sub(1));
        Ok(())
    }

    fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        loop {
            terminal.draw(|frame| self.draw(frame))?;
            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
            {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if self.handle_key(key)? {
                    return Ok(());
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        let action = if matches!(&self.screen, Screen::List) {
            self.handle_list_key(key)
        } else if matches!(&self.screen, Screen::Add { .. }) {
            self.handle_add_key(key)
        } else {
            self.handle_confirm_key(key)
        };
        self.apply_action(action)
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.servers.is_empty() {
                    self.selected = (self.selected + 1).min(self.servers.len() - 1);
                }
                Action::None
            }
            KeyCode::Char('a') => Action::OpenAdd,
            KeyCode::Char('d') if !self.servers.is_empty() => Action::ConfirmDelete(self.selected),
            _ => Action::None,
        }
    }

    fn handle_add_key(&mut self, key: KeyEvent) -> Action {
        let Screen::Add { form, focus } = &mut self.screen else {
            unreachable!("add key handler called outside add screen");
        };
        if key.code == KeyCode::Esc {
            return Action::CancelAdd;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            return Action::Save(form.clone(), *focus);
        }

        match key.code {
            KeyCode::Tab | KeyCode::Enter => {
                if *focus + 1 < form.field_count() {
                    *focus += 1;
                    Action::None
                } else {
                    Action::Save(form.clone(), *focus)
                }
            }
            KeyCode::Backspace => {
                if let Some(value) = form.value_mut(*focus) {
                    value.pop();
                }
                Action::None
            }
            KeyCode::Left | KeyCode::Char('h') if *focus == 0 => {
                form.scope = form.scope.toggle();
                Action::None
            }
            KeyCode::Right | KeyCode::Char('l') if *focus == 0 => {
                form.scope = form.scope.toggle();
                Action::None
            }
            KeyCode::Left | KeyCode::Char('h') if *focus == 5 => {
                form.auth = AuthArg::Password;
                *focus = (*focus).min(form.field_count() - 1);
                Action::None
            }
            KeyCode::Right | KeyCode::Char('l') if *focus == 5 => {
                form.auth = AuthArg::SshAgent;
                *focus = (*focus).min(form.field_count() - 1);
                Action::None
            }
            KeyCode::Char(character) => {
                if let Some(value) = form.value_mut(*focus) {
                    value.push(character);
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) -> Action {
        let Screen::ConfirmDelete { index } = &self.screen else {
            unreachable!("confirm key handler called outside confirm screen");
        };
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Action::Delete(*index),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Action::CancelDelete,
            _ => Action::None,
        }
    }

    fn apply_action(&mut self, action: Action) -> Result<bool> {
        match action {
            Action::None => Ok(false),
            Action::Quit => Ok(true),
            Action::OpenAdd => {
                self.status = None;
                self.screen = Screen::Add {
                    form: AddForm::default(),
                    focus: 0,
                };
                Ok(false)
            }
            Action::CancelAdd => {
                self.screen = Screen::List;
                Ok(false)
            }
            Action::CancelDelete => {
                self.status = Some("Deletion cancelled.".to_owned());
                self.screen = Screen::List;
                Ok(false)
            }
            Action::ConfirmDelete(index) => {
                self.screen = Screen::ConfirmDelete { index };
                Ok(false)
            }
            Action::Delete(index) => {
                if let Some(entry) = self.servers.get(index).cloned() {
                    match remove_server(&self.current, &entry.alias, to_server_scope(entry.scope)) {
                        Ok(()) => {
                            self.reload()?;
                            self.status =
                                Some(format!("Deleted {}/{}.", entry.scope.label(), entry.alias));
                        }
                        Err(error) => {
                            self.status = Some(format!("Delete failed: {error:#}"));
                        }
                    }
                }
                self.screen = Screen::List;
                Ok(false)
            }
            Action::Save(form, focus) => {
                let alias = form.alias.clone();
                let scope = form.scope;
                match self.save_form(&form) {
                    Ok(()) => {
                        self.reload()?;
                        self.status = Some(format!("Added {}/{}.", scope.label(), alias));
                        self.screen = Screen::List;
                    }
                    Err(error) => {
                        self.status = Some(format!("Add failed: {error:#}"));
                        self.screen = Screen::Add { form, focus };
                    }
                }
                Ok(false)
            }
        }
    }

    fn save_form(&self, form: &AddForm) -> Result<()> {
        let port = form
            .port
            .parse::<u16>()
            .context("port must be a number between 1 and 65535")?;
        let password = if matches!(form.auth, AuthArg::Password) {
            Some(form.password.clone())
        } else {
            None
        };
        add_server(
            &self.current,
            ServerInput {
                alias: form.alias.clone(),
                host: form.host.clone(),
                port,
                username: form.username.clone(),
                auth: form.auth,
                password,
                scope: to_server_scope(form.scope),
            },
        )
    }

    fn draw(&self, frame: &mut Frame<'_>) {
        match &self.screen {
            Screen::List => self.draw_list(frame),
            Screen::Add { form, focus } => self.draw_add(frame, form, *focus),
            Screen::ConfirmDelete { index } => self.draw_confirm(frame, *index),
        }
    }

    fn draw_list(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(2),
            ])
            .split(area);
        let header = Paragraph::new(Line::from(vec![
            Span::styled(
                "Safe",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Hell", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  /  Server Manager  /  project: {}", self.project_label),
                Style::default().fg(MUTED),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(PANEL)),
        );
        frame.render_widget(header, layout[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
            .split(layout[1]);
        let items = if self.servers.is_empty() {
            vec![ListItem::new(Span::styled(
                "No servers configured",
                Style::default().fg(MUTED),
            ))]
        } else {
            self.servers
                .iter()
                .map(|entry| {
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("[{}] ", entry.scope.label()),
                            Style::default().fg(MUTED),
                        ),
                        Span::styled(&entry.alias, Style::default().fg(ACCENT)),
                        Span::styled(
                            format!(
                                "  {}@{}:{}",
                                entry.server.username, entry.server.host, entry.server.port
                            ),
                            Style::default().fg(MUTED),
                        ),
                    ]))
                })
                .collect()
        };
        frame.render_widget(
            List::new(items).block(Block::default().title(" Servers ").borders(Borders::ALL)),
            body[0],
        );
        if !self.servers.is_empty() {
            let selected = Rect {
                x: body[0].x + 1,
                y: body[0].y + 1 + self.selected as u16,
                width: body[0].width.saturating_sub(2),
                height: 1,
            };
            frame.render_widget(Block::default().style(Style::default().bg(PANEL)), selected);
            let entry = &self.servers[self.selected];
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        format!("[{}] ", entry.scope.label()),
                        Style::default().fg(MUTED),
                    ),
                    Span::styled(
                        &entry.alias,
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            "  {}@{}:{}",
                            entry.server.username, entry.server.host, entry.server.port
                        ),
                        Style::default().fg(Color::White),
                    ),
                ])),
                selected,
            );
        }

        let detail = if let Some(entry) = self.servers.get(self.selected) {
            vec![
                Line::from(Span::styled(
                    format!("{}/{}", entry.scope.label(), entry.alias),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(format!(
                    "Host           {}:{}",
                    entry.server.host, entry.server.port
                )),
                Line::from(format!("Username       {}", entry.server.username)),
                Line::from(format!("Authentication  {}", entry.server.auth.label())),
                Line::from(format!(
                    "Auto-approve   {} allow / {} deny rules",
                    entry.server.autoapprove.allow.len(),
                    entry.server.autoapprove.deny.len()
                )),
            ]
        } else {
            vec![Line::from(Span::styled(
                "Use 'a' to add a server.",
                Style::default().fg(MUTED),
            ))]
        };
        frame.render_widget(
            Paragraph::new(detail)
                .block(Block::default().title(" Details ").borders(Borders::ALL))
                .wrap(Wrap { trim: true }),
            body[1],
        );
        self.draw_footer(
            frame,
            layout[2],
            "↑↓/j k select   a add   d delete   q quit",
        );
    }

    fn draw_add(&self, frame: &mut Frame<'_>, form: &AddForm, focus: usize) {
        self.draw_list(frame);
        let popup = centered(frame.area(), 72, 18);
        frame.render_widget(Clear, popup);
        let rows = (0..form.field_count())
            .map(|field| {
                let style = if field == focus {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                Line::from(vec![
                    Span::styled(format!("{:16}", form.field_label(field)), style),
                    Span::styled(form.field_value(field), Style::default().fg(Color::White)),
                    if field == focus {
                        Span::styled("  ◀", Style::default().fg(ACCENT))
                    } else {
                        Span::raw("")
                    },
                ])
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(rows)
                .block(
                    Block::default()
                        .title(" Add Server ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(ACCENT)),
                )
                .style(Style::default().bg(Color::Rgb(15, 23, 42))),
            popup,
        );
        let area = frame.area();
        self.draw_footer(
            frame,
            Rect {
                x: area.x,
                y: area.y + area.height.saturating_sub(1),
                width: area.width,
                height: 1,
            },
            "Tab/Enter next   Ctrl-S save   ←→ scope/auth   Esc cancel",
        );
    }

    fn draw_confirm(&self, frame: &mut Frame<'_>, index: usize) {
        self.draw_list(frame);
        let popup = centered(frame.area(), 62, 7);
        frame.render_widget(Clear, popup);
        let message = self
            .servers
            .get(index)
            .map(|entry| format!("Delete {}/{}?", entry.scope.label(), entry.alias))
            .unwrap_or_else(|| "Delete selected server?".to_owned());
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    message,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Enter/y confirm   n/Esc cancel",
                    Style::default().fg(MUTED),
                )),
            ])
            .block(
                Block::default()
                    .title(" Confirm Delete ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .style(Style::default().bg(Color::Rgb(15, 23, 42))),
            popup,
        );
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect, help: &str) {
        frame.render_widget(
            Paragraph::new(self.status.as_deref().unwrap_or(help))
                .style(Style::default().fg(MUTED)),
            area,
        );
    }
}

fn to_server_scope(scope: Scope) -> ServerScope {
    match scope {
        Scope::Global => ServerScope::Global,
        Scope::Project => ServerScope::Project,
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(area.width.saturating_sub(width) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(area);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(area.height.saturating_sub(height) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(horizontal[1]);
    vertical[1]
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, cursor::Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error.into());
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
        let _ = stdout.flush();
    }
}

pub(crate) fn run(current: &Path) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new(current)?;
    app.run(&mut terminal)
}
