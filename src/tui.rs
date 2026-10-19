//! `deeds tui`: browse the graph and read issues without retyping ids.
//! A viewer for the human; the only write path is handing a file to
//! $EDITOR. Polls `.deeds/` so edits made elsewhere show up live.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, bail};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::graph::Graph;
use crate::issue::{DEFAULT_TIER, Issue, find_deeds_dir, load_all, tier_name};
use crate::relative;

pub fn run() -> Result<()> {
    if !std::io::stdout().is_terminal() {
        bail!("tui needs a terminal");
    }
    let mut app = App::new(find_deeds_dir()?);
    app.load()?;
    let mut terminal = ratatui::init();
    let result = app.event_loop(&mut terminal);
    ratatui::restore();
    result
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Tree,
    Ready,
    Closed,
}

const VIEWS: [View; 3] = [View::Tree, View::Ready, View::Closed];

/// One line of the left pane.
struct Row {
    id: String,
    /// Ancestor ids plus own id: tells apart repeats of a shared blocker.
    key: String,
    /// None for a blocker id with no file: gc'd or a typo.
    issue: Option<usize>,
    depth: usize,
    /// Tree guides and fold marker; empty in flat views.
    guide: String,
    /// Some when the node has children to show.
    folded: Option<bool>,
    /// Back edge to an ancestor; not expanded.
    cycle: bool,
}

struct App {
    deeds: PathBuf,
    issues: Vec<Issue>,
    index: HashMap<String, usize>,
    /// For closed issues this is the close time: `close` stamps it.
    mtimes: Vec<SystemTime>,
    stamp: Vec<(PathBuf, SystemTime, u64)>,
    view: View,
    show_closed: bool,
    /// Ids folded against the default: open expanded, closed folded.
    flipped: HashSet<String>,
    filter: String,
    typing: bool,
    rows: Vec<Row>,
    list: ListState,
    scroll: u16,
    /// Content pane height at last render, for paging.
    page: u16,
    flash: Option<String>,
}

impl App {
    fn new(deeds: PathBuf) -> Self {
        App {
            deeds,
            issues: vec![],
            index: HashMap::new(),
            mtimes: vec![],
            stamp: vec![],
            view: View::Tree,
            show_closed: false,
            flipped: HashSet::new(),
            filter: String::new(),
            typing: false,
            rows: vec![],
            list: ListState::default(),
            scroll: 0,
            page: 0,
            flash: None,
        }
    }

    fn load(&mut self) -> Result<()> {
        // Stamp first: a write racing the load shows up on the next poll.
        self.stamp = stamp(&self.deeds);
        self.set_issues(load_all(&self.deeds)?);
        Ok(())
    }

    fn set_issues(&mut self, issues: Vec<Issue>) {
        self.issues = issues;
        self.mtimes = self
            .issues
            .iter()
            .map(|i| i.path.metadata().and_then(|m| m.modified()))
            .map(|t| t.unwrap_or(SystemTime::UNIX_EPOCH))
            .collect();
        self.index = self
            .issues
            .iter()
            .enumerate()
            .map(|(n, i)| (i.id.clone(), n))
            .collect();
        // Resolution matches the CLI (the closed copy wins); say so loudly.
        let dups: Vec<&str> = self
            .issues
            .windows(2)
            .filter(|w| w[0].id == w[1].id)
            .map(|w| w[0].id.as_str())
            .collect();
        if !dups.is_empty() {
            self.flash = Some(format!(
                "duplicate ids {}: see deeds check",
                dups.join(", ")
            ));
        }
        self.rebuild();
    }

    fn reload(&mut self) {
        if let Err(e) = self.load() {
            self.flash = Some(format!("reload: {e:#}"));
        }
    }

    fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut polled = Instant::now();
        loop {
            terminal.draw(|frame| self.render(frame))?;
            if event::poll(Duration::from_millis(500))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && self.on_key(key, terminal)?
            {
                return Ok(());
            }
            if polled.elapsed() >= Duration::from_secs(1) {
                polled = Instant::now();
                if stamp(&self.deeds) != self.stamp {
                    self.reload();
                }
            }
        }
    }

    // ---- rows ----

    fn selected(&self) -> Option<&Row> {
        self.list.selected().and_then(|k| self.rows.get(k))
    }

    /// Recompute the left pane, keeping the selection on the same node.
    fn rebuild(&mut self) {
        let old = self.selected().map(|r| (r.key.clone(), r.id.clone()));
        self.rows = self.view_rows();
        let mut found = old.as_ref().and_then(|(key, id)| self.locate(key, id));
        // An issue folded away, e.g. arriving from another view: unfold to it.
        if found.is_none()
            && let Some((key, id)) = &old
            && self.view == View::Tree
            && self.filter.is_empty()
            && self.index.get(id).is_some_and(|&n| self.shown(n))
        {
            self.unfold_dependents(id);
            self.rows = self.tree_rows();
            found = self.locate(key, id);
        }
        let last = self.rows.len().saturating_sub(1);
        let k = found.unwrap_or(self.list.selected().unwrap_or(0)).min(last);
        self.list.select((!self.rows.is_empty()).then_some(k));
        if self.selected().map(|r| &r.id) != old.as_ref().map(|(_, id)| id) {
            self.scroll = 0;
        }
    }

    fn view_rows(&self) -> Vec<Row> {
        match self.view {
            View::Tree => self.tree_rows(),
            View::Ready => {
                let g = Graph::new(&self.issues);
                let ready = g.ready().iter().map(|i| self.index[&i.id]).collect();
                self.flat_rows(ready)
            }
            View::Closed => {
                let mut closed: Vec<usize> = (0..self.issues.len())
                    .filter(|&n| self.issues[n].closed)
                    .collect();
                closed.sort_by_key(|&n| Reverse(self.mtimes[n]));
                self.flat_rows(closed)
            }
        }
    }

    /// Same tree position if it survived, else the issue's first row.
    fn locate(&self, key: &str, id: &str) -> Option<usize> {
        let by_key = self.rows.iter().position(|r| r.key == key);
        by_key.or_else(|| self.rows.iter().position(|r| r.id == id))
    }

    /// Unfold everything that transitively waits on `id`.
    fn unfold_dependents(&mut self, id: &str) {
        let mut stack = vec![id.to_string()];
        let mut seen = HashSet::new();
        while let Some(id) = stack.pop() {
            for n in 0..self.issues.len() {
                if self.shown(n) && self.issues[n].blocked_by.contains(&id) && seen.insert(n) {
                    if !self.expanded(n) {
                        self.flip(self.issues[n].id.clone());
                    }
                    stack.push(self.issues[n].id.clone());
                }
            }
        }
    }

    fn flip(&mut self, id: String) {
        if !self.flipped.remove(&id) {
            self.flipped.insert(id);
        }
    }

    fn matches(&self, n: usize) -> bool {
        let needle = self.filter.to_lowercase();
        let issue = &self.issues[n];
        issue.id.contains(&needle) || issue.text.to_lowercase().contains(&needle)
    }

    fn flat_rows(&self, issues: Vec<usize>) -> Vec<Row> {
        issues
            .into_iter()
            .filter(|&n| self.matches(n))
            .map(|n| Row {
                id: self.issues[n].id.clone(),
                key: self.issues[n].id.clone(),
                issue: Some(n),
                depth: 0,
                guide: String::new(),
                folded: None,
                cycle: false,
            })
            .collect()
    }

    fn shown(&self, n: usize) -> bool {
        self.show_closed || !self.issues[n].closed
    }

    fn expanded(&self, n: usize) -> bool {
        let issue = &self.issues[n];
        !issue.closed != self.flipped.contains(&issue.id)
    }

    /// Open first, then tier, then id; unknown ids last.
    fn sort_key<'s>(&self, id: &'s str) -> (u8, u8, &'s str) {
        match self.index.get(id) {
            Some(&n) => (self.issues[n].closed as u8, self.issues[n].tier, id),
            None => (2, 0, id),
        }
    }

    /// Blockers drawn under an issue. Unknown ids are mostly gc'd closed
    /// issues, so they ride along with the closed toggle.
    fn children(&self, n: usize) -> Vec<&str> {
        let mut kids: Vec<&str> = self.issues[n]
            .blocked_by
            .iter()
            .map(String::as_str)
            .filter(|id| match self.index.get(*id) {
                Some(&k) => self.shown(k),
                None => self.show_closed,
            })
            .collect();
        kids.sort_by_key(|id| self.sort_key(id));
        kids.dedup();
        kids
    }

    /// The blocker DAG top-down: each issue's children are its blockers.
    fn tree_rows(&self) -> Vec<Row> {
        let mut has_dependent = HashSet::new();
        let mut has_open_dependent = HashSet::new();
        for issue in &self.issues {
            for b in &issue.blocked_by {
                has_dependent.insert(b.as_str());
                if !issue.closed {
                    has_open_dependent.insert(b.as_str());
                }
            }
        }
        // Roots: open issues nothing open waits on, closed ones nothing at
        // all waits on. Open parts of a closed milestone are roots too.
        let mut roots: Vec<usize> = (0..self.issues.len())
            .filter(|&n| self.shown(n))
            .filter(|&n| {
                let issue = &self.issues[n];
                let deps = if issue.closed {
                    &has_dependent
                } else {
                    &has_open_dependent
                };
                !deps.contains(issue.id.as_str())
            })
            .collect();
        // A cycle has no top; promote one member of each stranded component.
        let mut seen = HashSet::new();
        for &n in &roots {
            self.reach(&self.issues[n].id, &mut seen);
        }
        for n in 0..self.issues.len() {
            if self.shown(n) && !seen.contains(self.issues[n].id.as_str()) {
                roots.push(n);
                self.reach(&self.issues[n].id, &mut seen);
            }
        }
        roots.sort_by_key(|&n| self.sort_key(&self.issues[n].id));

        let keep = (!self.filter.is_empty()).then(|| self.filter_keep());
        let mut rows = vec![];
        for &n in &roots {
            let id = self.issues[n].id.as_str();
            if keep.as_ref().is_none_or(|k| k.contains(id)) {
                self.walk(id, &mut vec![], &mut vec![], keep.as_ref(), &mut rows);
            }
        }
        rows
    }

    fn reach<'s>(&'s self, from: &'s str, seen: &mut HashSet<&'s str>) {
        let mut stack = vec![from];
        while let Some(id) = stack.pop() {
            if seen.insert(id)
                && let Some(&n) = self.index.get(id)
            {
                stack.extend(self.children(n));
            }
        }
    }

    /// Under a filter a node stays if it or anything below it matches:
    /// walk child-to-parent edges up from every match.
    fn filter_keep(&self) -> HashSet<&str> {
        let needle = self.filter.to_lowercase();
        let mut parents: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut stack = vec![];
        for n in (0..self.issues.len()).filter(|&n| self.shown(n)) {
            let id = self.issues[n].id.as_str();
            for kid in self.children(n) {
                parents.entry(kid).or_default().push(id);
                if !self.index.contains_key(kid) && kid.contains(&needle) {
                    stack.push(kid);
                }
            }
            if self.matches(n) {
                stack.push(id);
            }
        }
        let mut keep = HashSet::new();
        while let Some(id) = stack.pop() {
            if keep.insert(id) {
                stack.extend(parents.get(id).into_iter().flatten().copied());
            }
        }
        keep
    }

    /// `lasts[d]`: whether the ancestor at depth d+1 is its parent's last
    /// child, which decides between "│ " and "  " in the guides.
    fn walk<'s>(
        &'s self,
        id: &'s str,
        lasts: &mut Vec<bool>,
        path: &mut Vec<&'s str>,
        keep: Option<&HashSet<&str>>,
        rows: &mut Vec<Row>,
    ) {
        let n = self.index.get(id).copied();
        let cycle = path.contains(&id);
        let kids: Vec<&str> = match n {
            Some(n) if !cycle => self
                .children(n)
                .into_iter()
                .filter(|c| keep.is_none_or(|k| k.contains(c)))
                .collect(),
            _ => vec![],
        };
        // Filtering shows every match, so it overrides folds.
        let folded = n
            .filter(|_| !kids.is_empty())
            .map(|n| self.filter.is_empty() && !self.expanded(n));
        let marker = match folded {
            Some(true) => "▸ ",
            Some(false) => "▾ ",
            None if lasts.is_empty() => "  ",
            None => "─ ",
        };
        let mut guide = String::new();
        if let Some((&last, above)) = lasts.split_last() {
            for &l in above {
                guide += if l { "  " } else { "│ " };
            }
            guide += if last { "└─" } else { "├─" };
        }
        guide += marker;
        let mut key = path.join("/");
        key += &format!("/{id}");
        rows.push(Row {
            id: id.to_string(),
            key,
            issue: n,
            depth: lasts.len(),
            guide,
            folded,
            cycle,
        });
        if folded != Some(false) {
            return;
        }
        path.push(id);
        for (k, kid) in kids.iter().enumerate() {
            lasts.push(k + 1 == kids.len());
            self.walk(kid, lasts, path, keep, rows);
            lasts.pop();
        }
        path.pop();
    }

    // ---- keys ----

    /// Returns true to quit.
    fn on_key(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) -> Result<bool> {
        self.flash = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Ok(true);
        }
        if self.typing {
            match key.code {
                KeyCode::Esc => {
                    self.typing = false;
                    self.filter.clear();
                }
                KeyCode::Enter => self.typing = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char('u') if ctrl => self.filter.clear(),
                KeyCode::Char(c) if !ctrl => self.filter.push(c),
                _ => return Ok(false),
            }
            self.rebuild();
            return Ok(false);
        }
        let page = self.page.saturating_sub(1).max(1);
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.rebuild();
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::Char('g') | KeyCode::Home => self.move_by(isize::MIN / 2),
            KeyCode::Char('G') | KeyCode::End => self.move_by(isize::MAX / 2),
            KeyCode::Char('h') | KeyCode::Left => self.fold_or_parent(),
            KeyCode::Char('l') | KeyCode::Right => match self.selected().and_then(|r| r.folded) {
                Some(true) => self.toggle_fold(),
                Some(false) => self.move_by(1),
                None => {}
            },
            KeyCode::Enter => self.toggle_fold(),
            KeyCode::Char('J') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Char('K') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char(' ') | KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(page)
            }
            KeyCode::Char('b') | KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(page),
            KeyCode::Tab => self.switch(1),
            KeyCode::BackTab => self.switch(VIEWS.len() - 1),
            KeyCode::Char(c @ '1'..='3') => {
                let target = c as usize - '1' as usize;
                let current = VIEWS.iter().position(|&v| v == self.view).unwrap();
                self.switch(target + VIEWS.len() - current);
            }
            KeyCode::Char('c') => {
                self.show_closed = !self.show_closed;
                self.rebuild();
            }
            KeyCode::Char('/') => self.typing = true,
            KeyCode::Char('e') => self.edit(terminal)?,
            KeyCode::Char('r') => {
                self.reload();
                self.flash.get_or_insert_with(|| "reloaded".into());
            }
            _ => {}
        }
        Ok(false)
    }

    fn move_by(&mut self, delta: isize) {
        let Some(k) = self.list.selected() else {
            return;
        };
        let last = self.rows.len().saturating_sub(1) as isize;
        let to = (k as isize).saturating_add(delta).clamp(0, last) as usize;
        if to != k {
            self.list.select(Some(to));
            self.scroll = 0;
        }
    }

    /// Switching keeps the selected issue if the new view has it, so
    /// Ready → Tree locates an issue in the DAG.
    fn switch(&mut self, step: usize) {
        let current = VIEWS.iter().position(|&v| v == self.view).unwrap();
        self.view = VIEWS[(current + step) % VIEWS.len()];
        self.rebuild();
    }

    fn fold_or_parent(&mut self) {
        let Some(row) = self.selected() else {
            return;
        };
        if row.folded == Some(false) && self.filter.is_empty() {
            return self.toggle_fold();
        }
        let k = self.list.selected().unwrap();
        let depth = row.depth;
        if let Some(parent) = self.rows[..k].iter().rposition(|r| r.depth < depth) {
            self.move_by(parent as isize - k as isize);
        }
    }

    fn toggle_fold(&mut self) {
        let Some(row) = self.selected() else {
            return;
        };
        if row.folded.is_none() || !self.filter.is_empty() {
            return;
        }
        self.flip(row.id.clone());
        self.rebuild();
    }

    fn edit(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let Some(n) = self.selected().and_then(|r| r.issue) else {
            return Ok(());
        };
        let path = self.issues[n].path.clone();
        let editor = ["VISUAL", "EDITOR"]
            .into_iter()
            .filter_map(|var| std::env::var(var).ok())
            .find(|e| !e.trim().is_empty())
            .unwrap_or_else(|| "vi".into());
        ratatui::restore();
        // Through the shell, as git does: EDITOR may carry quoted arguments.
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("{editor} \"$@\""))
            .arg(&editor)
            .arg(&path)
            .status();
        enable_raw_mode()?;
        execute!(std::io::stdout(), EnterAlternateScreen)?;
        terminal.clear()?;
        match status {
            Ok(s) if !s.success() => self.flash = Some(format!("{editor}: {s}")),
            Err(e) => self.flash = Some(format!("{editor}: {e}")),
            Ok(_) => {}
        }
        self.reload();
        Ok(())
    }

    // ---- drawing ----

    fn render(&mut self, frame: &mut Frame) {
        let [top, main, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [left, right] = if main.width >= 100 {
            Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(main)
        } else {
            Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(main)
        };
        let g = Graph::new(&self.issues);

        self.render_tabs(frame, top, &g);

        let selected = self.list.selected();
        let items: Vec<ListItem> = self
            .rows
            .iter()
            .enumerate()
            .map(|(k, row)| {
                let line = self.row_line(row, &g);
                // Plain text under the highlight, so reversal gives one solid bar.
                if Some(k) == selected {
                    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                    ListItem::new(text)
                } else {
                    ListItem::new(line)
                }
            })
            .collect();
        let mut title = vec![];
        if self.view == View::Tree && self.show_closed {
            title.push("+closed".to_string());
        }
        if !self.filter.is_empty() {
            title.push(format!("/{}", self.filter));
        }
        let mut block = pane();
        if !title.is_empty() {
            block = block.title(format!(" {} ", title.join(" ")));
        }
        if items.is_empty() {
            block = block.title_bottom(" nothing here ");
        }
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .scroll_padding(2);
        frame.render_stateful_widget(list, left, &mut self.list);

        let block = pane();
        let inner = block.inner(right);
        let (text, path) = match self.selected() {
            Some(row) => (
                self.content(row, &g, inner.width),
                row.issue.map(|n| relative(&self.issues[n].path)),
            ),
            None => (Text::default(), None),
        };
        let para = Paragraph::new(text).wrap(Wrap { trim: false });
        let max = (para.line_count(inner.width) as u16).saturating_sub(inner.height);
        self.scroll = self.scroll.min(max);
        self.page = inner.height;
        let mut block = block;
        if let Some(path) = path {
            block = block.title(format!(" {path} "));
        }
        if max > 0 {
            let pct = self.scroll as u32 * 100 / max as u32;
            block = block.title_bottom(Line::from(format!(" {pct}% ")).right_aligned());
        }
        frame.render_widget(para.scroll((self.scroll, 0)).block(block), right);

        self.render_status(frame, bottom);
    }

    fn render_tabs(&self, frame: &mut Frame, area: Rect, g: &Graph) {
        let open = self.issues.iter().filter(|i| !i.closed).count();
        let titles = [
            format!("Tree {open}"),
            format!("Ready {}", g.ready().len()),
            format!("Closed {}", self.issues.len() - open),
        ];
        let current = VIEWS.iter().position(|&v| v == self.view);
        let tabs = Tabs::new(titles)
            .select(current)
            .highlight_style(Style::new().bold().reversed());
        let project = self
            .deeds
            .parent()
            .and_then(Path::file_name)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let project = Line::from(project).dark_gray().right_aligned();
        let [tabs_area, project_area] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(project.width() as u16 + 1),
        ])
        .areas(area);
        frame.render_widget(tabs, tabs_area);
        frame.render_widget(project, project_area);
    }

    fn render_status(&self, frame: &mut Frame, area: Rect) {
        if self.typing {
            let line = Line::from(format!("/{}", self.filter));
            let x = area.x + line.width() as u16;
            frame.render_widget(line, area);
            frame.set_cursor_position((x, area.y));
            return;
        }
        if let Some(msg) = &self.flash {
            frame.render_widget(Line::from(msg.as_str()).yellow(), area);
            return;
        }
        let keys = [
            ("j/k", "move"),
            ("h/l", "fold"),
            ("J/K", "scroll"),
            ("tab", "view"),
            ("/", "filter"),
            ("c", "closed"),
            ("e", "edit"),
            ("q", "quit"),
        ];
        let spans: Vec<Span> = keys
            .iter()
            .flat_map(|(k, what)| [Span::from(*k).bold(), format!(" {what}  ").dark_gray()])
            .collect();
        frame.render_widget(Line::from(spans), area);
    }

    fn row_line(&self, row: &Row, g: &Graph) -> Line<'static> {
        let mut spans = vec![Span::from(row.guide.clone()).dark_gray()];
        let Some(n) = row.issue else {
            spans.push(format!("? {} gone", row.id).dark_gray());
            return Line::from(spans);
        };
        let issue = &self.issues[n];
        // Rows kept only as ancestors of a filter match recede.
        let dim = issue.closed || !self.matches(n);
        let (glyph, glyph_style) = status(issue, g);
        spans.push(Span::styled(format!("{glyph} "), glyph_style));
        spans.push(Span::from(format!("{} ", issue.id)));
        if self.view == View::Closed {
            spans.push(format!("{:>4} ", age(self.mtimes[n])).dark_gray());
        }
        if issue.tier != DEFAULT_TIER {
            spans.push(Span::styled(
                format!("{} ", tier_name(issue.tier)),
                tier_style(issue.tier),
            ));
        }
        let title = Span::from(issue.title.clone());
        spans.push(if dim { title.dark_gray() } else { title });
        if row.cycle {
            spans.push(" ↻ cycle".red());
        }
        Line::from(spans)
    }

    fn content(&self, row: &Row, g: &Graph, width: u16) -> Text<'static> {
        let Some(n) = row.issue else {
            return Text::from(vec![
                Line::from(vec![row.id.clone().bold(), "  gone".dark_gray()]),
                Line::default(),
                Line::from(format!(
                    "Named as a blocker, but no file has this id: gc'd or a typo. \
                     Gc'd issues live in VCS history as .deeds/closed/{}.md.",
                    row.id
                )),
            ]);
        };
        let issue = &self.issues[n];
        let state = if issue.closed {
            format!("closed {} ago", age(self.mtimes[n])).dark_gray()
        } else if g.open_blockers(issue).is_empty() {
            "ready".green()
        } else {
            "blocked".yellow()
        };
        let mut head = vec![issue.id.clone().bold(), "  ".into(), state];
        if issue.tier != DEFAULT_TIER {
            head.push("  ".into());
            head.push(Span::styled(tier_name(issue.tier), tier_style(issue.tier)));
        }
        let mut lines = vec![Line::from(head)];
        for (k, id) in issue.blocked_by.iter().enumerate() {
            lines.push(edge_line(if k == 0 { "blocked by" } else { "" }, id, g));
        }
        for (k, b) in g.blocks(&issue.id).iter().enumerate() {
            lines.push(edge_line(if k == 0 { "blocks" } else { "" }, &b.id, g));
        }
        for (k, problem) in issue.problems.iter().enumerate() {
            let label = if k == 0 { "lint" } else { "" };
            lines.push(Line::from(format!("{label:<11}{problem}")).red());
        }
        lines.push(Line::from("─".repeat(width as usize)).dark_gray());
        lines.extend(markdown(&issue.text));
        Text::from(lines)
    }
}

fn pane() -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().dark_gray())
}

fn status(issue: &Issue, g: &Graph) -> (&'static str, Style) {
    if issue.closed {
        ("✓", Style::new().dark_gray())
    } else if g.open_blockers(issue).is_empty() {
        ("●", Style::new().green())
    } else {
        ("○", Style::new().yellow())
    }
}

fn tier_style(tier: u8) -> Style {
    match tier_name(tier) {
        "objective" => Style::new().magenta().bold(),
        "nit" => Style::new().blue(),
        _ => Style::new().cyan(),
    }
}

fn edge_line(label: &str, id: &str, g: &Graph) -> Line<'static> {
    let mut spans = vec![format!("{label:<11}").dark_gray()];
    match g.by_id.get(id) {
        Some(b) => {
            let (glyph, style) = status(b, g);
            let title = Span::from(b.title.clone());
            spans.push(Span::styled(format!("{glyph} "), style));
            spans.push(format!("{id} ").into());
            spans.push(if b.closed { title.dark_gray() } else { title });
        }
        None => spans.push(format!("? {id} gone").dark_gray()),
    }
    Line::from(spans)
}

fn age(t: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(t)
        .map_or(0, |d| d.as_secs());
    match secs {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

/// Change detector: name, mtime and size of every issue file.
fn stamp(deeds: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    let mut out = vec![];
    for dir in [deeds.to_path_buf(), deeds.join("closed")] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "md") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((path, mtime, meta.len()));
            }
        }
    }
    out.sort();
    out
}

/// Line-level markdown styling: enough to scan an issue, not a renderer.
fn markdown(text: &str) -> Vec<Line<'static>> {
    let mut out = vec![];
    let mut lines = text.lines().peekable();
    if lines.peek().map(|l| l.trim()) == Some("---") {
        out.push(Line::from(lines.next().unwrap().to_string()).dark_gray());
        for line in lines.by_ref() {
            out.push(Line::from(line.to_string()).dark_gray());
            if line.trim() == "---" {
                break;
            }
        }
    }
    let mut fenced = false;
    for line in lines {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            out.push(Line::from(line.to_string()).dark_gray());
        } else if fenced {
            out.push(Line::from(line.to_string()).yellow());
        } else if is_heading(line) {
            out.push(Line::from(line.to_string()).cyan().bold());
        } else if let Some(rest) = line.strip_prefix("Closed:") {
            let mut spans = vec!["Closed:".green().bold()];
            spans.extend(inline(rest));
            out.push(Line::from(spans));
        } else {
            out.push(Line::from(inline(line)));
        }
    }
    out
}

fn is_heading(line: &str) -> bool {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&hashes) && line[hashes..].starts_with(' ')
}

/// `code` and **bold** spans.
fn inline(line: &str) -> Vec<Span<'static>> {
    let mut spans = vec![];
    for (k, part) in line.split('`').enumerate() {
        if k % 2 == 1 {
            spans.push(part.to_string().yellow());
            continue;
        }
        for (j, piece) in part.split("**").enumerate() {
            let piece = piece.to_string();
            spans.push(if j % 2 == 1 {
                piece.bold()
            } else {
                piece.into()
            });
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::issue::parse;

    fn app(specs: &[(&str, &[&str], bool)]) -> App {
        let issues = specs
            .iter()
            .map(|(id, blockers, closed)| {
                let fm = if blockers.is_empty() {
                    String::new()
                } else {
                    format!("---\nblocked-by: [{}]\n---\n", blockers.join(", "))
                };
                parse(id, Path::new("x"), &format!("{fm}# {id}\n"), *closed)
            })
            .collect();
        let mut app = App::new(PathBuf::new());
        app.set_issues(issues);
        app
    }

    fn drawn(app: &App) -> Vec<String> {
        let mark = |r: &Row| if r.cycle { " ↻" } else { "" };
        app.rows
            .iter()
            .map(|r| format!("{}{}{}", r.guide, r.id, mark(r)))
            .collect()
    }

    #[test]
    fn tree_shape() {
        let mut app = app(&[
            ("mmmmmm", &["aaaaaa", "bbbbbb"], false),
            ("aaaaaa", &["cccccc"], false),
            ("bbbbbb", &["cccccc"], false),
            ("cccccc", &[], false),
            // A cycle has no natural root; one member is promoted.
            ("xxxxxx", &["yyyyyy"], false),
            ("yyyyyy", &["xxxxxx"], false),
            // An open part of a closed milestone stays a root.
            ("zzzzzz", &["pppppp", "gggggg"], true),
            ("pppppp", &[], false),
        ]);
        assert_eq!(
            drawn(&app),
            [
                "▾ mmmmmm",
                "├─▾ aaaaaa",
                "│ └── cccccc",
                "└─▾ bbbbbb",
                "  └── cccccc",
                "  pppppp",
                "▾ xxxxxx",
                "└─▾ yyyyyy",
                "  └── xxxxxx ↻",
            ]
        );

        app.show_closed = true;
        app.rebuild();
        assert_eq!(drawn(&app).last().unwrap(), "▸ zzzzzz");
        app.list.select(Some(app.rows.len() - 1));
        app.toggle_fold();
        // gggggg has no file: shown as gone, sorted last.
        assert_eq!(
            drawn(&app)[app.rows.len() - 3..],
            ["▾ zzzzzz", "├── pppppp", "└── gggggg"]
        );
    }

    #[test]
    fn filter_keeps_ancestors_and_selection() {
        let mut app = app(&[
            ("mmmmmm", &["aaaaaa", "bbbbbb"], false),
            ("aaaaaa", &[], false),
            ("bbbbbb", &[], false),
            ("pppppp", &[], false),
        ]);
        app.list.select(Some(2)); // bbbbbb
        app.filter = "bbb".into();
        app.rebuild();
        assert_eq!(drawn(&app), ["▾ mmmmmm", "└── bbbbbb"]);
        assert_eq!(app.selected().unwrap().id, "bbbbbb");
    }

    #[test]
    fn filter_sees_through_cycles() {
        // The match under aaaaaa must stay reachable via bbbbbb's back edge.
        let mut app = app(&[
            ("rrrrrr", &["aaaaaa", "bbbbbb"], false),
            ("aaaaaa", &["bbbbbb", "cccccc"], false),
            ("bbbbbb", &["aaaaaa"], false),
            ("cccccc", &[], false),
        ]);
        app.filter = "cccccc".into();
        app.rebuild();
        assert!(drawn(&app).contains(&"└─▾ bbbbbb".to_string()));
    }

    #[test]
    fn switching_views_unfolds_to_the_selection() {
        let mut app = app(&[("mmmmmm", &["aaaaaa"], false), ("aaaaaa", &[], false)]);
        app.toggle_fold(); // mmmmmm, selected first
        assert_eq!(drawn(&app), ["▸ mmmmmm"]);
        app.switch(1); // Ready: aaaaaa only
        assert_eq!(app.selected().unwrap().id, "aaaaaa");
        app.switch(2); // back to Tree
        assert_eq!(app.selected().unwrap().id, "aaaaaa");
        assert_eq!(drawn(&app), ["▾ mmmmmm", "└── aaaaaa"]);
    }
}
