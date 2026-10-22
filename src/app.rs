use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc;

use crate::config::{self, Settings, Token};
use crate::input::Input;
use crate::picker::Picker;
use crate::usos::{
    Course, GradeEntry, GradeValue, Group, Kind, NewGrade, NewPoints, Node, PointEntry, ScaleValue,
    Tree, Usos,
};

/// USOS rejects longer comments.
pub const COMMENT_MAX: usize = 300;

pub enum Msg {
    Input(Event),
    Tick,
    AuthUrl(Result<(String, Token)>),
    AuthDone(Result<Token>),
    Courses(Result<Vec<Course>>),
    Tree {
        epoch: u64,
        result: Result<Tree>,
    },
    Groups {
        epoch: u64,
        result: Result<Vec<Group>>,
    },
    Scale {
        id: String,
        result: Result<Vec<ScaleValue>>,
    },
    Saved {
        epoch: u64,
        node: u64,
        student: String,
        saved: Saved,
        result: Result<()>,
    },
}

/// What a successful save changed, applied to the local copy without a reload.
pub enum Saved {
    Points(PointEntry),
    Grade(GradeEntry),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Setup,
    Auth,
    Courses,
    Tree,
    NodeStudents,
    Students,
    StudentNodes,
    Form,
    Groups,
}

pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(v) => Some(v),
            _ => None,
        }
    }
}

pub struct Setup {
    /// Base URL, consumer key, consumer secret, language.
    pub fields: [Input; 4],
    pub focus: usize,
    pub error: Option<String>,
    /// Where Esc returns to when the user came here to change working settings.
    back: Option<Screen>,
}

pub const LANGUAGE_FIELD: usize = 3;

pub const SETUP_LABELS: [&str; 4] = [
    "USOS API address",
    "Consumer key",
    "Consumer secret",
    "Names in",
];

impl Setup {
    /// USOS API's page for registering an application and getting a key.
    pub fn developers_url(&self) -> String {
        format!(
            "{}/developers/",
            self.fields[0].text.trim().trim_end_matches('/')
        )
    }
}

impl Setup {
    fn new(existing: Option<&Settings>) -> Self {
        let mut fields: [Input; 4] = Default::default();
        match existing {
            Some(s) => {
                fields[0].set(s.base_url.clone());
                fields[1].set(s.consumer_key.clone());
                fields[2].set(s.consumer_secret.clone());
                fields[3].set(s.language.clone());
            }
            None => {
                fields[0].set(config::DEFAULT_BASE_URL);
                fields[3].set("pl");
            }
        }
        Self {
            fields,
            focus: if existing.is_some() { 0 } else { 1 },
            error: None,
            back: None,
        }
    }
}

#[derive(Default)]
pub struct Auth {
    pub url: Option<String>,
    pub request: Option<Token>,
    pub pin: Input,
    pub busy: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Value,
    Comment,
    Private,
}

pub struct Form {
    pub node: usize,
    pub student: String,
    /// The list the form was opened from, and returns to.
    pub back: Screen,
    pub points: Input,
    /// Index into the grade choices; 0 is "no grade".
    pub grade: usize,
    pub comment: Input,
    pub private: Input,
    pub focus: Field,
    initial: (String, usize, String, String),
    /// `last_changed` of the entry when the form opened.
    pub base: Option<String>,
    pub saving: bool,
    pub confirm_discard: bool,
    pub error: Option<String>,
}

impl Form {
    pub fn dirty(&self) -> bool {
        (
            self.points.text.trim(),
            self.grade,
            self.comment.text.trim(),
            self.private.text.trim(),
        ) != (
            self.initial.0.as_str(),
            self.initial.1,
            self.initial.2.as_str(),
            self.initial.3.as_str(),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub at: Instant,
}

pub enum Action {
    None,
    Quit,
}

pub struct App {
    usos: Option<Usos>,
    tx: mpsc::UnboundedSender<Msg>,
    pub screen: Screen,
    pub setup: Setup,
    pub auth: Auth,
    pub courses: Load<Vec<Course>>,
    pub course_picker: Picker,
    pub course: Option<usize>,
    pub tree: Load<Tree>,
    pub tree_picker: Picker,
    /// Index into the tree's nodes.
    pub node: Option<usize>,
    pub groups: Load<Vec<Group>>,
    pub group_picker: Picker,
    /// Index into `groups`; None shows every student.
    pub group: Option<usize>,
    groups_back: Screen,
    pub student_picker: Picker,
    pub student: Option<String>,
    pub student_node_picker: Picker,
    /// Folders shown open; every folder starts folded.
    pub expanded: HashSet<u64>,
    pub scales: HashMap<String, Load<Vec<ScaleValue>>>,
    pub form: Option<Form>,
    pub toast: Option<Toast>,
    pub frame: usize,
    /// Bumped whenever a course is (re)loaded, so late results for an old load are dropped.
    epoch: u64,
    reloading: bool,
}

impl App {
    pub fn new(
        usos: Option<Usos>,
        settings: Option<Settings>,
        tx: mpsc::UnboundedSender<Msg>,
    ) -> Self {
        let mut app = Self {
            usos,
            tx,
            screen: Screen::Courses,
            setup: Setup::new(settings.as_ref()),
            auth: Auth::default(),
            courses: Load::Loading,
            course_picker: Picker::default(),
            course: None,
            tree: Load::Loading,
            tree_picker: Picker::default(),
            node: None,
            groups: Load::Loading,
            group_picker: Picker::default(),
            group: None,
            groups_back: Screen::Tree,
            student_picker: Picker::default(),
            student: None,
            student_node_picker: Picker::default(),
            expanded: HashSet::new(),
            scales: HashMap::new(),
            form: None,
            toast: None,
            frame: 0,
            epoch: 0,
            reloading: false,
        };
        match &app.usos {
            None => app.screen = Screen::Setup,
            Some(u) if u.needs_auth() => app.start_auth(),
            Some(_) => app.load_courses(),
        }
        app
    }

    pub fn is_mock(&self) -> bool {
        self.usos.as_ref().is_some_and(Usos::is_mock)
    }

    fn usos(&self) -> Usos {
        self.usos.clone().expect("set up before any USOS call")
    }

    fn spawn(&self, fut: impl Future<Output = Msg> + Send + 'static) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(fut.await);
        });
    }

    fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            kind,
            at: Instant::now(),
        });
    }

    // ----- what the lists show -----

    pub fn current_course(&self) -> Option<&Course> {
        self.courses.ready()?.get(self.course?)
    }

    pub fn current_node(&self) -> Option<&Node> {
        self.tree.ready()?.nodes.get(self.node?)
    }

    pub fn current_group(&self) -> Option<&Group> {
        self.groups.ready()?.get(self.group?)
    }

    pub fn student_name(&self, id: &str) -> String {
        self.tree
            .ready()
            .and_then(|t| t.students.iter().find(|s| s.id == id))
            .map(|s| {
                format!("{} {}", s.last_name, s.first_name)
                    .trim_end()
                    .to_string()
            })
            .unwrap_or_else(|| id.to_string())
    }

    pub fn course_items(&self) -> Vec<String> {
        self.courses
            .ready()
            .map(|cs| {
                cs.iter()
                    .map(|c| format!("{} — {}", c.course_name, c.test_name))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Indices of the nodes listed in the tree view; the root is the course itself.
    pub fn listed_nodes(&self) -> Vec<usize> {
        self.tree
            .ready()
            .map(|t| {
                t.nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| !matches!(n.kind, Kind::Root))
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn node_items(&self) -> Vec<String> {
        let Some(tree) = self.tree.ready() else {
            return Vec::new();
        };
        self.listed_nodes()
            .into_iter()
            .map(|i| {
                let n = &tree.nodes[i];
                let marker = match n.kind {
                    Kind::Folder if self.expanded.contains(&n.id) => "▾ ",
                    Kind::Folder => "▸ ",
                    _ => "  ",
                };
                format!(
                    "{}{marker}{}",
                    "  ".repeat(n.depth.saturating_sub(1)),
                    n.name
                )
            })
            .collect()
    }

    /// The outermost folded folder above `node` (indices into the tree's nodes), or
    /// None when the node is visible.
    fn folded_ancestor(&self, node: usize) -> Option<usize> {
        let tree = self.tree.ready()?;
        let mut outer = None;
        let mut parent = tree.nodes[node].parent;
        while let Some(id) = parent {
            let i = tree.nodes.iter().position(|n| n.id == id)?;
            if matches!(tree.nodes[i].kind, Kind::Root) {
                break;
            }
            if !self.expanded.contains(&id) {
                outer = Some(i);
            }
            parent = tree.nodes[i].parent;
        }
        outer
    }

    /// Hides rows inside folded folders from both tree lists. A filter searches the
    /// whole tree, so folding applies only while it is empty.
    fn apply_folds(&mut self) {
        let listed = self.listed_nodes();
        let position = |node: usize| listed.iter().position(|&i| i == node);
        // For each listed row: None if visible, else the row of the folder hiding it.
        let hidden: Vec<Option<usize>> = listed
            .iter()
            .map(|&i| self.folded_ancestor(i).and_then(position))
            .collect();
        for picker in [&mut self.tree_picker, &mut self.student_node_picker] {
            if !picker.query.is_empty() {
                continue;
            }
            let selected = picker.selected_index();
            picker
                .matches
                .retain(|m| hidden.get(m.index).is_some_and(Option::is_none));
            let target = selected.map(|i| hidden.get(i).copied().flatten().unwrap_or(i));
            match target {
                Some(t) if picker.matches.iter().any(|m| m.index == t) => picker.select_item(t),
                _ => picker.selected = picker.selected.min(picker.matches.len().saturating_sub(1)),
            }
        }
    }

    /// Opens or closes the folder at `node`; returns false if it is not a folder.
    fn toggle_fold(&mut self, node: usize, open: Option<bool>) -> bool {
        let Some(n) = self.tree.ready().map(|t| &t.nodes[node]) else {
            return false;
        };
        if !matches!(n.kind, Kind::Folder) {
            return false;
        }
        let id = n.id;
        let open = open.unwrap_or(!self.expanded.contains(&id));
        if open {
            self.expanded.insert(id);
        } else {
            self.expanded.remove(&id);
        }
        let items = self.node_items();
        self.tree_picker.refilter(&items, true);
        self.student_node_picker.refilter(&items, true);
        true
    }

    /// ←/→ on a tree row: open or close a folder; ← on anything else closes the
    /// folder it sits in.
    fn fold_key(&mut self, picker_node: Option<usize>, open: bool) {
        let Some(node) = picker_node else { return };
        if self.toggle_fold(node, Some(open)) || open {
            return;
        }
        let parent = self
            .tree
            .ready()
            .and_then(|t| t.nodes[node].parent)
            .and_then(|id| self.tree.ready()?.nodes.iter().position(|n| n.id == id));
        if let Some(p) = parent {
            self.toggle_fold(p, Some(false));
        }
    }

    /// Indices into the tree's students, limited to the chosen group.
    pub fn roster(&self) -> Vec<usize> {
        let Some(tree) = self.tree.ready() else {
            return Vec::new();
        };
        let group = self.current_group();
        tree.students
            .iter()
            .enumerate()
            .filter(|(_, s)| group.is_none_or(|g| g.people.contains(&s.id)))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn student_items(&self) -> Vec<String> {
        let Some(tree) = self.tree.ready() else {
            return Vec::new();
        };
        self.roster()
            .into_iter()
            .map(|i| {
                let s = &tree.students[i];
                format!("{} {}", s.last_name, s.first_name)
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    pub fn group_items(&self) -> Vec<String> {
        let mut items = vec!["All students".to_string()];
        if let Some(gs) = self.groups.ready() {
            items.extend(gs.iter().map(group_name));
        }
        items
    }

    pub fn group_label(&self) -> String {
        self.current_group()
            .map(group_name)
            .unwrap_or_else(|| "all students".into())
    }

    /// Whether `node` (a task or grade) has a value for `student`.
    pub fn has_value(node: &Node, student: &str) -> bool {
        match &node.kind {
            Kind::Task { points, .. } => points.get(student).is_some_and(|e| e.points.is_some()),
            Kind::Grade { grades, .. } => grades.get(student).is_some_and(|e| e.value.is_some()),
            _ => false,
        }
    }

    /// Students in the current roster with a value in `node`, out of the roster.
    pub fn graded_count(&self, node: &Node) -> (usize, usize) {
        let Some(tree) = self.tree.ready() else {
            return (0, 0);
        };
        let roster = self.roster();
        let graded = roster
            .iter()
            .filter(|&&i| Self::has_value(node, &tree.students[i].id))
            .count();
        (graded, roster.len())
    }

    /// A student's points summed over all tasks, with the most they could have.
    pub fn total(&self, student: &str) -> (f64, f64) {
        let Some(tree) = self.tree.ready() else {
            return (0.0, 0.0);
        };
        tree.nodes
            .iter()
            .fold((0.0, 0.0), |(got, max), n| match &n.kind {
                Kind::Task { max: m, points, .. } => (
                    got + points.get(student).and_then(|e| e.points).unwrap_or(0.0),
                    max + m,
                ),
                _ => (got, max),
            })
    }

    /// Grade choices for a scale, worst first, after a leading "no grade".
    pub fn grade_choices(&self, scale: &str) -> Option<Vec<Option<&ScaleValue>>> {
        let values = self.scales.get(scale)?.ready()?;
        let mut choices: Vec<Option<&ScaleValue>> = vec![None];
        choices.extend(values.iter().rev().map(Some));
        Some(choices)
    }

    // ----- event handling -----

    pub fn handle(&mut self, msg: Msg) -> Action {
        let action = self.dispatch(msg);
        self.apply_folds();
        action
    }

    fn dispatch(&mut self, msg: Msg) -> Action {
        match msg {
            Msg::Tick => {
                self.frame = self.frame.wrapping_add(1);
                if let Some(t) = &self.toast {
                    let ttl = if t.kind == ToastKind::Error { 8 } else { 4 };
                    if t.at.elapsed() > Duration::from_secs(ttl) {
                        self.toast = None;
                    }
                }
            }
            Msg::Input(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                return self.key(key);
            }
            Msg::Input(_) => {}
            Msg::AuthUrl(result) => {
                self.auth.busy = false;
                match result {
                    Ok((url, request)) => {
                        self.auth.url = Some(url);
                        self.auth.request = Some(request);
                    }
                    Err(e) => {
                        self.auth.error =
                            Some(format!("{e:#} — check the address and API key (ctrl-k)"))
                    }
                }
            }
            Msg::AuthDone(result) => {
                self.auth.busy = false;
                match result {
                    Ok(token) => match config::save_token(&token) {
                        Ok(()) => {
                            self.toast(ToastKind::Success, "Connected to USOS");
                            if self.current_course().is_some() {
                                // Signing in again mid-session returns to the open course.
                                self.screen = Screen::Tree;
                                self.reload(false);
                            } else {
                                self.load_courses();
                            }
                        }
                        Err(e) => self.auth.error = Some(format!("could not save token: {e:#}")),
                    },
                    Err(e) => {
                        // The request token is single-use; a failed exchange needs a fresh one.
                        self.auth.error = Some(format!("{e:#} — open the new link and try again"));
                        self.auth.pin.set("");
                        self.request_auth();
                    }
                }
            }
            Msg::Courses(result) => {
                self.courses = match result {
                    Ok(cs) => Load::Ready(cs),
                    Err(e) => Load::Failed(format!("{e:#}")),
                };
                let items = self.course_items();
                self.course_picker.reset(&items);
            }
            Msg::Tree { epoch, result } if epoch == self.epoch => self.tree_arrived(result),
            Msg::Groups { epoch, result } if epoch == self.epoch => {
                self.groups = match result {
                    Ok(gs) => Load::Ready(gs),
                    Err(e) if format!("{e:#}").contains("Scope") => Load::Failed(
                        "USOS needs a fresh sign-in to list group members: \
                         press ctrl-k, then ctrl-s"
                            .into(),
                    ),
                    Err(e) => Load::Failed(format!("{e:#}")),
                };
                if self.screen == Screen::Groups {
                    let items = self.group_items();
                    self.group_picker.refilter(&items, true);
                }
            }
            Msg::Scale { id, result } => {
                let load = match result {
                    Ok(v) => Load::Ready(v),
                    Err(e) => Load::Failed(format!("{e:#}")),
                };
                self.scales.insert(id, load);
                self.prefill_grade();
            }
            Msg::Saved {
                epoch,
                node,
                student,
                saved,
                result,
            } if epoch == self.epoch => self.saved(node, student, saved, result),
            _ => {}
        }
        Action::None
    }

    fn key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        let lists = [
            Screen::Courses,
            Screen::Tree,
            Screen::NodeStudents,
            Screen::Students,
            Screen::StudentNodes,
            Screen::Groups,
        ];
        if ctrl && key.code == KeyCode::Char('k') && lists.contains(&self.screen) && !self.is_mock()
        {
            self.open_setup();
            return Action::None;
        }
        match self.screen {
            Screen::Setup => return self.key_setup(key),
            Screen::Auth => return self.key_auth(key),
            Screen::Courses => return self.key_courses(key),
            Screen::Tree => self.key_tree(key),
            Screen::NodeStudents => self.key_node_students(key),
            Screen::Students => self.key_students(key),
            Screen::StudentNodes => self.key_student_nodes(key),
            Screen::Form => self.key_form(key),
            Screen::Groups => self.key_groups(key),
        }
        Action::None
    }

    // ----- setup and sign-in -----

    /// Whether Esc on the setup screen goes back rather than quitting.
    pub fn setup_returns(&self) -> bool {
        self.setup.back.is_some() && self.usos.is_some()
    }

    /// Shows the API key and sign-in settings, filled with the current ones.
    fn open_setup(&mut self) {
        let current = config::load_settings().ok().flatten();
        self.setup = Setup::new(current.as_ref());
        self.setup.back = Some(self.screen);
        self.screen = Screen::Setup;
    }

    fn key_setup(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last = self.setup.fields.len() - 1;
        match key.code {
            KeyCode::Esc => match self.setup.back {
                Some(back) if self.usos.is_some() => self.screen = back,
                _ => return Action::Quit,
            },
            KeyCode::Char('o') if ctrl => open_in_browser(&self.setup.developers_url()),
            KeyCode::Char('s') if ctrl => self.finish_setup(),
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                if self.setup.focus == LANGUAGE_FIELD =>
            {
                let field = &mut self.setup.fields[LANGUAGE_FIELD];
                let next = if field.text == "pl" { "en" } else { "pl" };
                field.set(next);
            }
            KeyCode::Char(_) if self.setup.focus == LANGUAGE_FIELD => {}
            KeyCode::Tab | KeyCode::Down => self.setup.focus = (self.setup.focus + 1).min(last),
            KeyCode::BackTab | KeyCode::Up => self.setup.focus = self.setup.focus.saturating_sub(1),
            KeyCode::Enter if self.setup.focus < last => self.setup.focus += 1,
            KeyCode::Enter => self.finish_setup(),
            _ => {
                let focus = self.setup.focus;
                self.setup.fields[focus].handle_key(key);
                self.setup.error = None;
            }
        }
        Action::None
    }

    fn finish_setup(&mut self) {
        let [base, key, secret, language] = &self.setup.fields;
        let get = |i: &Input| i.text.trim().to_string();
        let settings = Settings {
            base_url: get(base).trim_end_matches('/').to_string(),
            consumer_key: get(key),
            consumer_secret: get(secret),
            language: get(language).to_lowercase(),
        };
        let missing = [
            &settings.base_url,
            &settings.consumer_key,
            &settings.consumer_secret,
        ]
        .iter()
        .position(|v| v.is_empty());
        if let Some(i) = missing {
            self.setup.error = Some(format!("{} is required", SETUP_LABELS[i]));
            self.setup.focus = i;
            return;
        }
        if !settings.base_url.starts_with("https://") {
            self.setup.focus = 0;
            self.setup.error = Some("The address must start with https://".into());
            return;
        }
        if !["pl", "en"].contains(&settings.language.as_str()) {
            self.setup.error = Some("Language must be pl or en".into());
            return;
        }
        if let Err(e) = config::save_settings(&settings) {
            self.setup.error = Some(format!("Could not save settings: {e:#}"));
            return;
        }
        self.usos = Some(Usos::real(settings, None));
        self.start_auth();
    }

    fn start_auth(&mut self) {
        self.screen = Screen::Auth;
        self.request_auth();
    }

    fn request_auth(&mut self) {
        self.auth.busy = true;
        self.auth.url = None;
        let usos = self.usos();
        self.spawn(async move { Msg::AuthUrl(usos.request_token().await) });
    }

    fn key_auth(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Action::Quit,
            KeyCode::Char('k') if ctrl => self.open_setup(),
            KeyCode::Char('o') if ctrl => {
                if let Some(url) = &self.auth.url {
                    open_in_browser(url);
                }
            }
            KeyCode::Enter => {
                let pin = self.auth.pin.text.trim().to_string();
                if let (false, Some(request)) =
                    (pin.is_empty() || self.auth.busy, self.auth.request.clone())
                {
                    self.auth.busy = true;
                    self.auth.error = None;
                    let usos = self.usos();
                    self.spawn(
                        async move { Msg::AuthDone(usos.access_token(&request, &pin).await) },
                    );
                }
            }
            _ => {
                self.auth.pin.handle_key(key);
            }
        }
        Action::None
    }

    // ----- courses -----

    fn load_courses(&mut self) {
        self.screen = Screen::Courses;
        self.courses = Load::Loading;
        let usos = self.usos();
        self.spawn(async move { Msg::Courses(usos.courses().await) });
    }

    fn key_courses(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let items = self.course_items();
        match key.code {
            KeyCode::Esc if !self.course_picker.query.is_empty() => {
                self.course_picker.clear_query(&items)
            }
            KeyCode::Char('r') if ctrl => self.load_courses(),
            KeyCode::Esc => return Action::Quit,
            KeyCode::Enter => {
                if let Some(i) = self.course_picker.selected_index() {
                    self.open_course(i);
                }
            }
            _ => {
                self.course_picker.handle_key(key, &items);
            }
        }
        Action::None
    }

    fn open_course(&mut self, index: usize) {
        if self.course == Some(index) && !matches!(self.tree, Load::Failed(_)) {
            self.screen = Screen::Tree;
            return;
        }
        self.course = Some(index);
        self.tree = Load::Loading;
        self.groups = Load::Loading;
        self.group = None;
        self.expanded.clear();
        self.node = None;
        self.student = None;
        self.tree_picker.reset(&[]);
        self.screen = Screen::Tree;
        self.reload(false);
        self.reloading = false;
    }

    /// Fetches the tree (structure first unless one is shown) and the user's groups.
    fn reload(&mut self, announce: bool) {
        let Some(course) = self.current_course().cloned() else {
            return;
        };
        self.epoch += 1;
        self.reloading = announce;
        if announce {
            self.toast(ToastKind::Info, "Reloading from USOS…");
        }
        let (epoch, usos) = (self.epoch, self.usos());
        if self.tree.ready().is_none() {
            let usos = usos.clone();
            self.spawn(async move {
                Msg::Tree {
                    epoch,
                    result: usos.tree(course.root, false).await,
                }
            });
        }
        let u = usos.clone();
        let root = course.root;
        self.spawn(async move {
            Msg::Tree {
                epoch,
                result: u.tree(root, true).await,
            }
        });
        self.spawn(async move {
            Msg::Groups {
                epoch,
                result: usos.groups(&course).await,
            }
        });
    }

    fn tree_arrived(&mut self, result: Result<Tree>) {
        let full = result.as_ref().is_ok_and(|t| t.loaded);
        let had_full = self.tree.ready().is_some_and(|t| t.loaded);
        if !full && (had_full || (result.is_err() && self.tree.ready().is_some())) {
            // A quick structure load lost the race to the full one; keep the full one.
            return;
        }
        let keep_node = self.current_node().map(|n| n.id);
        let had_list = self.tree.ready().is_some();
        match result {
            Ok(tree) => {
                let scales: Vec<String> = tree
                    .nodes
                    .iter()
                    .filter_map(|n| match &n.kind {
                        Kind::Grade { scale, .. } => Some(scale.clone()),
                        _ => None,
                    })
                    .collect();
                self.tree = Load::Ready(tree);
                for id in scales {
                    self.load_scale(id);
                }
                if full && std::mem::take(&mut self.reloading) {
                    self.toast(ToastKind::Success, "Reloaded from USOS");
                }
            }
            Err(e) => {
                self.reloading = false;
                self.tree = Load::Failed(format!("{e:#}"));
            }
        }
        self.node =
            keep_node.and_then(|id| self.tree.ready()?.nodes.iter().position(|n| n.id == id));
        self.refresh_lists(had_list);
        if full && !had_full && self.tree_picker.query.is_empty() {
            self.select_first_unfinished();
        }
        if self.screen == Screen::Form {
            self.prefill_grade();
        }
    }

    fn load_scale(&mut self, id: String) {
        if self.scales.contains_key(&id) {
            return;
        }
        self.scales.insert(id.clone(), Load::Loading);
        let usos = self.usos();
        self.spawn(async move {
            Msg::Scale {
                result: usos.scale(&id).await,
                id,
            }
        });
    }

    /// Re-filters every list after its contents changed, keeping each cursor in place.
    fn refresh_lists(&mut self, keep: bool) {
        let items = self.node_items();
        if keep {
            self.tree_picker.refilter(&items, true);
        } else {
            self.tree_picker.reset(&items);
        }
        self.student_node_picker.refilter(&items, true);
        let items = self.student_items();
        self.student_picker.refilter(&items, true);
    }

    // ----- tree -----

    fn key_tree(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let items = self.node_items();
        match key.code {
            KeyCode::Esc if !self.tree_picker.query.is_empty() => {
                self.tree_picker.clear_query(&items)
            }
            KeyCode::Esc => self.screen = Screen::Courses,
            KeyCode::Char('r') if ctrl => self.reload(true),
            KeyCode::Char('g') if ctrl => self.open_groups(),
            KeyCode::Tab => self.open_students(),
            KeyCode::Right | KeyCode::Left => {
                let listed = self.listed_nodes();
                let node = self.tree_picker.selected_index().map(|i| listed[i]);
                self.fold_key(node, key.code == KeyCode::Right);
            }
            KeyCode::Enter => {
                let listed = self.listed_nodes();
                if let Some(i) = self.tree_picker.selected_index().map(|i| listed[i])
                    && !self.toggle_fold(i, None)
                {
                    self.open_node(i);
                }
            }
            _ => {
                self.tree_picker.handle_key(key, &items);
            }
        }
    }

    /// Returns to the tree with its filter cleared and the cursor where it was.
    fn back_to_tree(&mut self) {
        let items = self.node_items();
        self.tree_picker.clear_query(&items);
        self.screen = Screen::Tree;
    }

    /// Puts the tree's cursor on the first task or grade not everyone has a value in.
    fn select_first_unfinished(&mut self) {
        let Some(tree) = self.tree.ready().filter(|t| t.loaded) else {
            return;
        };
        let listed = self.listed_nodes();
        let target = listed.iter().position(|&i| {
            let n = &tree.nodes[i];
            if !matches!(n.kind, Kind::Task { .. } | Kind::Grade { .. }) {
                return false;
            }
            let (graded, total) = self.graded_count(n);
            graded < total
        });
        if let Some(pos) = target {
            let shown = self
                .folded_ancestor(listed[pos])
                .and_then(|f| listed.iter().position(|&i| i == f))
                .unwrap_or(pos);
            self.apply_folds();
            self.tree_picker.select_item(shown);
        }
    }

    fn open_node(&mut self, index: usize) {
        let Some(node) = self.tree.ready().map(|t| &t.nodes[index]) else {
            return;
        };
        if matches!(node.kind, Kind::Folder | Kind::Root) {
            return;
        }
        self.node = Some(index);
        let items = self.student_items();
        self.student_picker.reset(&items);
        self.select_next_ungraded(true);
        self.screen = Screen::NodeStudents;
    }

    // ----- one node, all students -----

    fn key_node_students(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let items = self.student_items();
        match key.code {
            KeyCode::Esc if !self.student_picker.query.is_empty() => {
                self.student_picker.clear_query(&items)
            }
            KeyCode::Esc => self.back_to_tree(),
            KeyCode::Char('r') if ctrl => self.reload(true),
            KeyCode::Char('g') if ctrl => self.open_groups(),
            KeyCode::Enter => {
                if let (Some(id), Some(node)) = (self.picked_student(), self.node) {
                    self.open_form(node, id, Screen::NodeStudents);
                }
            }
            _ => {
                self.student_picker.handle_key(key, &items);
            }
        }
    }

    fn picked_student(&self) -> Option<String> {
        let roster = self.roster();
        let i = *roster.get(self.student_picker.selected_index()?)?;
        Some(self.tree.ready()?.students[i].id.clone())
    }

    /// Moves to the next student without a value in the current node; `from_top`
    /// starts at the first row instead of below the cursor.
    fn select_next_ungraded(&mut self, from_top: bool) {
        let (Some(node), Some(tree)) = (self.current_node(), self.tree.ready()) else {
            return;
        };
        let roster = self.roster();
        let n = self.student_picker.matches.len();
        if n == 0 || !tree.loaded {
            return;
        }
        let start = if from_top {
            0
        } else {
            self.student_picker.selected + 1
        };
        let next = (0..n).map(|k| (start + k) % n).find(|&pos| {
            let id = &tree.students[roster[self.student_picker.matches[pos].index]].id;
            !Self::has_value(node, id)
        });
        if let Some(pos) = next {
            self.student_picker.selected = pos;
        }
    }

    // ----- students, one student's nodes -----

    fn open_students(&mut self) {
        let items = self.student_items();
        self.student_picker.reset(&items);
        if let Some(i) = self
            .student
            .as_ref()
            .and_then(|id| self.roster_position(id))
        {
            self.student_picker.select_item(i);
        }
        self.screen = Screen::Students;
    }

    fn roster_position(&self, id: &str) -> Option<usize> {
        let tree = self.tree.ready()?;
        self.roster()
            .iter()
            .position(|&i| tree.students[i].id == id)
    }

    fn key_students(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let items = self.student_items();
        match key.code {
            KeyCode::Esc if !self.student_picker.query.is_empty() => {
                self.student_picker.clear_query(&items)
            }
            KeyCode::Esc | KeyCode::Tab => self.back_to_tree(),
            KeyCode::Char('r') if ctrl => self.reload(true),
            KeyCode::Char('g') if ctrl => self.open_groups(),
            KeyCode::Enter => {
                if let Some(id) = self.picked_student() {
                    self.student = Some(id);
                    let items = self.node_items();
                    self.student_node_picker.reset(&items);
                    self.select_next_gradable(true);
                    self.screen = Screen::StudentNodes;
                }
            }
            _ => {
                self.student_picker.handle_key(key, &items);
            }
        }
    }

    fn key_student_nodes(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let items = self.node_items();
        match key.code {
            KeyCode::Esc if !self.student_node_picker.query.is_empty() => {
                self.student_node_picker.clear_query(&items)
            }
            KeyCode::Esc => self.open_students(),
            KeyCode::Char('r') if ctrl => self.reload(true),
            KeyCode::Right | KeyCode::Left => {
                let listed = self.listed_nodes();
                let node = self.student_node_picker.selected_index().map(|i| listed[i]);
                self.fold_key(node, key.code == KeyCode::Right);
            }
            KeyCode::Enter => {
                let listed = self.listed_nodes();
                let node = self.student_node_picker.selected_index().map(|i| listed[i]);
                if let (Some(node), Some(id)) = (node, self.student.clone())
                    && !self.toggle_fold(node, None)
                {
                    self.open_form(node, id, Screen::StudentNodes);
                }
            }
            _ => {
                self.student_node_picker.handle_key(key, &items);
            }
        }
    }

    /// Moves to the next editable task or grade below the cursor that the student has
    /// no value in.
    fn select_next_gradable(&mut self, from_top: bool) {
        let (Some(tree), Some(id)) = (self.tree.ready(), self.student.as_ref()) else {
            return;
        };
        let listed = self.listed_nodes();
        let picker = &self.student_node_picker;
        let n = picker.matches.len();
        if n == 0 || !tree.loaded {
            return;
        }
        let start = if from_top { 0 } else { picker.selected + 1 };
        let next = (start..n).find(|&pos| {
            let node = &tree.nodes[listed[picker.matches[pos].index]];
            node.editable
                && matches!(node.kind, Kind::Task { .. } | Kind::Grade { .. })
                && !Self::has_value(node, id)
        });
        if let Some(pos) = next {
            self.student_node_picker.selected = pos;
        }
    }

    // ----- groups -----

    fn open_groups(&mut self) {
        self.groups_back = self.screen;
        let items = self.group_items();
        self.group_picker.reset(&items);
        self.group_picker
            .select_item(self.group.map_or(0, |g| g + 1));
        self.screen = Screen::Groups;
    }

    fn key_groups(&mut self, key: KeyEvent) {
        let items = self.group_items();
        match key.code {
            KeyCode::Esc if !self.group_picker.query.is_empty() => {
                self.group_picker.clear_query(&items)
            }
            KeyCode::Esc => self.screen = self.groups_back,
            KeyCode::Enter => {
                if let Some(i) = self.group_picker.selected_index() {
                    self.group = i.checked_sub(1);
                    let items = self.student_items();
                    self.student_picker.reset(&items);
                    if self.groups_back == Screen::NodeStudents {
                        self.select_next_ungraded(true);
                    }
                    self.screen = self.groups_back;
                }
            }
            _ => {
                self.group_picker.handle_key(key, &items);
            }
        }
    }

    // ----- the grading form -----

    fn open_form(&mut self, node: usize, student: String, back: Screen) {
        let Some(n) = self.tree.ready().map(|t| &t.nodes[node]) else {
            return;
        };
        if !self.tree.ready().is_some_and(|t| t.loaded) {
            self.toast(
                ToastKind::Info,
                "Still loading the current values from USOS…",
            );
            return;
        }
        if !n.editable {
            self.toast(ToastKind::Error, "USOS does not let you change this node");
            return;
        }
        let (points, comment, private, base) = match &n.kind {
            Kind::Task {
                precision, points, ..
            } => {
                let e = points.get(&student).cloned().unwrap_or_default();
                (
                    e.points
                        .map(|p| fmt_points(p, *precision))
                        .unwrap_or_default(),
                    e.comment.unwrap_or_default(),
                    String::new(),
                    e.last_changed,
                )
            }
            Kind::Grade { grades, .. } => {
                let e = grades.get(&student).cloned().unwrap_or_default();
                (
                    String::new(),
                    e.comment.unwrap_or_default(),
                    e.private_comment.unwrap_or_default(),
                    e.last_changed,
                )
            }
            _ => return,
        };
        let mut form = Form {
            node,
            student,
            back,
            points: Input::accepting(|c| c.is_ascii_digit() || c == '.' || c == ','),
            grade: 0,
            comment: Input::with_max(COMMENT_MAX),
            private: Input::with_max(COMMENT_MAX),
            focus: Field::Value,
            initial: Default::default(),
            base,
            saving: false,
            confirm_discard: false,
            error: None,
        };
        form.points.set(points.clone());
        form.comment.set(comment.clone());
        form.private.set(private.clone());
        form.initial = (points, 0, comment, private);
        self.form = Some(form);
        self.screen = Screen::Form;
        self.prefill_grade();
    }

    /// Selects the student's current grade once the scale is known.
    fn prefill_grade(&mut self) {
        let Some(form) = &self.form else { return };
        let Some(Kind::Grade { scale, grades }) =
            self.tree.ready().map(|t| &t.nodes[form.node].kind)
        else {
            return;
        };
        let current = grades.get(&form.student).and_then(|e| e.value.clone());
        let Some(choices) = self.grade_choices(scale) else {
            return;
        };
        let index = choices
            .iter()
            .position(|c| c.map(|v| v.order_key) == current.as_ref().map(|v| v.order_key))
            .unwrap_or(0);
        let form = self.form.as_mut().expect("checked above");
        if !form.dirty() {
            form.grade = index;
            form.initial.1 = index;
        }
    }

    pub fn form_node(&self) -> Option<&Node> {
        self.tree.ready()?.nodes.get(self.form.as_ref()?.node)
    }

    /// Validates the points field against the task's limits.
    pub fn parse_points(&self) -> Result<f64, String> {
        let (Some(form), Some(node)) = (&self.form, self.form_node()) else {
            return Err("nothing to grade".into());
        };
        let Kind::Task {
            min,
            max,
            precision,
            ..
        } = node.kind
        else {
            return Err("not a task".into());
        };
        let text = form.points.text.trim().replace(',', ".");
        if text.is_empty() {
            return Err("Enter the points first".into());
        }
        let value: f64 = text
            .parse()
            .map_err(|_| format!("“{text}” is not a number"))?;
        if value < min || value > max {
            return Err(format!(
                "Points must be between {} and {}",
                fmt_points(min, precision),
                fmt_points(max, precision)
            ));
        }
        let scale = 10f64.powi(precision as i32);
        if ((value * scale).round() - value * scale).abs() > 1e-6 {
            return Err(match precision {
                0 => "Only whole points are allowed".into(),
                1 => "At most one decimal place is allowed".into(),
                p => format!("At most {p} decimal places are allowed"),
            });
        }
        Ok(value)
    }

    fn key_form(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let is_grade = matches!(self.form_node().map(|n| &n.kind), Some(Kind::Grade { .. }));
        let choices = self
            .form_node()
            .and_then(|n| match &n.kind {
                Kind::Grade { scale, .. } => self.grade_choices(scale).map(|c| {
                    c.iter()
                        .map(|v| v.map(|v| v.symbol.clone()).unwrap_or_default())
                        .collect::<Vec<_>>()
                }),
                _ => None,
            })
            .unwrap_or_default();
        let Some(form) = self.form.as_mut() else {
            return;
        };
        if form.saving {
            return;
        }
        if form.confirm_discard {
            form.confirm_discard = false;
            if matches!(key.code, KeyCode::Char('y' | 'd')) {
                self.screen = form.back;
                self.toast(ToastKind::Info, "Changes discarded");
            }
            return;
        }
        let fields: &[Field] = if is_grade {
            &[Field::Value, Field::Comment, Field::Private]
        } else {
            &[Field::Value, Field::Comment]
        };
        let pos = fields.iter().position(|f| *f == form.focus).unwrap_or(0);
        match key.code {
            KeyCode::Esc if form.dirty() => form.confirm_discard = true,
            KeyCode::Esc => self.screen = form.back,
            KeyCode::Char('s') if ctrl => self.save(),
            KeyCode::Tab | KeyCode::Down => form.focus = fields[(pos + 1) % fields.len()],
            KeyCode::BackTab | KeyCode::Up => {
                form.focus = fields[(pos + fields.len() - 1) % fields.len()]
            }
            KeyCode::Enter if form.focus == Field::Value => form.focus = Field::Comment,
            KeyCode::Enter => {}
            KeyCode::Left if is_grade && form.focus == Field::Value => {
                form.grade = form.grade.saturating_sub(1);
                form.error = None;
            }
            KeyCode::Right if is_grade && form.focus == Field::Value => {
                form.grade = (form.grade + 1).min(choices.len().saturating_sub(1));
                form.error = None;
            }
            KeyCode::Backspace | KeyCode::Delete if is_grade && form.focus == Field::Value => {
                form.grade = 0;
            }
            KeyCode::Char(c) if is_grade && form.focus == Field::Value && !ctrl => {
                // Typing a digit picks the grade starting with it; typing it again moves
                // on to the next such grade, e.g. 4 then 4,5.
                let matching: Vec<usize> = (0..choices.len())
                    .filter(|&i| choices[i].starts_with(c))
                    .collect();
                if let Some(&first) = matching.first() {
                    form.grade = matching
                        .iter()
                        .copied()
                        .find(|&i| i > form.grade && matching.contains(&form.grade))
                        .unwrap_or(first);
                    form.error = None;
                }
            }
            _ => {
                let input = match form.focus {
                    Field::Value => &mut form.points,
                    Field::Comment => &mut form.comment,
                    Field::Private => &mut form.private,
                };
                if input.handle_key(key) {
                    form.error = None;
                }
            }
        }
    }

    fn save(&mut self) {
        let points = match self.form_node().map(|n| &n.kind) {
            Some(Kind::Task { .. }) => match self.parse_points() {
                Ok(p) => Some(p),
                Err(e) => {
                    if let Some(f) = self.form.as_mut() {
                        f.error = Some(e);
                        f.focus = Field::Value;
                    }
                    return;
                }
            },
            _ => None,
        };
        let (Some(form), Some(node)) = (&self.form, self.form_node()) else {
            return;
        };
        let comment = form.comment.text.trim().to_string();
        let (usos, epoch, node_id, student) =
            (self.usos(), self.epoch, node.id, form.student.clone());
        match &node.kind {
            Kind::Task { .. } => {
                let points = points.expect("parsed above");
                let update = NewPoints {
                    student: student.clone(),
                    points,
                    comment: comment.clone(),
                    last_changed: form.base.clone(),
                };
                let saved = Saved::Points(PointEntry {
                    points: Some(points),
                    comment: (!comment.is_empty()).then_some(comment),
                    last_changed: None,
                    grader: None,
                });
                self.spawn(async move {
                    Msg::Saved {
                        epoch,
                        node: node_id,
                        student,
                        saved,
                        result: usos.set_points(node_id, vec![update]).await,
                    }
                });
            }
            Kind::Grade { scale, .. } => {
                let Some(choices) = self.grade_choices(scale) else {
                    return;
                };
                let value = choices.get(form.grade).copied().flatten();
                let private = form.private.text.trim().to_string();
                let update = NewGrade {
                    student: student.clone(),
                    order_key: value.map(|v| v.order_key),
                    comment: comment.clone(),
                    private_comment: private.clone(),
                    last_changed: form.base.clone(),
                };
                let saved = Saved::Grade(GradeEntry {
                    value: value.map(|v| GradeValue {
                        order_key: v.order_key,
                        symbol: v.symbol.clone(),
                    }),
                    automatic: None,
                    comment: (!comment.is_empty()).then_some(comment),
                    private_comment: (!private.is_empty()).then_some(private),
                    last_changed: None,
                    grader: None,
                });
                self.spawn(async move {
                    Msg::Saved {
                        epoch,
                        node: node_id,
                        student,
                        saved,
                        result: usos.set_grades(node_id, vec![update]).await,
                    }
                });
            }
            _ => return,
        }
        if let Some(f) = self.form.as_mut() {
            f.saving = true;
            f.error = None;
        }
    }

    fn saved(&mut self, node: u64, student: String, saved: Saved, result: Result<()>) {
        if let Some(f) = self.form.as_mut() {
            f.saving = false;
        }
        if let Err(e) = result {
            let msg = format!("{e:#}");
            let text = if msg.contains("400") {
                format!(
                    "{msg}. Someone may have changed it in USOS meanwhile; \
                     press ctrl-r on the list to see the current value."
                )
            } else {
                msg
            };
            if let Some(f) = self.form.as_mut() {
                f.error = Some(text);
            }
            return;
        }
        // The stored version stamp is unknown until a reload; keep the old one out so a
        // second edit is refused by USOS rather than silently overwriting.
        let mut summary = String::new();
        if let Load::Ready(tree) = &mut self.tree
            && let Some(n) = tree.nodes.iter_mut().find(|n| n.id == node)
        {
            match (&mut n.kind, saved) {
                (
                    Kind::Task {
                        points,
                        precision,
                        max,
                        ..
                    },
                    Saved::Points(mut e),
                ) => {
                    summary = format!(
                        "{} / {}",
                        fmt_points(e.points.unwrap_or(0.0), *precision),
                        fmt_points(*max, *precision)
                    );
                    e.last_changed = Some("unknown — reload to edit again".into());
                    e.grader = Some("you".into());
                    points.insert(student.clone(), e);
                }
                (Kind::Grade { grades, .. }, Saved::Grade(mut e)) => {
                    summary = e
                        .value
                        .as_ref()
                        .map(|v| v.symbol.clone())
                        .unwrap_or_else(|| "no grade".into());
                    e.last_changed = Some("unknown — reload to edit again".into());
                    e.grader = Some("you".into());
                    grades.insert(student.clone(), e);
                }
                _ => {}
            }
        }
        let who = self.student_name(&student);
        self.toast(ToastKind::Success, format!("Saved {summary} for {who}"));
        let back = self.form.as_ref().map_or(Screen::Tree, |f| f.back);
        self.form = None;
        self.screen = back;
        // The filter found this row; clearing it readies the list for the next name.
        match back {
            Screen::NodeStudents => {
                let items = self.student_items();
                self.student_picker.clear_query(&items);
                self.select_next_ungraded(false);
            }
            Screen::StudentNodes => {
                let items = self.node_items();
                self.student_node_picker.clear_query(&items);
                self.select_next_gradable(false);
            }
            _ => {}
        }
        self.reload_quietly();
    }

    /// Refreshes the tree after a save so version stamps are current for the next edit.
    fn reload_quietly(&mut self) {
        let Some(course) = self.current_course().cloned() else {
            return;
        };
        let (epoch, usos) = (self.epoch, self.usos());
        self.spawn(async move {
            Msg::Tree {
                epoch,
                result: usos.tree(course.root, true).await,
            }
        });
    }
}

/// How a group is listed, e.g. "Laboratorium, group 1".
pub fn group_name(g: &Group) -> String {
    let mut kind = g.class_type.clone();
    if let Some(first) = kind.get(..1) {
        kind = first.to_uppercase() + &kind[1..];
    }
    format!("{kind}, group {}", g.number)
}

pub fn fmt_points(p: f64, precision: u32) -> String {
    let s = format!("{p:.*}", precision as usize);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn open_in_browser(url: &str) {
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
