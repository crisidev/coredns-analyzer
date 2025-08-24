use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::widgets::canvas::{Canvas, Circle, Line as CanvasLine, Points, Rectangle};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Tabs, Wrap},
};
use serde::Serialize;

use crate::log_analyzer::DnsData;

pub(crate) async fn test() -> Result<()> {
    // Demo data if you want to run without hooking into your crate yet
    let demo = DnsData {
        internal: HashMap::from([
            ("pod-a".into(), vec!["svc-auth".into(), "svc-api".into()]),
            ("pod-b".into(), vec!["svc-api".into(), "svc-db".into()]),
            ("pod-c".into(), vec!["svc-queue".into()]),
        ]),
        external: HashMap::from([
            (
                "pod-a".into(),
                vec!["api.stripe.com".into(), "cdn.example.com".into()],
            ),
            ("pod-b".into(), vec!["registry-1.docker.io".into()]),
            (
                "pod-c".into(),
                vec!["charts.helm.sh".into(), "k8s.gcr.io".into()],
            ),
        ]),
    };

    // In your integration, replace `demo` with a channel that receives DnsData snapshots
    // and call `app.update_data(new_data)` when fresh logs are parsed.

    // TUI setup
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = AppState::default();
    app.last_tick = Instant::now();
    app.update_data(demo);

    let tick_rate = Duration::from_millis(16); // ~60 FPS animations
    let mut running = true;

    while running {
        // Draw
        terminal.draw(|f| ui(f, &mut app))?;

        // Poll input with a tiny timeout so we keep animating
        let timeout = tick_rate.saturating_sub(app.last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                running = handle_key(key, &mut app)?;
            }
        }
        animate(&mut app);
    }

    // teardown
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
enum NodeKind {
    External,
    Pod,
    Service,
}

#[derive(Clone, Debug)]
struct Node {
    id: String,
    kind: NodeKind,
    // animated position (world coords, -1..1 both axes for Canvas)
    x: f64,
    y: f64,
    // target pos for smooth transitions
    tx: f64,
    ty: f64,
}

#[derive(Clone, Debug)]
struct Edge {
    from: String,
    to: String,
}

#[derive(Clone, Debug, Default)]
struct Filters {
    pod: Option<String>,
    service: Option<String>,
    external: Option<String>,
}

#[derive(Clone, Debug)]
struct AppState {
    data: DnsData,
    nodes: HashMap<String, Node>,
    edges: Vec<Edge>,
    filters: Filters,
    tab: usize,
    last_tick: Instant,
    input_mode: InputMode,
    input_buffer: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            data: Default::default(),
            nodes: Default::default(),
            edges: Default::default(),
            filters: Default::default(),
            tab: Default::default(),
            last_tick: Instant::now(),
            input_mode: Default::default(),
            input_buffer: Default::default(),
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
enum InputMode {
    #[default]
    Normal,
    FilterPod,
    FilterService,
    FilterExternal,
    ClearConfirm,
}

fn handle_key(key: KeyEvent, app: &mut AppState) -> Result<bool> {
    match app.input_mode {
        InputMode::Normal => match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => return Ok(false),
            (KeyCode::Char('1'), _) => app.tab = 0, // Graph
            (KeyCode::Char('2'), _) => app.tab = 1, // Lists
            (KeyCode::Char('/'), _) => {
                app.input_mode = InputMode::FilterPod;
                app.input_buffer.clear();
            }
            (KeyCode::Char('s'), _) => {
                app.input_mode = InputMode::FilterService;
                app.input_buffer.clear();
            }
            (KeyCode::Char('e'), _) => {
                app.input_mode = InputMode::FilterExternal;
                app.input_buffer.clear();
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                app.input_mode = InputMode::ClearConfirm;
            }
            _ => {}
        },
        InputMode::FilterPod
        | InputMode::FilterService
        | InputMode::FilterExternal
        | InputMode::ClearConfirm => {
            match key.code {
                KeyCode::Esc => {
                    app.input_mode = InputMode::Normal;
                    app.input_buffer.clear();
                }
                KeyCode::Enter => {
                    match app.input_mode {
                        InputMode::FilterPod => {
                            app.filters.pod = non_empty(app.input_buffer.trim())
                        }
                        InputMode::FilterService => {
                            app.filters.service = non_empty(app.input_buffer.trim())
                        }
                        InputMode::FilterExternal => {
                            app.filters.external = non_empty(app.input_buffer.trim())
                        }
                        InputMode::ClearConfirm => {
                            app.filters = Filters::default();
                        }
                        _ => {}
                    }
                    app.input_mode = InputMode::Normal;
                    app.input_buffer.clear();
                    app.recompute_targets(); // re-layout towards new filter targets
                }
                KeyCode::Backspace => {
                    app.input_buffer.pop();
                }
                KeyCode::Char(c) => {
                    app.input_buffer.push(c);
                }
                _ => {}
            }
        }
    }
    Ok(true)
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

impl AppState {
    fn update_data(&mut self, data: DnsData) {
        self.data = data;
        self.rebuild_graph();
        self.recompute_targets();
    }

    fn rebuild_graph(&mut self) {
        self.nodes.clear();
        self.edges.clear();
        // Create nodes
        let mut seen = HashSet::new();
        for (pod, svcs) in &self.data.internal {
            seen.insert(pod.clone());
            self.nodes.entry(pod.clone()).or_insert(Node {
                id: pod.clone(),
                kind: NodeKind::Pod,
                x: 0.0,
                y: 0.0,
                tx: 0.0,
                ty: 0.0,
            });
            for s in svcs {
                if seen.insert(s.clone()) {
                    self.nodes.entry(s.clone()).or_insert(Node {
                        id: s.clone(),
                        kind: NodeKind::Service,
                        x: 0.0,
                        y: 0.0,
                        tx: 0.0,
                        ty: 0.0,
                    });
                }
                self.edges.push(Edge {
                    from: pod.clone(),
                    to: s.clone(),
                });
            }
        }
        for (pod, exts) in &self.data.external {
            self.nodes.entry(pod.clone()).or_insert(Node {
                id: pod.clone(),
                kind: NodeKind::Pod,
                x: 0.0,
                y: 0.0,
                tx: 0.0,
                ty: 0.0,
            });
            for d in exts {
                if seen.insert(d.clone()) {
                    self.nodes.entry(d.clone()).or_insert(Node {
                        id: d.clone(),
                        kind: NodeKind::External,
                        x: 0.0,
                        y: 0.0,
                        tx: 0.0,
                        ty: 0.0,
                    });
                }
                self.edges.push(Edge {
                    from: d.clone(),
                    to: pod.clone(),
                }); // external -> pod (outer to middle)
            }
        }
        // initialize sprinkled positions to avoid popping
        use rand::{Rng, SeedableRng, rngs::StdRng};
        let mut rng = StdRng::seed_from_u64(42);
        for n in self.nodes.values_mut() {
            n.x = rng.random_range(-1.0..1.0);
            n.y = rng.random_range(-1.0..1.0);
            n.tx = n.x;
            n.ty = n.y;
        }
    }

    fn recompute_targets(&mut self) {
        // Radial onion: radius per layer; compact when filters applied
        let (r_ext, r_pod, r_svc) = (0.95, 0.55, 0.15);

        // Filter active sets
        let mut allowed_pods: Option<HashSet<String>> = None;
        if let Some(p) = &self.filters.pod {
            allowed_pods = Some(HashSet::from([p.clone()]));
        }
        if let Some(svc) = &self.filters.service {
            let pods: HashSet<String> = self
                .data
                .internal
                .iter()
                .filter_map(|(p, svcs)| {
                    if svcs.iter().any(|s| s.contains(svc)) {
                        Some(p.clone())
                    } else {
                        None
                    }
                })
                .collect();
            allowed_pods = Some(match allowed_pods {
                Some(a) => &a & &pods,
                None => pods,
            });
        }
        if let Some(ext) = &self.filters.external {
            let pods: HashSet<String> = self
                .data
                .external
                .iter()
                .filter_map(|(p, ex)| {
                    if ex.iter().any(|d| d.contains(ext)) {
                        Some(p.clone())
                    } else {
                        None
                    }
                })
                .collect();
            allowed_pods = Some(match allowed_pods {
                Some(a) => &a & &pods,
                None => pods,
            });
        }
        let allowed_pods = allowed_pods; // immut

        // Collect lists by layer (filtered if needed)
        let externals: Vec<String> = self
            .nodes
            .values()
            .filter(|n| matches!(n.kind, NodeKind::External))
            .filter(|n| match &allowed_pods {
                Some(pods) => {
                    // keep only externals that connect to allowed pods
                    self.edges
                        .iter()
                        .any(|e| e.from == n.id && pods.contains(&e.to))
                }
                None => true,
            })
            .map(|n| n.id.clone())
            .collect();

        let pods: Vec<String> = self
            .nodes
            .values()
            .filter(|n| matches!(n.kind, NodeKind::Pod))
            .filter(|n| match &allowed_pods {
                Some(pods) => pods.contains(&n.id),
                None => true,
            })
            .map(|n| n.id.clone())
            .collect();

        let services: Vec<String> = self
            .nodes
            .values()
            .filter(|n| matches!(n.kind, NodeKind::Service))
            .filter(|n| match &allowed_pods {
                Some(pods) => {
                    // keep only services that are pointed by allowed pods
                    self.edges
                        .iter()
                        .any(|e| pods.contains(&e.from) && e.to == n.id)
                }
                None => true,
            })
            .map(|n| n.id.clone())
            .collect();

        // Place items evenly spaced on each circle
        place_on_circle(self, &externals, r_ext);
        place_on_circle(self, &pods, r_pod);
        place_on_circle(self, &services, r_svc);
    }
}

fn place_on_circle(app: &mut AppState, ids: &Vec<String>, radius: f64) {
    if ids.is_empty() {
        return;
    }
    let step = std::f64::consts::TAU / ids.len() as f64;
    for (i, id) in ids.iter().enumerate() {
        if let Some(n) = app.nodes.get_mut(id) {
            let ang = i as f64 * step;
            n.tx = radius * ang.cos();
            n.ty = radius * ang.sin();
        }
    }
}

fn animate(app: &mut AppState) {
    let now = Instant::now();
    let dt = (now - app.last_tick).as_secs_f64();
    app.last_tick = now;

    // simple critically damped spring/lERP blend for smoothness
    let speed = 8.0; // higher = snappier
    for n in app.nodes.values_mut() {
        n.x += (n.tx - n.x) * (1.0 - (-speed * dt).exp());
        n.y += (n.ty - n.y) * (1.0 - (-speed * dt).exp());
    }
}

fn ui(f: &mut ratatui::Frame, app: &mut AppState) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(size);

    // Header with tabs
    let titles = ["Graph", "Lists"].iter().map(|t| {
        Line::from(Span::styled(
            *t,
            Style::default().fg(PASTEL_3).add_modifier(Modifier::BOLD),
        ))
    });
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title("DNS Topology"))
        .select(app.tab)
        .highlight_style(Style::default().fg(PASTEL_1));
    f.render_widget(tabs, chunks[0]);

    match app.tab {
        0 => draw_graph(f, chunks[1], app),
        1 => draw_lists(f, chunks[1], app),
        _ => {}
    }

    draw_footer(f, chunks[2], app);

    // Modals for input
    match app.input_mode {
        InputMode::FilterPod
        | InputMode::FilterService
        | InputMode::FilterExternal
        | InputMode::ClearConfirm => {
            let area = centered_rect(60, 25, size);
            f.render_widget(Clear, area);
            let title = match app.input_mode {
                InputMode::FilterPod => "Filter: pod contains…",
                InputMode::FilterService => "Filter: service contains…",
                InputMode::FilterExternal => "Filter: external domain contains…",
                InputMode::ClearConfirm => "Press Enter to clear all filters",
                _ => "",
            };
            let p = Paragraph::new(app.input_buffer.as_str())
                .block(Block::default().borders(Borders::ALL).title(title))
                .wrap(Wrap { trim: true });
            f.render_widget(p, area);
        }
        _ => {}
    }
}

fn draw_graph(f: &mut ratatui::Frame, area: Rect, app: &AppState) {
    let canvas = Canvas::default()
        .x_bounds([-1.0, 1.0])
        .y_bounds([-1.0, 1.0])
        .paint(|ctx| {
            // background rings
            ctx.draw(&Circle {
                x: 0.0,
                y: 0.0,
                radius: 0.95,
                color: PASTEL_5,
            });
            ctx.draw(&Circle {
                x: 0.0,
                y: 0.0,
                radius: 0.55,
                color: PASTEL_4,
            });
            ctx.draw(&Circle {
                x: 0.0,
                y: 0.0,
                radius: 0.15,
                color: PASTEL_2,
            });

            // edges (draw first under nodes)
            for e in &app.edges {
                if let (Some(a), Some(b)) = (app.nodes.get(&e.from), app.nodes.get(&e.to)) {
                    // Filter visibility: if any filter set, hide irrelevant edges
                    if !edge_visible(app, a, b) {
                        continue;
                    }
                    ctx.draw(&CanvasLine {
                        x1: a.x,
                        y1: a.y,
                        x2: b.x,
                        y2: b.y,
                        color: PASTEL_EDGE,
                    });
                    // small arrow head toward b
                    let dirx = b.x - a.x;
                    let diry = b.y - a.y;
                    let len = (dirx * dirx + diry * diry).sqrt();
                    if len > 0.0 {
                        let ux = dirx / len;
                        let uy = diry / len;
                        ctx.draw(&Points {
                            coords: &[(b.x - ux * 0.02, b.y - uy * 0.02)],
                            color: PASTEL_EDGE,
                        });
                    }
                }
            }

            // nodes
            for n in app.nodes.values() {
                if !node_visible(app, n) {
                    continue;
                }
                let (c, r) = match n.kind {
                    NodeKind::External => (PASTEL_1, 0.012),
                    NodeKind::Pod => (PASTEL_3, 0.014),
                    NodeKind::Service => (PASTEL_6, 0.016),
                };
                ctx.draw(&Circle {
                    x: n.x,
                    y: n.y,
                    radius: r,
                    color: c,
                });
            }

            // labels on top for pods (most useful)
            for n in app.nodes.values() {
                if !node_visible(app, n) {
                    continue;
                }
                if matches!(n.kind, NodeKind::Pod) {
                    ctx.print(
                        n.x,
                        n.y,
                        Span::styled(truncate(&n.id, 16), Style::default().fg(Color::White)),
                    );
                }
            }
        })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Onion Graph (outer: external, middle: pods, inner: services)"),
        );
    f.render_widget(canvas, area);
}

fn edge_visible(app: &AppState, a: &Node, b: &Node) -> bool {
    match (
        &app.filters.pod,
        &app.filters.service,
        &app.filters.external,
    ) {
        (None, None, None) => true,
        _ => {
            let mut ok = true;
            if let Some(p) = &app.filters.pod {
                ok &= a.id.contains(p) || b.id.contains(p);
            }
            if let Some(s) = &app.filters.service {
                ok &= a.id.contains(s) || b.id.contains(s);
            }
            if let Some(e) = &app.filters.external {
                ok &= a.id.contains(e) || b.id.contains(e);
            }
            ok
        }
    }
}

fn node_visible(app: &AppState, n: &Node) -> bool {
    match (
        &app.filters.pod,
        &app.filters.service,
        &app.filters.external,
    ) {
        (None, None, None) => true,
        _ => match n.kind {
            NodeKind::Pod => {
                app.filters
                    .pod
                    .as_ref()
                    .map(|s| n.id.contains(s))
                    .unwrap_or(true)
                    && app
                        .filters
                        .service
                        .as_ref()
                        .map(|s| n.id.contains(s))
                        .unwrap_or(true)
                    && app
                        .filters
                        .external
                        .as_ref()
                        .map(|s| n.id.contains(s))
                        .unwrap_or(true)
            }
            NodeKind::Service => app
                .filters
                .service
                .as_ref()
                .map(|s| n.id.contains(s))
                .unwrap_or(true),
            NodeKind::External => app
                .filters
                .external
                .as_ref()
                .map(|s| n.id.contains(s))
                .unwrap_or(true),
        },
    }
}

fn draw_lists(f: &mut ratatui::Frame, area: Rect, app: &AppState) {
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(area);

    let mk = |title: &'static str, items: Vec<String>| {
        let lines: Vec<Line> = items
            .into_iter()
            .map(|s| Line::from(Span::raw(s)))
            .collect();
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: true })
    };

    let ext = app
        .nodes
        .values()
        .filter(|n| matches!(n.kind, NodeKind::External))
        .map(|n| n.id.clone())
        .collect();
    let pods = app
        .nodes
        .values()
        .filter(|n| matches!(n.kind, NodeKind::Pod))
        .map(|n| n.id.clone())
        .collect();
    let svcs = app
        .nodes
        .values()
        .filter(|n| matches!(n.kind, NodeKind::Service))
        .map(|n| n.id.clone())
        .collect();

    f.render_widget(mk("External domains", ext), layout[0]);
    f.render_widget(mk("Pods", pods), layout[1]);
    f.render_widget(mk("Services", svcs), layout[2]);
}

fn draw_footer(f: &mut ratatui::Frame, area: Rect, app: &AppState) {
    let filter_line = format!(
        "Filters — pod: {} | service: {} | external: {}",
        app.filters.pod.as_deref().unwrap_or("(none)"),
        app.filters.service.as_deref().unwrap_or("(none)"),
        app.filters.external.as_deref().unwrap_or("(none)")
    );

    let help = "[1] Graph  [2] Lists   [/] Pod filter   [s] Service filter   [e] External filter   [Ctrl+C] Clear filters   [q] Quit";

    let p = Paragraph::new(vec![
        Line::from(Span::styled(filter_line, Style::default().fg(Color::White))),
        Line::from(Span::styled(help, Style::default().fg(Color::DarkGray))),
    ])
    .block(Block::default().borders(Borders::ALL).title("Status"))
    .alignment(Alignment::Left);
    f.render_widget(p, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

// Pastel palette
const PASTEL_1: Color = Color::Rgb(186, 220, 212); // mint
const PASTEL_2: Color = Color::Rgb(255, 214, 165); // peach
const PASTEL_3: Color = Color::Rgb(199, 206, 234); // lavender
const PASTEL_4: Color = Color::Rgb(253, 255, 182); // butter
const PASTEL_5: Color = Color::Rgb(255, 179, 186); // rose
const PASTEL_6: Color = Color::Rgb(204, 255, 229); // aqua
const PASTEL_EDGE: Color = Color::Rgb(200, 200, 200);
