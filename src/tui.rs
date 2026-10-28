//! Inline fuzzy picker: draws a few rows under the cursor on the controlling
//! tty (no alternate screen), so stdout stays free for the selection.

use std::cmp::Reverse;
use std::fs::{File, OpenOptions};
use std::io::Write;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use eyre::Result;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

use crate::entries::Entry;
use crate::fuzzy;

const MAX_ROWS: usize = 10;
const MAX_LABEL: usize = 30;

pub fn open_tty() -> Option<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()
}

/// Which entries show: everything, only project roots, or only worktrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    All,
    Projects,
    Worktrees,
}

impl Kind {
    fn next(self) -> Self {
        match self {
            Kind::All => Kind::Projects,
            Kind::Projects => Kind::Worktrees,
            Kind::Worktrees => Kind::All,
        }
    }

    fn shows(self, entry: &Entry) -> bool {
        match self {
            Kind::All => true,
            Kind::Projects => entry.worktree.is_none(),
            Kind::Worktrees => entry.worktree.is_some(),
        }
    }
}

/// Single-line text field; `cursor` is a byte offset on a char boundary.
#[derive(Debug, Default, Clone)]
struct TextInput {
    text: String,
    cursor: usize,
}

impl TextInput {
    fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            cursor: text.len(),
        }
    }

    fn prev(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |c| self.cursor + c.len_utf8())
    }

    fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn backspace(&mut self) {
        let at = self.prev();
        self.text.replace_range(at..self.cursor, "");
        self.cursor = at;
    }

    fn delete(&mut self) {
        let to = self.next();
        self.text.replace_range(self.cursor..to, "");
    }

    fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        let at = trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        self.text.replace_range(at..self.cursor, "");
        self.cursor = at;
    }

    fn clear_before(&mut self) {
        self.text.replace_range(..self.cursor, "");
        self.cursor = 0;
    }

    /// Returns true when the text changed.
    fn handle(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Left => self.cursor = self.prev(),
            KeyCode::Right => self.cursor = self.next(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Char('u') if ctrl => self.clear_before(),
            KeyCode::Char(c) if !ctrl => self.insert(c),
            _ => return false,
        }
        matches!(
            key.code,
            KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_)
        )
    }
}

#[derive(Debug)]
pub enum Action {
    None,
    Cancel,
    Select(Box<Entry>),
    SaveTags { project: String, tags: Vec<String> },
}

struct Hit {
    index: usize,
    positions: Vec<usize>,
}

struct Picker {
    entries: Vec<Entry>,
    input: TextInput,
    tag: Option<String>,
    kind: Kind,
    hits: Vec<Hit>,
    selected: usize,
    /// Tag editor for the highlighted project: (project, field).
    edit: Option<(String, TextInput)>,
    /// Modal keys: start in normal mode, `/` `i` `a` to search, esc to leave search.
    vim: bool,
    insert: bool,
}

impl Picker {
    fn new(entries: Vec<Entry>, query: &str, vim: bool) -> Self {
        let mut picker = Self {
            entries,
            input: TextInput::new(query),
            tag: None,
            kind: Kind::All,
            hits: Vec::new(),
            selected: 0,
            edit: None,
            vim,
            insert: !vim,
        };
        picker.refilter();
        picker
    }

    fn tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = self.entries.iter().flat_map(|e| e.tags.clone()).collect();
        tags.sort();
        tags.dedup();
        tags
    }

    fn has_worktrees(&self) -> bool {
        self.entries.iter().any(|e| e.worktree.is_some())
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.hits.get(self.selected).map(|h| &self.entries[h.index])
    }

    fn refilter(&mut self) {
        let query = self.input.text.trim();
        let mut hits = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            let tag_ok = self.tag.as_ref().is_none_or(|t| entry.tags.contains(t));
            if !tag_ok || !self.kind.shows(entry) {
                continue;
            }
            if query.is_empty() {
                hits.push((
                    0,
                    Hit {
                        index,
                        positions: Vec::new(),
                    },
                ));
            } else if let Some(m) = fuzzy::find(query, &entry.search_text()) {
                hits.push((
                    m.score,
                    Hit {
                        index,
                        positions: m.positions,
                    },
                ));
            }
        }
        if !query.is_empty() {
            // Stable: equal scores keep the frecency order.
            hits.sort_by_key(|(score, _)| Reverse(*score));
        }
        self.hits = hits.into_iter().map(|(_, h)| h).collect();
        self.selected = 0;
    }

    fn cycle_tag(&mut self, forward: bool) {
        let tags = self.tags();
        let at = self
            .tag
            .as_ref()
            .and_then(|t| tags.iter().position(|x| x == t));
        // Slots: 0 = no filter, 1..=n = tags.
        let slots = tags.len() + 1;
        let cur = at.map_or(0, |i| i + 1);
        let next = if forward {
            (cur + 1) % slots
        } else {
            (cur + slots - 1) % slots
        };
        self.tag = next.checked_sub(1).map(|i| tags[i].clone());
        self.refilter();
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.hits.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    fn cycle_kind(&mut self) {
        if self.has_worktrees() {
            self.kind = self.kind.next();
            self.refilter();
        }
    }

    fn begin_edit(&mut self) {
        if let Some(entry) = self.selected_entry() {
            let text = entry.tags.join(", ");
            self.edit = Some((entry.project.clone(), TextInput::new(&text)));
        }
    }

    /// Normal-mode keys. `None` means "not mine": the shared bindings handle it.
    fn handle_normal(&mut self, key: KeyEvent) -> Option<Action> {
        let KeyCode::Char(c) = key.code else {
            return None;
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return None;
        }
        match c {
            'j' => self.move_by(1),
            'k' => self.move_by(-1),
            'g' => self.selected = 0,
            'G' => self.selected = self.hits.len().saturating_sub(1),
            'd' => self.move_by(10),
            'u' => self.move_by(-10),
            '/' | 'i' | 'a' => self.insert = true,
            'q' => return Some(Action::Cancel),
            't' => self.cycle_tag(true),
            'T' => self.cycle_tag(false),
            'w' => self.cycle_kind(),
            'e' => self.begin_edit(),
            // Never type into the search while in normal mode.
            _ => {}
        }
        Some(Action::None)
    }

    fn apply_tags(&mut self, project: &str, tags: Vec<String>) {
        for e in self.entries.iter_mut().filter(|e| e.project == project) {
            e.tags = tags.clone();
        }
        if self.tag.as_ref().is_some_and(|t| !self.tags().contains(t)) {
            self.tag = None;
        }
        let keep = self.selected_entry().map(|e| e.path.clone());
        self.refilter();
        if let Some(i) = keep.and_then(|p| {
            self.hits
                .iter()
                .position(|h| self.entries[h.index].path == p)
        }) {
            self.selected = i;
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if let Some((project, field)) = self.edit.as_mut() {
            match key.code {
                KeyCode::Esc => self.edit = None,
                KeyCode::Char('c') if ctrl => self.edit = None,
                KeyCode::Enter => {
                    let tags = parse_tags(&field.text);
                    let project = project.clone();
                    self.edit = None;
                    return Action::SaveTags { project, tags };
                }
                _ => {
                    field.handle(key);
                }
            }
            return Action::None;
        }

        if self.vim {
            if self.insert && key.code == KeyCode::Esc {
                self.insert = false;
                return Action::None;
            }
            if !self.insert
                && let Some(action) = self.handle_normal(key)
            {
                return action;
            }
        }

        match key.code {
            KeyCode::Esc => return Action::Cancel,
            KeyCode::Char('c') if ctrl => return Action::Cancel,
            KeyCode::Enter => {
                return self
                    .selected_entry()
                    .map_or(Action::None, |e| Action::Select(Box::new(e.clone())));
            }
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::Char('p' | 'k') if ctrl => self.move_by(-1),
            KeyCode::Char('n' | 'j') if ctrl => self.move_by(1),
            KeyCode::PageUp => self.move_by(-(MAX_ROWS as isize)),
            KeyCode::PageDown => self.move_by(MAX_ROWS as isize),
            KeyCode::Tab => self.cycle_tag(true),
            KeyCode::BackTab => self.cycle_tag(false),
            KeyCode::Char('o') if ctrl => self.cycle_kind(),
            KeyCode::Char('e') if ctrl => self.begin_edit(),
            _ => {
                if self.input.handle(key) {
                    self.refilter();
                }
            }
        }
        Action::None
    }
}

/// Tags may be separated by commas and/or whitespace.
fn parse_tags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for t in text.split(|c: char| c == ',' || c.is_whitespace()) {
        if !t.is_empty() && !tags.iter().any(|x| x == t) {
            tags.push(t.to_owned());
        }
    }
    tags
}

fn tilde(path: &std::path::Path) -> String {
    let shown = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => shown
            .strip_prefix(&home)
            .map_or(shown.clone(), |rest| format!("~{rest}")),
        _ => shown,
    }
}

fn chip(text: String, color: Color) -> Span<'static> {
    Span::styled(format!(" {text} "), Style::new().fg(Color::Black).bg(color))
}

fn view(f: &mut Frame, picker: &Picker) {
    let [prompt, list, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    draw_prompt(f, picker, prompt);
    draw_list(f, picker, list);
    draw_status(f, picker, status);
}

fn draw_prompt(f: &mut Frame, picker: &Picker, area: Rect) {
    if let Some((project, field)) = &picker.edit {
        let label = format!("tags for {project}: ");
        let line = Line::from(vec![
            Span::styled(label.clone(), Style::new().fg(Color::Magenta)),
            Span::raw(field.text.clone()),
        ]);
        let x = Line::from(label).width() + Line::from(&field.text[..field.cursor]).width();
        f.render_widget(Paragraph::new(line), area);
        f.set_cursor_position((area.x + x as u16, area.y));
        return;
    }

    let mut chips = Vec::new();
    if let Some(tag) = &picker.tag {
        chips.push(chip(format!("#{tag}"), Color::Magenta));
    }
    match picker.kind {
        Kind::All => {}
        Kind::Projects => chips.push(chip("projects".into(), Color::Cyan)),
        Kind::Worktrees => chips.push(chip("worktrees".into(), Color::Cyan)),
    }
    let chips = Line::from(chips);
    let [text, chips_area] =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(chips.width() as u16)])
            .areas(area);

    let line = Line::from(vec![
        Span::styled("› ", Style::new().fg(Color::Green)),
        Span::raw(picker.input.text.clone()),
    ]);
    let x = 2 + Line::from(&picker.input.text[..picker.input.cursor]).width();
    f.render_widget(Paragraph::new(line), text);
    f.render_widget(
        Paragraph::new(chips).alignment(Alignment::Right),
        chips_area,
    );
    // No cursor in vim normal mode: ratatui hides it when none is set.
    if picker.insert {
        f.set_cursor_position((
            area.x + (x as u16).min(text.width.saturating_sub(1)),
            area.y,
        ));
    }
}

fn draw_list(f: &mut Frame, picker: &Picker, area: Rect) {
    let label_width = picker
        .hits
        .iter()
        .map(|h| picker.entries[h.index].label.chars().count())
        .max()
        .unwrap_or(0)
        .min(MAX_LABEL);

    let items: Vec<ListItem> = picker
        .hits
        .iter()
        .map(|hit| {
            let entry = &picker.entries[hit.index];
            let mut spans = Vec::new();
            let label_len = entry.label.chars().count();
            for (i, c) in entry.label.chars().enumerate() {
                let style = if hit.positions.binary_search(&i).is_ok() {
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::new()
                };
                spans.push(Span::styled(c.to_string(), style));
            }
            spans.push(Span::raw(
                " ".repeat(label_width.saturating_sub(label_len) + 2),
            ));
            if !entry.tags.is_empty() {
                spans.push(Span::styled(
                    format!("#{}  ", entry.tags.join(" #")),
                    Style::new().fg(Color::Magenta),
                ));
            }
            spans.push(Span::styled(
                tilde(&entry.path),
                Style::new().fg(Color::DarkGray),
            ));
            ListItem::new(Line::from(spans))
        })
        .collect();

    if items.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("no match", Style::new().fg(Color::DarkGray))),
            area,
        );
        return;
    }
    let mut state = ListState::default().with_selected(Some(picker.selected));
    f.render_stateful_widget(
        List::new(items).highlight_symbol("▌ ").highlight_style(
            Style::new()
                .add_modifier(Modifier::BOLD)
                .bg(Color::Indexed(236)),
        ),
        area,
        &mut state,
    );
}

fn draw_status(f: &mut Frame, picker: &Picker, area: Rect) {
    let dim = Style::new().fg(Color::DarkGray);
    let text = if picker.edit.is_some() {
        "enter save · esc cancel · separate tags with commas".to_owned()
    } else {
        let mut keys = if picker.vim && !picker.insert {
            vec!["j/k move", "/ search", "t tag", "e edit tags"]
        } else if picker.vim {
            vec!["↑↓ move", "enter pick", "tab tag", "esc normal"]
        } else {
            vec!["↑↓ move", "enter pick", "tab tag", "^e edit tags"]
        };
        if picker.has_worktrees() {
            keys.push(if picker.vim && !picker.insert {
                "w worktrees"
            } else {
                "^o worktrees"
            });
        }
        keys.push(if picker.vim && !picker.insert {
            "q quit"
        } else {
            "esc cancel"
        });
        let mode = match (picker.vim, picker.insert) {
            (true, false) => "NORMAL  ",
            (true, true) => "INSERT  ",
            _ => "",
        };
        format!(
            "{mode}{}/{}  {}",
            picker.hits.len(),
            picker.entries.len(),
            keys.join(" · ")
        )
    };
    f.render_widget(Paragraph::new(Span::styled(text, dim)), area);
}

/// crossterm sends its cursor-position query to stdout, which would end up in
/// the captured result (`$(po pick)`) and never reach the terminal. Point fd 1
/// at the tty for the duration of the picker and put it back afterwards.
struct StdoutRedirect(libc::c_int);

impl StdoutRedirect {
    fn to(tty: &File) -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;
        std::io::stdout().flush()?;
        // SAFETY: plain fd duplication; the saved fd is closed in `drop`.
        unsafe {
            let saved = libc::dup(1);
            if saved < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::dup2(tty.as_raw_fd(), 1) < 0 {
                let err = std::io::Error::last_os_error();
                libc::close(saved);
                return Err(err);
            }
            Ok(Self(saved))
        }
    }
}

impl Drop for StdoutRedirect {
    fn drop(&mut self) {
        // SAFETY: `self.0` is the fd saved in `to`.
        unsafe {
            libc::dup2(self.0, 1);
            libc::close(self.0);
        }
    }
}

/// Restores the terminal on drop (normal exit, error, or panic unwind).
struct TermGuard {
    tty: File,
    alt_screen: bool,
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.alt_screen {
            let _ = execute!(self.tty, LeaveAlternateScreen);
        }
        let _ = execute!(self.tty, crossterm::cursor::Show);
        let _ = self.tty.flush();
    }
}

pub enum Outcome {
    /// The terminal did not answer the cursor-position query an inline view needs.
    Unsupported,
    Cancelled,
    Picked(Box<Entry>),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// The alternate screen instead of a few inline rows.
    pub full_screen: bool,
    /// Modal keys, like `bracco --vim`.
    pub vim: bool,
}

/// Tag edits are handed to `save_tags` as they happen.
pub fn run(
    tty: File,
    entries: Vec<Entry>,
    query: &str,
    save_tags: &mut dyn FnMut(&str, &[String]) -> Result<()>,
    options: Options,
) -> Result<Outcome> {
    let full_screen = options.full_screen;
    let _redirect = StdoutRedirect::to(&tty)?;
    enable_raw_mode()?;
    let _guard = TermGuard {
        tty: tty.try_clone()?,
        alt_screen: full_screen,
    };
    if full_screen {
        execute!(&tty, EnterAlternateScreen)?;
    }

    let prev = std::panic::take_hook();
    let panic_tty = tty.try_clone()?;
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        if full_screen {
            let _ = execute!(&panic_tty, LeaveAlternateScreen);
        }
        prev(info);
    }));

    let viewport = if full_screen {
        Viewport::Fullscreen
    } else {
        Viewport::Inline(2 + entries.len().clamp(1, MAX_ROWS) as u16)
    };
    let Ok(mut terminal) =
        Terminal::with_options(CrosstermBackend::new(tty), TerminalOptions { viewport })
    else {
        return Ok(Outcome::Unsupported);
    };
    let mut picker = Picker::new(entries, query, options.vim);

    let picked = loop {
        terminal.draw(|f| view(f, &picker))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match picker.handle_key(key) {
            Action::None => {}
            Action::Cancel => break Outcome::Cancelled,
            Action::Select(entry) => break Outcome::Picked(entry),
            Action::SaveTags { project, tags } => {
                save_tags(&project, &tags)?;
                picker.apply_tags(&project, tags);
            }
        }
    };

    // The alternate screen is discarded on exit; an inline view has to erase itself.
    if !full_screen {
        terminal.clear()?;
    }
    terminal.show_cursor()?;
    Ok(picked)
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::entries::WorktreeRef;

    fn entry(label: &str, project: &str, tags: &[&str], wt: bool) -> Entry {
        Entry {
            label: label.into(),
            project: project.into(),
            path: format!("/w/{label}").into(),
            project_path: format!("/w/{project}").into(),
            tags: tags.iter().map(|t| (*t).into()).collect(),
            worktree: wt.then(|| WorktreeRef {
                name: label.into(),
                branch: Some("b".into()),
            }),
            rank: 0.0,
        }
    }

    fn picker() -> Picker {
        Picker::new(
            vec![
                entry("shop", "shop", &["work"], false),
                entry("shop:feat", "shop", &["work"], true),
                entry("blog", "blog", &["me"], false),
                entry("tools", "tools", &[], false),
            ],
            "",
            false,
        )
    }

    fn vim_picker() -> Picker {
        let mut p = picker();
        p.vim = true;
        p.insert = false;
        p
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn labels(p: &Picker) -> Vec<&str> {
        p.hits
            .iter()
            .map(|h| p.entries[h.index].label.as_str())
            .collect()
    }

    fn type_text(p: &mut Picker, text: &str) {
        for c in text.chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn typing_filters_and_ranks() {
        let mut p = picker();
        type_text(&mut p, "sh");
        assert_eq!(labels(&p), ["shop", "shop:feat"]);
        p.handle_key(ctrl('u'));
        assert_eq!(p.hits.len(), 4);
    }

    #[test]
    fn tab_cycles_tags_through_no_filter() {
        let mut p = picker();
        p.handle_key(key(KeyCode::Tab));
        assert_eq!(p.tag.as_deref(), Some("me"));
        assert_eq!(labels(&p), ["blog"]);
        p.handle_key(key(KeyCode::Tab));
        assert_eq!(p.tag.as_deref(), Some("work"));
        p.handle_key(key(KeyCode::Tab));
        assert_eq!(p.tag, None);
        p.handle_key(key(KeyCode::BackTab));
        assert_eq!(p.tag.as_deref(), Some("work"));
    }

    #[test]
    fn ctrl_o_cycles_kind() {
        let mut p = picker();
        p.handle_key(ctrl('o'));
        assert_eq!(labels(&p), ["shop", "blog", "tools"]);
        p.handle_key(ctrl('o'));
        assert_eq!(labels(&p), ["shop:feat"]);
        p.handle_key(ctrl('o'));
        assert_eq!(p.hits.len(), 4);
    }

    #[test]
    fn enter_selects_and_esc_cancels() {
        let mut p = picker();
        p.handle_key(key(KeyCode::Down));
        match p.handle_key(key(KeyCode::Enter)) {
            Action::Select(e) => assert_eq!(e.label, "shop:feat"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(p.handle_key(key(KeyCode::Esc)), Action::Cancel));
    }

    #[test]
    fn selection_is_clamped() {
        let mut p = picker();
        p.move_by(100);
        assert_eq!(p.selected, 3);
        p.move_by(-100);
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn edits_tags_of_the_highlighted_project() {
        let mut p = picker();
        p.handle_key(ctrl('e'));
        assert_eq!(p.edit.as_ref().unwrap().1.text, "work");
        type_text(&mut p, ", rust rust");
        let Action::SaveTags { project, tags } = p.handle_key(key(KeyCode::Enter)) else {
            panic!("expected SaveTags");
        };
        assert_eq!(project, "shop");
        assert_eq!(tags, ["work", "rust"]);
        p.apply_tags(&project, tags);
        // Worktree entries share the project's tags.
        assert_eq!(p.entries[1].tags, ["work", "rust"]);
        assert!(p.edit.is_none());
    }

    #[test]
    fn esc_leaves_the_tag_editor_without_cancelling() {
        let mut p = picker();
        p.handle_key(ctrl('e'));
        assert!(matches!(p.handle_key(key(KeyCode::Esc)), Action::None));
        assert!(p.edit.is_none());
    }

    #[test]
    fn dropping_the_last_tag_clears_its_filter() {
        let mut p = picker();
        p.handle_key(key(KeyCode::Tab)); // #me -> blog
        p.handle_key(ctrl('e'));
        p.handle_key(ctrl('u'));
        let Action::SaveTags { project, tags } = p.handle_key(key(KeyCode::Enter)) else {
            panic!("expected SaveTags");
        };
        p.apply_tags(&project, tags);
        assert_eq!(p.tag, None);
        assert_eq!(p.hits.len(), 4);
    }

    #[test]
    fn text_input_handles_multibyte() {
        let mut t = TextInput::new("aé日");
        t.backspace();
        assert_eq!(t.text, "aé");
        t.handle(key(KeyCode::Left));
        t.delete();
        assert_eq!(t.text, "a");
    }

    #[test]
    fn parses_tag_lists() {
        assert_eq!(parse_tags("a, b  c,,a"), ["a", "b", "c"]);
        assert!(parse_tags("  ").is_empty());
    }

    fn render(p: &Picker, w: u16, h: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| view(f, p)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn renders_prompt_rows_and_status() {
        let mut p = picker();
        type_text(&mut p, "blog");
        let lines = render(&p, 70, 6);
        assert!(lines[0].starts_with("› blog"), "{lines:?}");
        assert!(
            lines[1].contains("blog") && lines[1].contains("#me"),
            "{lines:?}"
        );
        assert!(lines[5].starts_with("1/4"), "{lines:?}");
    }

    #[test]
    fn renders_filter_chips() {
        let mut p = picker();
        p.handle_key(key(KeyCode::Tab));
        let lines = render(&p, 70, 6);
        assert!(lines[0].trim_end().ends_with("#me"), "{lines:?}");
    }

    #[test]
    fn vim_starts_in_normal_mode_and_does_not_type() {
        let mut p = vim_picker();
        type_text(&mut p, "xyz");
        assert_eq!(p.input.text, "");
        assert_eq!(p.hits.len(), 4);
    }

    #[test]
    fn vim_motion_keys() {
        let mut p = vim_picker();
        p.handle_key(key(KeyCode::Char('j')));
        assert_eq!(p.selected, 1);
        p.handle_key(key(KeyCode::Char('G')));
        assert_eq!(p.selected, 3);
        p.handle_key(key(KeyCode::Char('k')));
        assert_eq!(p.selected, 2);
        p.handle_key(key(KeyCode::Char('g')));
        assert_eq!(p.selected, 0);
        p.handle_key(key(KeyCode::Char('d')));
        assert_eq!(p.selected, 3);
        p.handle_key(key(KeyCode::Char('u')));
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn vim_search_roundtrip() {
        let mut p = vim_picker();
        p.handle_key(key(KeyCode::Char('/')));
        type_text(&mut p, "blog");
        assert_eq!(labels(&p), ["blog"]);
        // Esc leaves search, keeps the filter, and does not cancel.
        assert!(matches!(p.handle_key(key(KeyCode::Esc)), Action::None));
        assert!(!p.insert);
        assert_eq!(labels(&p), ["blog"]);
        // In normal mode esc cancels and q quits.
        assert!(matches!(p.handle_key(key(KeyCode::Esc)), Action::Cancel));
        assert!(matches!(
            p.handle_key(key(KeyCode::Char('q'))),
            Action::Cancel
        ));
    }

    #[test]
    fn vim_normal_mode_tag_and_kind_keys() {
        let mut p = vim_picker();
        p.handle_key(key(KeyCode::Char('t')));
        assert_eq!(p.tag.as_deref(), Some("me"));
        p.handle_key(key(KeyCode::Char('T')));
        assert_eq!(p.tag, None);
        p.handle_key(key(KeyCode::Char('w')));
        assert_eq!(p.kind, Kind::Projects);
        p.handle_key(key(KeyCode::Char('e')));
        assert!(p.edit.is_some());
        // Typing in the tag editor still works, and enter returns to normal mode.
        type_text(&mut p, "x");
        assert!(matches!(
            p.handle_key(key(KeyCode::Enter)),
            Action::SaveTags { .. }
        ));
        assert!(!p.insert);
    }

    #[test]
    fn vim_enter_still_picks() {
        let mut p = vim_picker();
        assert!(matches!(
            p.handle_key(key(KeyCode::Enter)),
            Action::Select(_)
        ));
    }

    #[test]
    fn non_vim_esc_cancels_and_typing_works() {
        let mut p = picker();
        type_text(&mut p, "j");
        assert_eq!(p.input.text, "j");
        assert!(matches!(p.handle_key(key(KeyCode::Esc)), Action::Cancel));
    }
}
