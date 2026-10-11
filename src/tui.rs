//! Interactive frontend, started by plain `pinr`.
//!
//! Elm-style: `App` is the state, `App::update` handles one `Msg` at a time
//! (pure, tested), `App::view` draws it, and `run` feeds it keys from a thread
//! and resolutions from a background task.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::cli::location;
use crate::domain::model::{Kind, Occurrence, Reference, Resolution};
use crate::infra::fs::FileSystem;
use crate::infra::http::Http;
use crate::service::{PinService, Resolutions};

/// How often the spinner advances when nothing else happens.
const TICK: Duration = Duration::from_millis(100);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const HINTS: &str = "↑↓/jk move · space select · a select all · enter pin selected · q quit";

pub enum Msg {
    Up,
    Down,
    Toggle,
    ToggleAll,
    Apply,
    Quit,
    /// Redraw and advance the spinner.
    Tick,
    Resolved(Reference, Result<Resolution, String>),
}

/// What the run loop should do after an update.
#[derive(Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
    Apply,
}

/// The state of one row, derived from its occurrence and the resolutions so far.
enum Status<'a> {
    Resolving,
    Resolved(&'a Resolution),
    Failed(&'a str),
    Pinned,
    Warning(&'a str),
}

pub struct App {
    occurrences: Vec<Occurrence>,
    resolutions: Resolutions,
    selected: Vec<bool>,
    cursor: usize,
    tick: usize,
    /// Keeps the list's scroll offset between frames.
    list: ListState,
}

impl App {
    pub fn new(occurrences: Vec<Occurrence>) -> Self {
        App {
            selected: vec![false; occurrences.len()],
            occurrences,
            resolutions: Resolutions::new(),
            cursor: 0,
            tick: 0,
            list: ListState::default(),
        }
    }

    pub fn update(&mut self, msg: Msg) -> Flow {
        match msg {
            Msg::Up => self.cursor = self.cursor.saturating_sub(1),
            Msg::Down => {
                self.cursor = (self.cursor + 1).min(self.occurrences.len().saturating_sub(1))
            }
            Msg::Toggle => {
                if self.selectable(self.cursor) {
                    self.selected[self.cursor] ^= true;
                }
            }
            Msg::ToggleAll => {
                let rows: Vec<usize> = self.rows().filter(|&i| self.selectable(i)).collect();
                let all = rows.iter().all(|&i| self.selected[i]);
                for i in rows {
                    self.selected[i] = !all;
                }
            }
            Msg::Apply if self.selected_occurrences().is_empty() => {}
            Msg::Apply => return Flow::Apply,
            Msg::Quit => return Flow::Quit,
            Msg::Tick => self.tick += 1,
            Msg::Resolved(reference, result) => {
                self.resolutions.insert(reference, result);
            }
        }
        Flow::Continue
    }

    pub fn selected_occurrences(&self) -> Vec<&Occurrence> {
        self.rows()
            .filter(|&i| self.selected[i])
            .map(|i| &self.occurrences[i])
            .collect()
    }

    fn rows(&self) -> std::ops::Range<usize> {
        0..self.occurrences.len()
    }

    fn status(&self, i: usize) -> Status<'_> {
        match &self.occurrences[i].kind {
            Kind::Warning(reason) => Status::Warning(reason),
            Kind::Ref(r) if r.is_pinned() => Status::Pinned,
            Kind::Ref(r) => match self.resolutions.get(r) {
                None => Status::Resolving,
                Some(Ok(res)) => Status::Resolved(res),
                Some(Err(e)) => Status::Failed(e),
            },
        }
    }

    fn selectable(&self, i: usize) -> bool {
        i < self.occurrences.len() && matches!(self.status(i), Status::Resolved(_))
    }

    pub fn view(&mut self, frame: &mut Frame) {
        let [main, footer] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).areas(frame.area());
        let [list, detail] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(main);

        let items: Vec<ListItem> = self.rows().map(|i| ListItem::new(self.row(i))).collect();
        let items = List::new(items)
            .block(Block::bordered().title(" pinr "))
            .highlight_style(Style::new().reversed());
        self.list
            .select((!self.occurrences.is_empty()).then_some(self.cursor));
        frame.render_stateful_widget(items, list, &mut self.list);

        let details = Paragraph::new(self.detail())
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" details "));
        frame.render_widget(details, detail);

        let footer_lines = vec![Line::from(self.summary()), Line::from(HINTS).dark_gray()];
        frame.render_widget(Paragraph::new(footer_lines), footer);
    }

    fn row(&self, i: usize) -> Line<'static> {
        let occ = &self.occurrences[i];
        let marker = match self.status(i) {
            Status::Resolving => Span::raw(format!(" {} ", SPINNER[self.tick % SPINNER.len()])),
            Status::Resolved(_) if self.selected[i] => Span::raw("[x]").bold(),
            Status::Resolved(_) => Span::raw("[ ]"),
            Status::Failed(_) => Span::raw(" ✗ ").red(),
            Status::Pinned => Span::raw(" ✓ ").green(),
            Status::Warning(_) => Span::raw(" ⚠ ").yellow(),
        };
        let what = match &occ.kind {
            Kind::Ref(r) => Span::raw(r.to_string()),
            Kind::Warning(reason) => Span::raw(reason.clone()).dark_gray(),
        };
        Line::from(vec![
            marker,
            Span::raw(" "),
            Span::raw(location(occ)).dark_gray(),
            Span::raw("  "),
            what,
        ])
    }

    fn detail(&self) -> Vec<Line<'_>> {
        let Some(occ) = self.occurrences.get(self.cursor) else {
            return vec![Line::from("No references found.")];
        };
        let mut lines = vec![
            Line::from(location(occ)).bold(),
            Line::from(format!("ecosystem  {}", occ.ecosystem.name())),
        ];
        if let Some(r) = occ.reference() {
            lines.push(Line::from(format!("reference  {r}")));
        }
        lines.push(Line::default());
        match self.status(self.cursor) {
            Status::Resolving => lines.push(Line::from("resolving…")),
            Status::Resolved(res) => {
                lines.push(Line::from("resolved"));
                lines.push(Line::from(format!("→ {res}")).green());
            }
            Status::Failed(e) => lines.push(Line::from(format!("error: {e}")).red()),
            Status::Pinned => lines.push(Line::from("✓ already pinned").green()),
            Status::Warning(reason) => {
                lines.push(Line::from(format!("⚠ can't be pinned: {reason}")).yellow())
            }
        }
        lines
    }

    fn summary(&self) -> String {
        let count =
            |pred: fn(&Status) -> bool| self.rows().filter(|&i| pred(&self.status(i))).count();
        let warnings = count(|s| matches!(s, Status::Warning(_)));
        format!(
            "{} refs · {} resolving · {} errors · {warnings} warnings · {} selected",
            self.occurrences.len() - warnings,
            count(|s| matches!(s, Status::Resolving)),
            count(|s| matches!(s, Status::Failed(_))),
            self.selected_occurrences().len(),
        )
    }
}

fn key_msg(key: KeyEvent) -> Option<Msg> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    Some(match key.code {
        KeyCode::Up | KeyCode::Char('k') => Msg::Up,
        KeyCode::Down | KeyCode::Char('j') => Msg::Down,
        KeyCode::Char(' ') => Msg::Toggle,
        KeyCode::Char('a') => Msg::ToggleAll,
        KeyCode::Enter => Msg::Apply,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Msg::Quit,
        KeyCode::Char('q') | KeyCode::Esc => Msg::Quit,
        _ => return None,
    })
}

/// Reads terminal events on a plain thread, sending a `Tick` when idle.
/// Ends once the receiver is gone.
fn spawn_input(tx: UnboundedSender<Msg>) {
    std::thread::spawn(move || {
        loop {
            let msg = match event::poll(TICK) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) => key_msg(key),
                    Ok(_) => Some(Msg::Tick),
                    Err(_) => return,
                },
                Ok(false) => Some(Msg::Tick),
                Err(_) => return,
            };
            if let Some(msg) = msg
                && tx.send(msg).is_err()
            {
                return;
            }
        }
    });
}

/// Resolves references in the background, sending results as they arrive.
fn spawn_resolver<F: FileSystem + 'static, H: Http + 'static>(
    service: Arc<PinService<F, H>>,
    occurrences: Vec<Occurrence>,
    tx: UnboundedSender<Msg>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut results = std::pin::pin!(service.resolve_each(&occurrences));
        while let Some((reference, result)) = results.next().await {
            if tx.send(Msg::Resolved(reference, result)).is_err() {
                return;
            }
        }
    })
}

/// Pins the selected occurrences. Returns (references, files) changed.
fn pin_selected<F: FileSystem, H: Http>(
    service: &PinService<F, H>,
    app: &App,
) -> Result<(usize, usize)> {
    let targets = app.selected_occurrences();
    let edits = service.plan(&targets, &app.resolutions)?;
    let files = service.apply(&edits)?;
    Ok((targets.len(), files))
}

pub async fn run<F: FileSystem + 'static, H: Http + 'static>(
    service: Arc<PinService<F, H>>,
    occurrences: Vec<Occurrence>,
) -> Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    let resolver = spawn_resolver(service.clone(), occurrences.clone(), tx.clone());
    let mut app = App::new(occurrences);

    // `init` also installs a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    spawn_input(tx);
    // Takes `rx`, so the input thread stops reading keys once the loop ends.
    let flow = event_loop(&mut terminal, &mut app, rx).await;
    ratatui::restore();
    resolver.abort();

    if flow? == Flow::Apply {
        let (references, files) = pin_selected(&service, &app)?;
        println!("pinned {references} references in {files} files");
    }
    Ok(())
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    mut rx: UnboundedReceiver<Msg>,
) -> Result<Flow> {
    loop {
        terminal.draw(|frame| app.view(frame))?;
        let Some(msg) = rx.recv().await else {
            return Ok(Flow::Quit);
        };
        match app.update(msg) {
            Flow::Continue => {}
            flow => return Ok(flow),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::Config;
    use crate::domain::model::{Ecosystem, GitRef};
    use crate::mock::{MockFs, MockHttp};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const SHA: &str = "11bd71901bbe5b1630ceea73d27597364c9af683";

    fn git(repo: &str, rev: &str) -> Reference {
        Reference::Git(GitRef {
            repo_url: format!("https://github.com/{repo}"),
            rev: rev.into(),
        })
    }

    /// `repo@rev` in `src` at `path`; the span covers the revision.
    fn occ(path: &str, src: &str, repo: &str, rev: &str) -> Occurrence {
        let start = src.find(&format!("{repo}@{rev}")).unwrap() + repo.len() + 1;
        Occurrence {
            path: path.into(),
            ecosystem: Ecosystem::GithubActions,
            line: 1,
            span: start..start + rev.len(),
            raw: rev.into(),
            kind: Kind::Ref(git(repo, rev)),
        }
    }

    fn checkout(path: &str) -> Occurrence {
        occ(path, "uses: actions/checkout@v4", "actions/checkout", "v4")
    }

    fn warning(path: &str) -> Occurrence {
        Occurrence {
            path: path.into(),
            ecosystem: Ecosystem::Dockerfile,
            line: 1,
            span: 0..0,
            raw: String::new(),
            kind: Kind::Warning("variable in image".into()),
        }
    }

    fn resolved(repo: &str, rev: &str) -> Msg {
        Msg::Resolved(
            git(repo, rev),
            Ok(Resolution::Git {
                sha: SHA.into(),
                tag: "v4.2.2".into(),
            }),
        )
    }

    /// Rows: unpinned checkout (resolved), pinned, warning, unpinned setup-node (resolving).
    fn app() -> App {
        let mut app = App::new(vec![
            checkout("a.yml"),
            occ(
                "a.yml",
                &format!("uses: actions/cache@{SHA}"),
                "actions/cache",
                SHA,
            ),
            warning("Dockerfile"),
            occ(
                "b.yml",
                "uses: actions/setup-node@v4",
                "actions/setup-node",
                "v4",
            ),
        ]);
        app.update(resolved("actions/checkout", "v4"));
        app
    }

    #[test]
    fn navigation_stays_in_bounds() {
        let mut app = app();
        app.update(Msg::Up);
        assert_eq!(app.cursor, 0);
        for _ in 0..10 {
            app.update(Msg::Down);
        }
        assert_eq!(app.cursor, 3);

        let mut empty = App::new(Vec::new());
        empty.update(Msg::Down);
        empty.update(Msg::Toggle);
        empty.update(Msg::ToggleAll);
        assert_eq!(empty.cursor, 0);
        assert_eq!(empty.update(Msg::Apply), Flow::Continue);
    }

    #[test]
    fn only_resolved_rows_toggle() {
        let mut app = app();
        for _ in 0..4 {
            app.update(Msg::Toggle);
            app.update(Msg::Toggle);
            app.update(Msg::Toggle);
            app.update(Msg::Down);
        }
        assert_eq!(app.selected, [true, false, false, false]);
    }

    #[test]
    fn select_all_toggles_resolved_rows() {
        let mut app = app();
        app.update(Msg::ToggleAll);
        assert_eq!(app.selected, [true, false, false, false]);

        app.update(resolved("actions/setup-node", "v4"));
        app.update(Msg::ToggleAll);
        assert_eq!(app.selected, [true, false, false, true]);
        app.update(Msg::ToggleAll);
        assert_eq!(app.selected, [false; 4]);
    }

    #[test]
    fn resolution_messages_update_rows() {
        let mut app = app();
        assert!(matches!(app.status(3), Status::Resolving));
        assert!(matches!(app.status(1), Status::Pinned));
        assert!(matches!(app.status(2), Status::Warning(_)));

        app.update(Msg::Resolved(
            git("actions/setup-node", "v4"),
            Err("not found".into()),
        ));
        assert!(matches!(app.status(3), Status::Failed("not found")));
        assert!(!app.selectable(3));
        assert!(
            app.summary()
                .starts_with("3 refs · 0 resolving · 1 errors · 1 warnings")
        );
    }

    #[test]
    fn enter_applies_only_with_a_selection() {
        let mut app = app();
        assert_eq!(app.update(Msg::Apply), Flow::Continue);
        app.update(Msg::Toggle);
        assert_eq!(app.update(Msg::Apply), Flow::Apply);
        assert_eq!(app.update(Msg::Quit), Flow::Quit);
    }

    /// Lets a test look at the files after handing them to the service.
    struct SharedFs(Arc<MockFs>);

    impl FileSystem for SharedFs {
        fn read(&self, path: &Path) -> Result<String> {
            self.0.read(path)
        }

        fn write(&self, path: &Path, contents: &str) -> Result<()> {
            self.0.write(path, contents)
        }
    }

    #[test]
    fn pins_only_selected_occurrences() {
        let fs = Arc::new(MockFs::new(&[
            ("a.yml", "uses: actions/checkout@v4"),
            ("b.yml", "uses: actions/checkout@v4"),
        ]));
        let service = PinService::new(SharedFs(fs.clone()), MockHttp::new(), Config::default());
        let mut app = App::new(vec![checkout("a.yml"), checkout("b.yml")]);
        app.update(resolved("actions/checkout", "v4"));
        app.update(Msg::Down);
        app.update(Msg::Toggle);

        assert_eq!(pin_selected(&service, &app).unwrap(), (1, 1));
        assert_eq!(fs.get("a.yml"), "uses: actions/checkout@v4");
        assert_eq!(
            fs.get("b.yml"),
            format!("uses: actions/checkout@{SHA} # v4.2.2")
        );
    }

    #[tokio::test]
    async fn resolves_each_unique_reference_once() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let service = Arc::new(PinService::new(
            MockFs::default(),
            MockHttp::new(),
            Config::default(),
        ));
        let occurrences = app().occurrences;
        let extra = checkout("c.yml");
        let occurrences = [occurrences, vec![extra]].concat();
        spawn_resolver(service, occurrences, tx);

        let mut resolved = Vec::new();
        while let Some(Msg::Resolved(r, _)) = rx.recv().await {
            resolved.push(r.to_string());
        }
        resolved.sort();
        assert_eq!(
            resolved,
            [
                "github.com/actions/checkout@v4",
                "github.com/actions/setup-node@v4"
            ]
        );
    }

    #[test]
    fn renders_rows_details_and_summary() {
        let mut app = app();
        app.update(Msg::Toggle);
        let mut terminal = Terminal::new(TestBackend::new(140, 10)).unwrap();
        terminal.draw(|frame| app.view(frame)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        for expected in [
            "[x] a.yml:1  github.com/actions/checkout@v4",
            " ✓ ",
            " ⚠ ",
            "→ 11bd7190",
            "1 selected",
            "enter pin selected",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}");
        }
    }
}
