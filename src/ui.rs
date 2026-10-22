use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, HighlightSpacing, Padding, Paragraph, Row, Table, Wrap,
};

use crate::app::{
    App, COMMENT_MAX, Field, LANGUAGE_FIELD, Load, SETUP_LABELS, Screen, ToastKind, fmt_points,
};
use crate::config;
use crate::input::Input;
use crate::picker::Picker;
use crate::usos::{Kind, Node};

const ACCENT: Color = Color::Cyan;
const ACCENT2: Color = Color::Magenta;
const DIM: Color = Color::DarkGray;
const SOFT: Color = Color::Gray;
const OK: Color = Color::Green;
const WARN: Color = Color::Yellow;
const ERR: Color = Color::Red;
const SELECTED: Color = Color::Indexed(236);
const TRACK: Color = Color::Indexed(238);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Wider than this, lists become hard to read across, so the body is centred instead.
const MAX_BODY_WIDTH: u16 = 120;

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, body, status, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_header(f, header, app);
    let [body] = Layout::horizontal([Constraint::Max(MAX_BODY_WIDTH)])
        .flex(Flex::Center)
        .areas(body.inner(Margin::new(1, 0)));
    match app.screen {
        Screen::Setup => draw_setup(f, body, app),
        Screen::Auth => draw_auth(f, body, app),
        Screen::Courses => draw_courses(f, body, app),
        Screen::Tree => draw_tree(f, body, app),
        Screen::NodeStudents => draw_node_students(f, body, app),
        Screen::Students => draw_students(f, body, app),
        Screen::StudentNodes => draw_student_nodes(f, body, app),
        Screen::Form => draw_form(f, body, app),
        Screen::Groups => draw_groups(f, body, app),
    }
    draw_status(f, status, app);
    draw_footer(f, footer, app);
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let mut badges = vec![Span::styled(
        " tuios ",
        Style::new().bg(ACCENT).fg(Color::Black).bold(),
    )];
    if app.is_mock() {
        badges.push(Span::styled(
            " MOCK DATA ",
            Style::new().bg(WARN).fg(Color::Black).bold(),
        ));
    }
    let mut crumbs: Vec<String> = Vec::new();
    let in_course = !matches!(app.screen, Screen::Setup | Screen::Auth | Screen::Courses);
    if in_course && let Some(c) = app.current_course() {
        crumbs.push(format!("{} ({})", c.course_name, c.term_id));
    }
    let node = app.current_node().map(|n| n.name.clone());
    let student = app.student.as_ref().map(|id| app.student_name(id));
    let back = app.form.as_ref().map(|f| f.back);
    match (app.screen, back) {
        (Screen::NodeStudents, _) | (Screen::Form, Some(Screen::NodeStudents)) => {
            crumbs.extend(node);
        }
        (Screen::Students, _) => crumbs.push("Students".into()),
        (Screen::StudentNodes, _) | (Screen::Form, Some(Screen::StudentNodes)) => {
            crumbs.extend(student);
        }
        (Screen::Setup, _) => crumbs.push("API key & sign-in".into()),
        _ => {}
    }
    if app.screen == Screen::Form
        && let (Some(form), Some(n)) = (&app.form, app.form_node())
    {
        crumbs.push(match form.back {
            Screen::NodeStudents => app.student_name(&form.student),
            _ => n.name.clone(),
        });
    }
    let used: usize = badges.iter().map(Span::width).sum::<usize>() + 2;
    let room = (area.width as usize).saturating_sub(used);
    // Drop the outermost crumbs first: the innermost say where you are.
    let total = |cs: &[String]| cs.iter().map(|c| width(c) + 3).sum::<usize>();
    let mut elided = false;
    while crumbs.len() > 1 && total(&crumbs) + 2 > room {
        crumbs.remove(0);
        elided = true;
    }
    let mut spans = badges;
    spans.push(Span::raw("  "));
    if elided {
        spans.push(Span::styled("… › ", Style::new().fg(DIM)));
    }
    let n = crumbs.len();
    for (i, crumb) in crumbs.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" › ", Style::new().fg(DIM)));
        }
        let last = i + 1 == n;
        let shown = if last {
            ellipsize(&crumb, room.saturating_sub(4))
        } else {
            crumb
        };
        let style = if last {
            Style::new().bold()
        } else {
            Style::new()
        };
        spans.push(Span::styled(shown, style));
    }
    f.render_widget(Line::from(spans), area);
}

/// Key hints as (key, description, priority); lower priority numbers survive longer
/// when the terminal is too narrow for all of them.
fn hints(app: &App) -> Vec<(&'static str, &'static str, u8)> {
    let picker = [
        ("enter", "open", 1),
        ("↑↓", "move", 6),
        ("type", "to filter", 7),
    ];
    let with = |extra: &[(&'static str, &'static str, u8)]| [&picker[..], extra].concat();
    match app.screen {
        Screen::Setup => {
            let mut h = vec![
                ("enter", "next field", 1),
                ("ctrl-s", "save & sign in", 0),
                ("ctrl-o", "open the key page", 1),
                ("tab", "switch field", 4),
            ];
            if app.setup.focus == LANGUAGE_FIELD {
                h.push(("←→", "change", 2));
            }
            h.push(("esc", if app.setup_returns() { "back" } else { "quit" }, 0));
            h
        }
        Screen::Auth => vec![
            ("enter", "submit PIN", 1),
            ("ctrl-o", "open link", 2),
            ("ctrl-k", "API key", 3),
            ("esc", "quit", 0),
        ],
        Screen::Courses => {
            let mut h = with(&[("ctrl-r", "reload", 4)]);
            if !app.is_mock() {
                h.push(("ctrl-k", "API key & sign-in", 5));
            }
            h.push(("esc", "quit", 0));
            h
        }
        Screen::Tree => with(&[
            ("←→", "fold", 5),
            ("tab", "students", 2),
            ("ctrl-g", "group", 3),
            ("ctrl-r", "reload", 4),
            ("esc", "courses", 0),
        ]),
        Screen::NodeStudents => with(&[
            ("ctrl-g", "group", 3),
            ("ctrl-r", "reload", 4),
            ("esc", "tree", 0),
        ]),
        Screen::Students => with(&[
            ("tab", "tree", 2),
            ("ctrl-g", "group", 3),
            ("ctrl-r", "reload", 4),
            ("esc", "tree", 0),
        ]),
        Screen::StudentNodes => with(&[("ctrl-r", "reload", 4), ("esc", "students", 0)]),
        Screen::Groups => with(&[("esc", "back", 0)]),
        Screen::Form => match &app.form {
            Some(f) if f.saving => vec![],
            Some(f) if f.confirm_discard => vec![
                ("y", "discard changes", 0),
                ("any other key", "keep editing", 0),
            ],
            Some(f) => {
                let grade = matches!(app.form_node().map(|n| &n.kind), Some(Kind::Grade { .. }));
                let mut h = vec![("ctrl-s", "save to USOS", 0)];
                if f.focus == Field::Value {
                    if grade {
                        h.push(("←→ or type", "choose grade", 1));
                    }
                    h.push(("enter", "to comment", 2));
                }
                h.extend([("tab", "switch field", 4), ("esc", "back", 0)]);
                h
            }
            None => vec![],
        },
    }
}

fn error_line(err: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("✗ ", Style::new().fg(ERR).bold()),
        Span::styled(err.to_string(), Style::new().fg(ERR)),
        Span::styled("   ctrl-r retries", Style::new().fg(DIM)),
    ])
}

/// Text that is still loading, failed, or ready but empty takes the list's place.
fn placeholder<T>(
    load: &Load<T>,
    frame: usize,
    loading: &str,
    empty: impl FnOnce(&T) -> Option<&'static str>,
) -> Option<Line<'static>> {
    match load {
        Load::Loading => Some(loading_line(frame, loading)),
        Load::Failed(e) => Some(error_line(e)),
        Load::Ready(v) => empty(v).map(|m| Line::styled(m, Style::new().fg(DIM))),
    }
}

// ----- setup and sign-in -----

fn draw_setup(f: &mut Frame, area: Rect, app: &mut App) {
    let card = centered(area, 92, 30);
    let outer = block(title(&["Connect tuios to USOS"], None));
    let inner = outer.inner(card).inner(Margin::new(1, 1));
    f.render_widget(Clear, card);
    f.render_widget(outer, card);
    let setup = &app.setup;
    let step = |n: &str| Span::styled(format!("{n}. "), Style::new().fg(ACCENT).bold());
    let intro = vec![
        Line::raw("tuios talks to USOS with an API key of your own. Getting one is free and"),
        Line::raw("takes a minute, and is needed only once:"),
        Line::raw(""),
        Line::from(vec![
            step("1"),
            Span::raw("Open "),
            Span::styled(setup.developers_url(), Style::new().fg(ACCENT).underlined()),
            Span::styled("  (ctrl-o)", Style::new().fg(DIM)),
        ]),
        Line::from(vec![
            Span::raw("   "),
            Span::raw("and sign in with your USOS account."),
        ]),
        Line::from(vec![
            step("2"),
            Span::raw("Register an application; any name works, e.g. “tuios”."),
        ]),
        Line::from(vec![
            step("3"),
            Span::raw("Paste the consumer key and secret it shows into the fields below."),
        ]),
    ];
    let [intro_area, _, fields_area, note_area, msg_area] = Layout::vertical([
        Constraint::Length(intro.len() as u16),
        Constraint::Length(1),
        Constraint::Length(12),
        Constraint::Length(2),
        Constraint::Min(1),
    ])
    .areas(inner);
    f.render_widget(Paragraph::new(intro), intro_area);

    let rows = Layout::vertical([Constraint::Length(3); 4]).split(fields_area);
    for (i, label) in SETUP_LABELS.iter().enumerate() {
        let focused = setup.focus == i;
        if i == LANGUAGE_FIELD {
            let [b] = Layout::horizontal([Constraint::Length(24)]).areas(rows[i]);
            let lang = &setup.fields[i].text;
            let line = Line::from(vec![
                Span::styled("‹ ", Style::new().fg(if focused { ACCENT } else { DIM })),
                Span::styled(
                    if lang == "en" { "English" } else { "Polish" },
                    Style::new().bold(),
                ),
                Span::styled(" ›", Style::new().fg(if focused { ACCENT } else { DIM })),
            ]);
            f.render_widget(
                Paragraph::new(line).block(input_block(label, focused, false)),
                b,
            );
            continue;
        }
        // The secret is shown masked except for its last characters, so a shared
        // screen does not leak it while a mistyped paste can still be spotted.
        let shown;
        let input = if i == 2 && !focused && !setup.fields[2].text.is_empty() {
            let text = &setup.fields[2].text;
            let n = text.chars().count();
            let tail: String = text.chars().skip(n.saturating_sub(4)).collect();
            shown = Input {
                text: format!("{}{tail}", "•".repeat(n.saturating_sub(4))),
                ..Input::default()
            };
            &shown
        } else {
            &setup.fields[i]
        };
        draw_input(
            f,
            rows[i],
            input,
            focused,
            input_block(label, focused, false),
        );
    }
    let note = Line::styled(
        format!(
            "Saved only on this computer, readable only by you: {}",
            home_relative(&config::settings_path())
        ),
        Style::new().fg(DIM),
    );
    f.render_widget(Paragraph::new(note).wrap(Wrap { trim: true }), note_area);
    let msg = match &setup.error {
        Some(e) => Line::styled(format!("✗ {e}"), Style::new().fg(ERR)),
        None => Line::default(),
    };
    f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }), msg_area);
}

fn draw_auth(f: &mut Frame, area: Rect, app: &mut App) {
    let card = centered(area, 92, 17);
    let outer = block(title(&["Sign in to USOS"], None));
    let inner = outer.inner(card);
    f.render_widget(outer, card);
    let [text_area, _, pin_area, msg_area] = Layout::vertical([
        Constraint::Length(7),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(1),
    ])
    .areas(inner.inner(Margin::new(0, 1)));
    let url = match &app.auth.url {
        Some(u) => Line::styled(u.clone(), Style::new().fg(ACCENT).underlined()),
        None if app.auth.busy => loading_line(app.frame, "Asking USOS for a sign-in link…"),
        None => Line::styled("(no link — see the error below)", Style::new().fg(DIM)),
    };
    let step = |n: &str| Span::styled(format!("{n}. "), Style::new().fg(ACCENT).bold());
    let lines = vec![
        Line::raw("Needed once. tuios asks to read and change points and grades in the"),
        Line::raw("course tests (Sprawdziany) you can grade, and to see who is in your groups."),
        Line::raw(""),
        Line::from(vec![
            step("1"),
            Span::raw("Open this link and approve access (ctrl-o opens it):"),
        ]),
        url,
        Line::from(vec![
            step("2"),
            Span::raw("Type the PIN USOS shows you below."),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), text_area);
    let [pin_box] = Layout::horizontal([Constraint::Length(24)]).areas(pin_area);
    draw_input(
        f,
        pin_box,
        &app.auth.pin,
        true,
        input_block("PIN", true, false),
    );
    let msg = if app.auth.busy && app.auth.url.is_some() {
        loading_line(app.frame, "Checking PIN…")
    } else if let Some(e) = &app.auth.error {
        Line::styled(format!("✗ {e}"), Style::new().fg(ERR))
    } else {
        Line::default()
    };
    f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }), msg_area);
}

// ----- lists -----

fn draw_courses(f: &mut Frame, area: Rect, app: &mut App) {
    let items = app.course_items();
    let terms: Vec<String> = app
        .courses
        .ready()
        .map(|cs| cs.iter().map(|c| c.term_id.clone()).collect())
        .unwrap_or_default();
    let columns = Columns {
        widths: vec![Constraint::Length(6)],
        cells: Box::new(move |i| {
            vec![Cell::from(Span::styled(
                terms[i].clone(),
                Style::new().fg(SOFT),
            ))]
        }),
        name_style: Box::new(|_| Style::new()),
    };
    let placeholder = placeholder(&app.courses, app.frame, "Loading your courses…", |cs| {
        cs.is_empty()
            .then_some("USOS lists no course test you can grade")
    });
    let t = title(&["Choose a course"], Some("latest term first".into()));
    draw_picker(
        f,
        area,
        t,
        &mut app.course_picker,
        &items,
        columns,
        placeholder,
    );
}

/// Points range or grade scale of a node, for the tree's second column.
fn node_kind(node: &Node) -> Span<'static> {
    match &node.kind {
        Kind::Task {
            min,
            max,
            precision,
            ..
        } => Span::styled(
            points_range(*min, *max, *precision, "pts"),
            Style::new().fg(SOFT),
        ),
        Kind::Grade { .. } => Span::styled("grade", Style::new().fg(ACCENT2)),
        _ => Span::raw(""),
    }
}

fn draw_tree(f: &mut Frame, area: Rect, app: &mut App) {
    let items = app.node_items();
    let loaded = app.tree.ready().is_some_and(|t| t.loaded);
    let frame = app.frame;
    let rows: Vec<(Span<'static>, Vec<Span<'static>>, bool, bool)> = app
        .tree
        .ready()
        .map(|t| {
            app.listed_nodes()
                .into_iter()
                .map(|i| {
                    let n = &t.nodes[i];
                    let folder = matches!(n.kind, Kind::Folder);
                    let progress_spans = if folder {
                        vec![]
                    } else if !loaded {
                        vec![
                            Span::styled(spinner(frame), Style::new().fg(ACCENT)),
                            Span::styled(" loading…", Style::new().fg(DIM)),
                        ]
                    } else {
                        let (g, total) = app.graded_count(n);
                        progress(g, total, 12)
                    };
                    (node_kind(n), progress_spans, folder, n.editable || folder)
                })
                .collect()
        })
        .unwrap_or_default();
    let styles: Vec<(bool, bool)> = rows.iter().map(|r| (r.2, r.3)).collect();
    let columns = Columns {
        widths: vec![Constraint::Length(12), Constraint::Length(26)],
        cells: Box::new(move |i| {
            let mut kind = vec![rows[i].0.clone()];
            if !rows[i].3 {
                kind.push(Span::styled(" read-only", Style::new().fg(DIM)));
            }
            vec![
                Cell::from(Line::from(kind)),
                Cell::from(Line::from(rows[i].1.clone())),
            ]
        }),
        name_style: Box::new(move |i| match styles[i] {
            (true, _) => Style::new().fg(SOFT).bold(),
            (_, false) => Style::new().fg(DIM),
            _ => Style::new(),
        }),
    };
    let placeholder = placeholder(&app.tree, frame, "Loading the course test…", |t| {
        (t.nodes.len() <= 1).then_some("This course test is empty")
    });
    let extra = format!("{} · {}", app.group_label(), course_label(app));
    let t = title(&["Tasks and grades"], Some(extra));
    draw_picker(
        f,
        area,
        t,
        &mut app.tree_picker,
        &items,
        columns,
        placeholder,
    );
}

fn course_label(app: &App) -> String {
    app.current_course()
        .map(|c| c.test_name.clone())
        .unwrap_or_default()
}

/// "5 pts", or "-2–5 pts" for the rare task whose minimum is not zero.
fn points_range(min: f64, max: f64, precision: u32, unit: &str) -> String {
    if min == 0.0 {
        format!("{} {unit}", fmt_points(max, precision))
    } else {
        format!(
            "{}–{} {unit}",
            fmt_points(min, precision),
            fmt_points(max, precision)
        )
    }
}

/// A student's value in a task or grade node, as shown in lists.
fn value_line(node: &Node, student: &str) -> Line<'static> {
    match &node.kind {
        Kind::Task {
            max,
            precision,
            points,
            ..
        } => match points.get(student).and_then(|e| e.points) {
            Some(p) => Line::from(vec![
                Span::styled(fmt_points(p, *precision), Style::new().fg(OK).bold()),
                Span::styled(
                    format!(" / {}", fmt_points(*max, *precision)),
                    Style::new().fg(DIM),
                ),
            ]),
            None => Line::styled("—", Style::new().fg(DIM)),
        },
        Kind::Grade { grades, .. } => {
            let entry = grades.get(student);
            match entry.and_then(|e| e.value.as_ref()) {
                Some(v) => Line::styled(v.symbol.clone(), Style::new().fg(OK).bold()),
                None => match entry.and_then(|e| e.automatic.clone()) {
                    Some(auto) => Line::styled(format!("({auto} auto)"), Style::new().fg(SOFT)),
                    None => Line::styled("—", Style::new().fg(DIM)),
                },
            }
        }
        _ => Line::default(),
    }
}

fn comment_of(node: &Node, student: &str) -> Option<String> {
    match &node.kind {
        Kind::Task { points, .. } => points.get(student)?.comment.clone(),
        Kind::Grade { grades, .. } => grades.get(student)?.comment.clone(),
        _ => None,
    }
}

fn draw_node_students(f: &mut Frame, area: Rect, app: &mut App) {
    let items = app.student_items();
    let Some(node) = app.current_node().cloned() else {
        return;
    };
    let ids: Vec<String> = app
        .tree
        .ready()
        .map(|t| {
            app.roster()
                .into_iter()
                .map(|i| t.students[i].id.clone())
                .collect()
        })
        .unwrap_or_default();
    let comment_w = picker_inner_width(area).saturating_sub(26 + 12 + 4).min(48);
    let n = node.clone();
    let columns = Columns {
        widths: vec![Constraint::Length(12), Constraint::Length(comment_w as u16)],
        cells: Box::new(move |i| {
            let comment = comment_of(&n, &ids[i])
                .map(|c| ellipsize(&c.replace('\n', " "), comment_w))
                .unwrap_or_default();
            vec![
                Cell::from(value_line(&n, &ids[i])),
                Cell::from(Span::styled(comment, Style::new().fg(SOFT))),
            ]
        }),
        name_style: Box::new(|_| Style::new()),
    };
    let (g, total) = app.graded_count(&node);
    let extra = format!("{g} of {total} graded · {}", app.group_label());
    let t = title(&[&node.name], Some(extra));
    let placeholder = match &app.tree {
        Load::Ready(t) if !t.loaded => Some(loading_line(app.frame, "Loading students…")),
        Load::Ready(_) if items.is_empty() => Some(Line::styled(
            "No students in this group",
            Style::new().fg(DIM),
        )),
        _ => None,
    };
    draw_picker(
        f,
        area,
        t,
        &mut app.student_picker,
        &items,
        columns,
        placeholder,
    );
}

fn draw_students(f: &mut Frame, area: Rect, app: &mut App) {
    let items = app.student_items();
    let rows: Vec<(String, String)> = app
        .tree
        .ready()
        .map(|t| {
            let grade_nodes: Vec<&Node> = t
                .nodes
                .iter()
                .filter(|n| matches!(n.kind, Kind::Grade { .. }))
                .collect();
            app.roster()
                .into_iter()
                .map(|i| {
                    let id = &t.students[i].id;
                    let (got, max) = app.total(id);
                    let grades: Vec<String> = grade_nodes
                        .iter()
                        .filter_map(|n| match &n.kind {
                            Kind::Grade { grades, .. } => {
                                grades.get(id)?.value.as_ref().map(|v| v.symbol.clone())
                            }
                            _ => None,
                        })
                        .collect();
                    let total = if max > 0.0 {
                        format!("Σ {} / {}", fmt_points(got, 2), fmt_points(max, 2))
                    } else {
                        String::new()
                    };
                    (total, grades.join(" "))
                })
                .collect()
        })
        .unwrap_or_default();
    let columns = Columns {
        widths: vec![Constraint::Length(16), Constraint::Length(12)],
        cells: Box::new(move |i| {
            vec![
                Cell::from(
                    Line::styled(rows[i].0.clone(), Style::new().fg(SOFT))
                        .alignment(Alignment::Right),
                ),
                Cell::from(Span::styled(rows[i].1.clone(), Style::new().fg(OK).bold())),
            ]
        }),
        name_style: Box::new(|_| Style::new()),
    };
    let placeholder = match &app.tree {
        Load::Loading => Some(loading_line(app.frame, "Loading the course test…")),
        Load::Failed(e) => Some(error_line(e)),
        Load::Ready(t) if !t.loaded => Some(loading_line(app.frame, "Loading students…")),
        Load::Ready(_) if items.is_empty() => Some(Line::styled(
            "No students in this group",
            Style::new().fg(DIM),
        )),
        _ => None,
    };
    let extra = format!("{} · {}", app.group_label(), course_label(app));
    let t = title(&["Students"], Some(extra));
    draw_picker(
        f,
        area,
        t,
        &mut app.student_picker,
        &items,
        columns,
        placeholder,
    );
}

fn draw_student_nodes(f: &mut Frame, area: Rect, app: &mut App) {
    let items = app.node_items();
    let Some(id) = app.student.clone() else {
        return;
    };
    let rows: Vec<(Line<'static>, String, bool)> = app
        .tree
        .ready()
        .map(|t| {
            app.listed_nodes()
                .into_iter()
                .map(|i| {
                    let n = &t.nodes[i];
                    let comment = comment_of(n, &id).unwrap_or_default().replace('\n', " ");
                    (value_line(n, &id), comment, matches!(n.kind, Kind::Folder))
                })
                .collect()
        })
        .unwrap_or_default();
    let folders: Vec<bool> = rows.iter().map(|r| r.2).collect();
    let comment_w = picker_inner_width(area).saturating_sub(30 + 12 + 4).min(48);
    let columns = Columns {
        widths: vec![Constraint::Length(12), Constraint::Length(comment_w as u16)],
        cells: Box::new(move |i| {
            vec![
                Cell::from(rows[i].0.clone()),
                Cell::from(Span::styled(
                    ellipsize(&rows[i].1, comment_w),
                    Style::new().fg(SOFT),
                )),
            ]
        }),
        name_style: Box::new(move |i| {
            if folders[i] {
                Style::new().fg(SOFT).bold()
            } else {
                Style::new()
            }
        }),
    };
    let (got, max) = app.total(&id);
    let extra =
        (max > 0.0).then(|| format!("Σ {} / {} points", fmt_points(got, 2), fmt_points(max, 2)));
    let name = app.student_name(&id);
    let t = title(&[&name], extra);
    draw_picker(
        f,
        area,
        t,
        &mut app.student_node_picker,
        &items,
        columns,
        None,
    );
}

fn draw_groups(f: &mut Frame, area: Rect, app: &mut App) {
    let items = app.group_items();
    let all = app.tree.ready().map_or(0, |t| t.students.len());
    let mut sizes = vec![all];
    if let Some(gs) = app.groups.ready() {
        sizes.extend(gs.iter().map(|g| g.people.len()));
    }
    let columns = Columns {
        widths: vec![Constraint::Length(12)],
        cells: Box::new(move |i| {
            vec![Cell::from(
                Line::styled(format!("{} students", sizes[i]), Style::new().fg(SOFT))
                    .alignment(Alignment::Right),
            )]
        }),
        name_style: Box::new(|_| Style::new()),
    };
    let (note, problem) = match &app.groups {
        Load::Loading => ("loading your groups…", None),
        Load::Failed(e) => ("", Some(e.clone())),
        Load::Ready(gs) if gs.is_empty() => ("USOS lists no group of yours here", None),
        Load::Ready(_) => ("groups you teach", None),
    };
    let [list, msg] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(if problem.is_some() { 3 } else { 0 }),
    ])
    .areas(area);
    let t = title(&["Show students from"], Some(note.into()));
    draw_picker(f, list, t, &mut app.group_picker, &items, columns, None);
    if let Some(e) = problem {
        let line = Line::from(vec![
            Span::styled(
                "✗ Your groups are unavailable: ",
                Style::new().fg(ERR).bold(),
            ),
            Span::styled(e, Style::new().fg(ERR)),
        ]);
        f.render_widget(
            Paragraph::new(line)
                .wrap(Wrap { trim: true })
                .block(Block::new().padding(Padding::horizontal(1))),
            msg,
        );
    }
}

// ----- the grading form -----

fn draw_form(f: &mut Frame, area: Rect, app: &mut App) {
    let (Some(form), Some(node)) = (&app.form, app.form_node()) else {
        return;
    };
    let label = |s: &str| Span::styled(format!("{s:<9}"), Style::new().fg(SOFT));
    let is_grade = matches!(node.kind, Kind::Grade { .. });
    let student_line = Line::from(vec![
        label("Student"),
        Span::styled(app.student_name(&form.student), Style::new().bold()),
    ]);
    let range = match &node.kind {
        Kind::Task {
            min,
            max,
            precision,
            ..
        } => format!("  {}", points_range(*min, *max, *precision, "points")),
        _ => "  grade".into(),
    };
    let node_line = Line::from(vec![
        label(if is_grade { "Grade" } else { "Task" }),
        Span::styled(node.name.clone(), Style::new().bold()),
        Span::styled(range, Style::new().fg(DIM)),
    ]);
    let (value, grader, changed, auto) = match &node.kind {
        Kind::Task {
            max,
            precision,
            points,
            ..
        } => {
            let e = points.get(&form.student);
            (
                e.and_then(|e| e.points).map(|p| {
                    format!(
                        "{} / {}",
                        fmt_points(p, *precision),
                        fmt_points(*max, *precision)
                    )
                }),
                e.and_then(|e| e.grader.clone()),
                e.and_then(|e| e.last_changed.clone()),
                None,
            )
        }
        Kind::Grade { grades, .. } => {
            let e = grades.get(&form.student);
            (
                e.and_then(|e| e.value.as_ref()).map(|v| v.symbol.clone()),
                e.and_then(|e| e.grader.clone()),
                e.and_then(|e| e.last_changed.clone()),
                e.and_then(|e| e.automatic.clone()),
            )
        }
        _ => (None, None, None, None),
    };
    let mut current = vec![label("In USOS")];
    match value {
        Some(v) => {
            current.push(Span::styled(v, Style::new().fg(OK).bold()));
            if let Some(g) = grader {
                current.push(Span::styled(format!("  by {g}"), Style::new().fg(SOFT)));
            }
            if let Some(t) = changed {
                let t = t.split('.').next().unwrap_or(&t).to_string();
                current.push(Span::styled(format!(", {t}"), Style::new().fg(DIM)));
            }
        }
        None => current.push(Span::styled("nothing yet", Style::new().fg(DIM))),
    }
    if let Some(a) = auto {
        current.push(Span::styled(
            format!("  (computed: {a})"),
            Style::new().fg(SOFT),
        ));
    }
    let info = vec![student_line, node_line, Line::from(current)];

    let card_w = area.width.min(84);
    let pad = u16::from(area.height >= 24);
    let avail = area.height.saturating_sub(2 + 2 * pad) as usize;
    let private_h = if is_grade { 4 } else { 0 };
    let fixed = info.len() + 1 + 3 + private_h + 2;
    let comment_h = avail.saturating_sub(fixed).clamp(3, 7);
    let card_h = (fixed + comment_h) as u16 + 2 + 2 * pad;
    let card = centered(area, card_w, card_h);
    let outer = block(title(&[if is_grade { "Grade" } else { "Points" }], None))
        .padding(Padding::new(1, 1, pad, pad));
    let inner = outer.inner(card);
    f.render_widget(Clear, card);
    f.render_widget(outer, card);
    let [
        info_area,
        _,
        value_area,
        comment_area,
        private_area,
        msg_area,
    ] = Layout::vertical([
        Constraint::Length(info.len() as u16),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(comment_h as u16),
        Constraint::Length(private_h as u16),
        Constraint::Min(1),
    ])
    .areas(inner);
    let room = info_area.width as usize;
    let info: Vec<Line> = info
        .into_iter()
        .map(|l| {
            if l.width() <= room {
                l
            } else {
                let plain: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                Line::raw(ellipsize(&plain, room))
            }
        })
        .collect();
    f.render_widget(Paragraph::new(info), info_area);

    let focus = form.focus;
    match &node.kind {
        Kind::Task { max, precision, .. } => {
            let invalid = !form.points.text.trim().is_empty() && app.parse_points().is_err();
            let [p_box, p_max] =
                Layout::horizontal([Constraint::Length(16), Constraint::Min(1)]).areas(value_area);
            draw_input(
                f,
                p_box,
                &form.points,
                focus == Field::Value,
                input_block("Points", focus == Field::Value, invalid),
            );
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!(" / {}", fmt_points(*max, *precision)),
                    Style::new().fg(SOFT),
                ))
                .block(Block::new().padding(Padding::top(1))),
                p_max,
            );
        }
        Kind::Grade { scale, .. } => {
            let focused = focus == Field::Value;
            let line = match app.grade_choices(scale) {
                Some(choices) => {
                    let mut spans = Vec::new();
                    for (i, c) in choices.iter().enumerate() {
                        let text = c.map_or("none".to_string(), |v| v.symbol.clone());
                        let selected = i == form.grade;
                        let style = match (selected, c.is_some_and(|v| v.passes) || c.is_none()) {
                            (true, _) => Style::new().bg(ACCENT).fg(Color::Black).bold(),
                            (false, true) => Style::new().fg(SOFT),
                            (false, false) => Style::new().fg(WARN),
                        };
                        spans.push(Span::styled(format!(" {text} "), style));
                        spans.push(Span::raw(" "));
                    }
                    Line::from(spans)
                }
                None => match app.scales.get(scale) {
                    Some(Load::Failed(e)) => Line::styled(
                        format!("✗ could not load the grade scale: {e}"),
                        Style::new().fg(ERR),
                    ),
                    _ => loading_line(app.frame, "Loading the grade scale…"),
                },
            };
            f.render_widget(
                Paragraph::new(line).block(input_block("Grade", focused, false)),
                value_area,
            );
        }
        _ => {}
    }
    let len = form.comment.text.chars().count();
    let counter = Span::styled(
        format!(" {len}/{COMMENT_MAX} "),
        Style::new().fg(if len >= COMMENT_MAX { WARN } else { DIM }),
    );
    let cb = input_block(
        "Comment for the student (optional)",
        focus == Field::Comment,
        false,
    )
    .title_bottom(Line::from(counter).alignment(Alignment::Right));
    draw_textarea(f, comment_area, &form.comment, focus == Field::Comment, cb);
    if is_grade {
        let pb = input_block(
            "Private note for graders (optional)",
            focus == Field::Private,
            false,
        );
        draw_textarea(f, private_area, &form.private, focus == Field::Private, pb);
    }

    let msg = if form.saving {
        loading_line(app.frame, "Saving to USOS…")
    } else if form.confirm_discard {
        Line::from(vec![
            Span::styled("Discard your changes?  ", Style::new().fg(WARN).bold()),
            Span::styled("y", Style::new().fg(ACCENT).bold()),
            Span::styled(
                " discards · any other key keeps editing",
                Style::new().fg(SOFT),
            ),
        ])
    } else if let Some(e) = &form.error {
        Line::from(vec![
            Span::styled("✗ ", Style::new().fg(ERR).bold()),
            Span::styled(e.clone(), Style::new().fg(ERR)),
        ])
    } else {
        Line::from(vec![
            Span::styled("ctrl-s", Style::new().fg(ACCENT).bold()),
            Span::styled(" saves to USOS", Style::new().fg(DIM)),
        ])
    };
    f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }), msg_area);
}

// ----- shared drawing helpers -----

/// `~/...` for paths under the home directory, which keeps them short on screen.
fn home_relative(path: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(h).ok().map(|p| p.to_path_buf())) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

struct Columns<'a> {
    widths: Vec<Constraint>,
    cells: Box<dyn Fn(usize) -> Vec<Cell<'static>> + 'a>,
    name_style: Box<dyn Fn(usize) -> Style + 'a>,
}

fn spinner(frame: usize) -> &'static str {
    SPINNER[frame % SPINNER.len()]
}

fn width(s: &str) -> usize {
    Span::raw(s).width()
}

/// Shortens `s` to at most `max` display cells, marking the cut with an ellipsis.
fn ellipsize(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if width(&out) + width(&c.to_string()) + 1 > max {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let Some(t) = &app.toast else { return };
    let (icon, color) = match t.kind {
        ToastKind::Info => ("●", ACCENT),
        ToastKind::Success => ("✓", OK),
        ToastKind::Error => ("✗", ERR),
    };
    let text = ellipsize(&t.text, (area.width as usize).saturating_sub(4));
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled(icon, Style::new().fg(color).bold()),
        Span::raw(" "),
        Span::styled(text, Style::new().fg(color)),
    ]);
    f.render_widget(line, area);
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let mut hints = hints(app);
    let line_width = |hs: &[(&str, &str, u8)]| -> usize {
        hs.iter()
            .map(|(k, d, _)| width(k) + 1 + width(d))
            .sum::<usize>()
            + 5 * hs.len().saturating_sub(1)
            + 1
    };
    while line_width(&hints) > area.width as usize {
        let Some((pos, _)) = hints
            .iter()
            .enumerate()
            .filter(|(_, h)| h.2 > 0)
            .max_by_key(|(_, h)| h.2)
        else {
            break;
        };
        hints.remove(pos);
    }
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, what, _)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::new().fg(DIM)));
        }
        spans.push(Span::styled(*key, Style::new().fg(ACCENT).bold()));
        spans.push(Span::styled(format!(" {what}"), Style::new().fg(SOFT)));
    }
    f.render_widget(Line::from(spans), area);
}

fn block(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(title.into().patch_style(Style::new().fg(ACCENT).bold()))
        .padding(Padding::horizontal(1))
}

fn title(parts: &[&str], extra: Option<String>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::new().fg(DIM)));
        }
        spans.push(Span::raw(p.to_string()));
    }
    if let Some(e) = extra {
        spans.push(Span::styled(
            format!("  {e}"),
            Style::new().fg(SOFT).remove_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// Highlights the fuzzy-matched characters of `text`.
fn highlighted(text: &str, positions: &[u32], base: Style) -> Line<'static> {
    if positions.is_empty() {
        return Line::styled(text.to_string(), base);
    }
    let hit = base.fg(ACCENT2).bold().underlined();
    let spans: Vec<Span> = text
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let style = if positions.binary_search(&(i as u32)).is_ok() {
                hit
            } else {
                base
            };
            Span::styled(c.to_string(), style)
        })
        .collect();
    Line::from(spans)
}

/// Inner width available to a picker's table rows (borders, padding, cursor column).
fn picker_inner_width(area: Rect) -> usize {
    (area.width as usize).saturating_sub(4 + 2)
}

/// A bordered fuzzy picker: query line on top, filtered table below.
fn draw_picker(
    f: &mut Frame,
    area: Rect,
    title: Line<'static>,
    picker: &mut Picker,
    items: &[String],
    columns: Columns,
    placeholder: Option<Line<'static>>,
) {
    let outer = block(title);
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    let [query_area, rule_area, list_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);

    let count = format!("{}/{}", picker.matches.len(), items.len());
    let [q, c] = Layout::horizontal([Constraint::Min(1), Constraint::Length(count.len() as u16)])
        .areas(query_area);
    let prompt = Span::styled("❯ ", Style::new().fg(ACCENT).bold());
    let query = if picker.query.is_empty() {
        Line::from(vec![
            prompt,
            Span::styled("type to filter", Style::new().fg(DIM).italic()),
        ])
    } else {
        Line::from(vec![prompt, Span::raw(picker.query.clone())])
    };
    f.render_widget(query, q);
    f.render_widget(Span::styled(count, Style::new().fg(DIM)), c);
    let cursor_x = q.x + 2 + width(&picker.query) as u16;
    f.set_cursor_position(Position::new(
        cursor_x.min(q.right().saturating_sub(1)),
        q.y,
    ));
    f.render_widget(
        Span::styled("─".repeat(rule_area.width as usize), Style::new().fg(DIM)),
        rule_area,
    );

    if let Some(p) = placeholder {
        f.render_widget(Paragraph::new(p).alignment(Alignment::Center), list_area);
        return;
    }
    if picker.matches.is_empty() {
        let msg = if items.is_empty() {
            "nothing here"
        } else {
            "no matches — esc clears the filter"
        };
        f.render_widget(
            Paragraph::new(Span::styled(msg, Style::new().fg(DIM).italic()))
                .alignment(Alignment::Center),
            list_area,
        );
        return;
    }
    let rows: Vec<Row> = picker
        .matches
        .iter()
        .map(|m| {
            let mut cells = vec![Cell::from(highlighted(
                &items[m.index],
                &m.positions,
                (columns.name_style)(m.index),
            ))];
            cells.extend((columns.cells)(m.index));
            Row::new(cells)
        })
        .collect();
    let mut widths = vec![Constraint::Fill(1)];
    widths.extend(columns.widths.iter().copied());
    let table = Table::new(rows, widths)
        .column_spacing(2)
        .row_highlight_style(Style::new().bg(SELECTED).add_modifier(Modifier::BOLD))
        .highlight_symbol(Span::styled("▌ ", Style::new().fg(ACCENT)))
        .highlight_spacing(HighlightSpacing::Always);
    picker.state.select(Some(picker.selected));
    f.render_stateful_widget(table, list_area, &mut picker.state);
}

fn loading_line(frame: usize, what: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(spinner(frame), Style::new().fg(ACCENT)),
        Span::styled(format!(" {what}"), Style::new().fg(SOFT)),
    ])
}

fn progress(graded: usize, total: usize, bar: usize) -> Vec<Span<'static>> {
    let filled = (graded * bar + total / 2).checked_div(total).unwrap_or(0);
    let color = if total > 0 && graded == total {
        OK
    } else {
        ACCENT
    };
    vec![
        Span::styled("━".repeat(filled), Style::new().fg(color)),
        Span::styled("━".repeat(bar - filled), Style::new().fg(TRACK)),
        Span::styled(
            format!(" {graded:>2}/{total} graded"),
            Style::new().fg(SOFT),
        ),
    ]
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let [hr] = Layout::horizontal([Constraint::Length(w.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [v] = Layout::vertical([Constraint::Length(h.min(area.height))])
        .flex(Flex::Center)
        .areas(hr);
    v
}

fn input_block(label: &str, focused: bool, invalid: bool) -> Block<'static> {
    let border = if invalid {
        ERR
    } else if focused {
        ACCENT
    } else {
        DIM
    };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" {label} "),
            Style::new().fg(if focused { ACCENT } else { SOFT }).bold(),
        ))
}

/// Single-line input; scrolls horizontally to keep the cursor visible.
fn draw_input(f: &mut Frame, area: Rect, input: &Input, focused: bool, b: Block<'static>) {
    let inner = b.inner(area);
    let room = inner.width.saturating_sub(1) as usize;
    let chars: Vec<char> = input.text.chars().collect();
    let mut start = 0;
    while width(&chars[start..input.cursor].iter().collect::<String>()) > room {
        start += 1;
    }
    let visible: String = chars[start..].iter().collect();
    f.render_widget(Paragraph::new(visible).block(b), area);
    if focused {
        let before: String = chars[start..input.cursor].iter().collect();
        f.set_cursor_position(Position::new(inner.x + width(&before) as u16, inner.y));
    }
}

/// Greedy word wrap returning char ranges of each line, so the cursor can be placed.
fn wrap(chars: &[char], room: usize) -> Vec<(usize, usize)> {
    let room = room.max(1);
    let mut lines = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = start;
        let mut last_space = None;
        let mut used = 0;
        while end < chars.len() {
            let w = width(&chars[end].to_string());
            if used + w > room {
                break;
            }
            if chars[end] == ' ' {
                last_space = Some(end);
            }
            used += w;
            end += 1;
        }
        if end < chars.len()
            && let Some(sp) = last_space.filter(|&sp| sp > start)
        {
            end = sp + 1;
        }
        lines.push((start, end));
        start = end;
    }
    // A full last line leaves no cell for a cursor at the end; give it a fresh line.
    if lines.last().is_none_or(|&(s, e)| e - s >= room) {
        lines.push((chars.len(), chars.len()));
    }
    lines
}

/// Multi-line input with word wrapping; scrolls vertically to keep the cursor visible.
fn draw_textarea(f: &mut Frame, area: Rect, input: &Input, focused: bool, b: Block<'static>) {
    let inner = b.inner(area);
    let chars: Vec<char> = input.text.chars().collect();
    let lines = wrap(&chars, inner.width.saturating_sub(1) as usize);
    let cursor_line = lines
        .iter()
        .position(|&(s, e)| input.cursor >= s && input.cursor < e)
        .unwrap_or(lines.len() - 1);
    let height = inner.height.max(1) as usize;
    let top = cursor_line.saturating_sub(height - 1);
    let text: Vec<Line> = lines
        .iter()
        .skip(top)
        .take(height)
        .map(|&(s, e)| Line::raw(chars[s..e].iter().collect::<String>()))
        .collect();
    f.render_widget(Paragraph::new(text).block(b), area);
    if focused {
        let (s, _) = lines[cursor_line];
        let before: String = chars[s..input.cursor.max(s)].iter().collect();
        f.set_cursor_position(Position::new(
            inner.x + width(&before) as u16,
            inner.y + (cursor_line - top) as u16,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_word_boundaries() {
        let chars: Vec<char> = "hello brave new world".chars().collect();
        let lines: Vec<String> = wrap(&chars, 11)
            .iter()
            .map(|&(s, e)| chars[s..e].iter().collect())
            .collect();
        assert_eq!(lines, ["hello ", "brave new ", "world"]);
    }

    #[test]
    fn ellipsizes() {
        assert_eq!(ellipsize("abcdef", 4), "abc…");
        assert_eq!(ellipsize("abc", 4), "abc");
    }
}
