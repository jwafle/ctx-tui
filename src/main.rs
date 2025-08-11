use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    layout::Margin,
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
struct Args {
    // project dir, defaults to current dir
    #[arg(short, long, default_value = ".")]
    path: String,
}

#[derive(Clone, Debug)]
enum Kind {
    Dir,
    File,
}

#[derive(Clone, Debug)]
struct Node {
    path: PathBuf,
    name: String,
    kind: Kind,
    depth: usize,
    parent: Option<usize>,
    children: Vec<usize>,
    expanded: bool,
}

struct Tree {
    nodes: Vec<Node>,
    index_by_path: HashMap<PathBuf, usize>,
    cursor: usize,
    included: HashSet<PathBuf>,
}

impl Tree {
    fn from_path(root: &Path) -> Result<Self> {
        let root = root.canonicalize().context("bad path")?;
        let mut t = Tree {
            nodes: Vec::new(),
            index_by_path: HashMap::new(),
            cursor: 0,
            included: HashSet::new(),
        };
        let root_idx = t.add_node(&root, None, 0, Kind::Dir);
        let mut parent_stack: Vec<(usize, PathBuf)> = vec![(root_idx, root.clone())];

        for entry in WalkDir::new(&root).min_depth(1) {
            let e = entry?;
            let p = e.path().to_path_buf();
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if name.starts_with(".git") {
                    if e.file_type().is_dir() {
                        continue;
                    }
                }
            }
            let depth = e.depth();
            let kind = if e.file_type().is_dir() {
                Kind::Dir
            } else {
                Kind::File
            };

            while !parent_stack.is_empty() && parent_stack.len() > depth {
                parent_stack.pop();
            }
            let parent_idx = parent_stack.last().map(|(i, _)| *i);
            let idx = t.add_node(&p, parent_idx, depth, kind.clone());
            if matches!(kind, Kind::Dir) {
                parent_stack.push((idx, p));
            }
        }
        if let Some(root) = t.nodes.get_mut(root_idx) {
            root.expanded = true;
        }
        Ok(t)
    }

    fn add_node(&mut self, path: &Path, parent: Option<usize>, depth: usize, kind: Kind) -> usize {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        let idx = self.nodes.len();
        self.nodes.push(Node {
            path: path.to_path_buf(),
            name,
            kind,
            depth,
            parent,
            children: vec![],
            expanded: false,
        });
        self.index_by_path.insert(path.to_path_buf(), idx);
        if let Some(pi) = parent {
            self.nodes[pi].children.push(idx);
        }
        idx
    }

    fn visible_indices(&self) -> Vec<usize> {
        fn walk(acc: &mut Vec<usize>, nodes: &Vec<Node>, i: usize) {
            acc.push(i);
            if nodes[i].expanded {
                for &ch in &nodes[i].children {
                    walk(acc, nodes, ch);
                }
            }
        }
        let mut out = Vec::new();
        if !self.nodes.is_empty() {
            walk(&mut out, &self.nodes, 0);
        }
        out
    }

    fn move_cursor(&mut self, delta: isize) {
        let vis = self.visible_indices();
        if vis.is_empty() {
            return;
        }
        let cur_vis_pos = vis.iter().position(|&i| i == self.cursor).unwrap_or(0);
        let n = vis.len() as isize;
        let mut next = cur_vis_pos as isize + delta;
        if next < 0 {
            next = 0;
        }
        if next >= n {
            next = n - 1;
        }
        self.cursor = vis[next as usize];
    }

    fn expand(&mut self) {
        if let Some(n) = self.nodes.get_mut(self.cursor) {
            if matches!(n.kind, Kind::Dir) {
                n.expanded = true;
            }
        }
    }

    fn collapse(&mut self) {
        if let Some(n) = self.nodes.get_mut(self.cursor) {
            if matches!(n.kind, Kind::Dir) {
                n.expanded = false;
            } else if let Some(parent) = n.parent {
                self.cursor = parent;
            }
        }
    }

    fn toggle_include(&mut self) {
        let p = self.nodes[self.cursor].path.clone();
        if !self.included.insert(p.clone()) {
            self.included.remove(&p);
        }
    }

    fn toggle_include_subtree(&mut self) {
        let root = self.cursor;
        let make_included = !self.included.contains(&self.nodes[root].path);
        fn walk(nodes: &Vec<Node>, i: usize, acc: &mut Vec<PathBuf>) {
            acc.push(nodes[i].path.clone());
            for &ch in &nodes[i].children {
                walk(nodes, ch, acc);
            }
        }
        let mut paths = Vec::new();
        walk(&self.nodes, root, &mut paths);
        for p in paths {
            if make_included {
                self.included.insert(p);
            } else {
                self.included.remove(&p);
            }
        }
    }

    fn toggle_expand_at(&mut self, idx: usize) {
        if let Some(n) = self.nodes.get_mut(idx) {
            if matches!(n.kind, Kind::Dir) {
                n.expanded = !n.expanded;
            }
        }
    }
}

struct App {
    running: bool,
    tree: Tree,
    path: PathBuf,
    last_size: Rect,
}

impl App {
    fn new(path: String) -> Result<Self> {
        let abs = fs::canonicalize(&path).unwrap_or_else(|_| PathBuf::from(&path));
        let tree = Tree::from_path(&abs)?;

        Ok(Self {
            running: true,
            tree,
            path: abs,
            last_size: Rect::default(),
        })
    }

    fn update(&mut self, ev: Event) -> Result<()> {
        match ev {
            Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Char('q') | KeyCode::Esc => self.running = false,
                KeyCode::Up | KeyCode::Char('k') => self.tree.move_cursor(-1),
                KeyCode::Down | KeyCode::Char('j') => self.tree.move_cursor(1),
                KeyCode::Left | KeyCode::Char('h') => self.tree.collapse(),
                KeyCode::Right | KeyCode::Char('l') => self.tree.expand(),
                KeyCode::Char(' ') => self.tree.toggle_include(),
                KeyCode::Char('a') => self.tree.toggle_include_subtree(),
                _ => {}
            },
            Event::Mouse(m) => {
                let (files_area, _details_area, _footer_area) = self.layout(self.last_size);
                let inner = files_area.inner(Margin {
                    horizontal: 1,
                    vertical: 1,
                });

                match m.kind {
                    MouseEventKind::ScrollUp => self.tree.move_cursor(-1),
                    MouseEventKind::ScrollDown => self.tree.move_cursor(1),
                    MouseEventKind::Down(MouseButton::Left) => {
                        // Is the click inside the files list?
                        if m.column >= inner.x
                            && m.column < inner.x + inner.width
                            && m.row >= inner.y
                            && m.row < inner.y + inner.height
                        {
                            // Which row got clicked?
                            let row = (m.row - inner.y) as usize;
                            let vis = self.tree.visible_indices();
                            if row < vis.len() {
                                let idx = vis[row];
                                self.tree.cursor = idx; // select row

                                // inside MouseEventKind::Down(MouseButton::Left) where you have `idx`:
                                let n = &self.tree.nodes[idx];

                                // constants that match the text you render
                                const GUTTER_W: u16 = 2; // "> " or "  "
                                const INDENT_STEP: u16 = 2; // "  " per depth
                                const EXPANDER_W: u16 = 2; // "▸ " or "▾ "
                                const CHECKBOX_W: u16 = 3; // "[ ]" or "[x]"

                                let indent_cols = n.depth as u16 * INDENT_STEP;
                                let expander_x = inner.x + GUTTER_W + indent_cols;
                                let checkbox_x = expander_x + EXPANDER_W;

                                // hit-tests (end is exclusive)
                                let on_expander =
                                    m.column >= expander_x && m.column < expander_x + EXPANDER_W;
                                let on_checkbox =
                                    m.column >= checkbox_x && m.column < checkbox_x + CHECKBOX_W;

                                if on_expander {
                                    self.tree.toggle_expand_at(idx);
                                } else if on_checkbox {
                                    match n.kind {
                                        Kind::Dir => self.tree.toggle_include_subtree(),
                                        Kind::File => self.tree.toggle_include(),
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    // layout returns (files_area, details_area, footer_area)
    fn layout(&self, size: Rect) -> (Rect, Rect, Rect) {
        let root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(2)])
            .split(size);
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(50), Constraint::Min(1)])
            .split(root[0]);

        (body[0], body[1], root[1])
    }
}

fn ui(frame: &mut Frame, app: &App) {
    // layout: [sidebar | main] above [footer]
    let (files_area, details_area, footer_area) = app.layout(frame.area());

    let mut buf = String::new();
    let vis = app.tree.visible_indices();
    for (_row, &idx) in vis.iter().enumerate() {
        let n = &app.tree.nodes[idx];
        let is_cursor = idx == app.tree.cursor;
        let indent = "  ".repeat(n.depth);
        let expander = match (&n.kind, n.expanded) {
            (Kind::Dir, true) => "▾ ",
            (Kind::Dir, false) => "▸ ",
            (Kind::File, _) => "  ",
        };
        let checked = if app.tree.included.contains(&n.path) {
            "[x]"
        } else {
            "[ ]"
        };
        let line = format!("{indent}{expander}{checked} {}", n.name);
        if is_cursor {
            buf.push_str(&format!("> {line}\n"));
        } else {
            buf.push_str(&format!("  {line}\n"));
        }
    }
    let sidebar = Paragraph::new(buf).block(
        Block::default()
            .title("Files (↑↓ ←/→ expand/collapse, space select, a subtree)")
            .borders(Borders::ALL),
    );
    frame.render_widget(sidebar, files_area);

    let details = Paragraph::new("Details (token counts coming next)")
        .block(Block::default().title("Details").borders(Borders::ALL));
    frame.render_widget(details, details_area);

    let footer = Paragraph::new(format!("Path: {} | q: quit", app.path.display()))
        .block(Block::default().borders(Borders::TOP));
    frame.render_widget(footer, footer_area);
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut app = App::new(args.path)?;

    // setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    while app.running {
        let sz = terminal.size()?;
        app.last_size = Rect::new(0, 0, sz.width, sz.height);
        terminal.draw(|f| ui(f, &app))?;
        if event::poll(std::time::Duration::from_millis(100))? {
            let ev = event::read()?;
            app.update(ev)?;
        }
    }

    // restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}
