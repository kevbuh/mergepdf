use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame, Terminal,
};
use std::{
    io::stdout,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

#[derive(PartialEq)]
enum Screen {
    FolderBrowser,
    FileSelect,
    OutputInput,
    ConfirmOverwrite,
    Merging,
    Done,
    Error,
}

#[derive(Clone)]
enum DirEntry {
    ParentDir,
    Dir(PathBuf),
    Pdf(PathBuf),
}

impl DirEntry {
    fn display_name(&self) -> String {
        match self {
            DirEntry::ParentDir => "..".to_string(),
            DirEntry::Dir(p) => format!("{}/", p.file_name().unwrap().to_string_lossy()),
            DirEntry::Pdf(p) => p.file_name().unwrap().to_string_lossy().to_string(),
        }
    }

    fn is_pdf(&self) -> bool {
        matches!(self, DirEntry::Pdf(_))
    }
}

struct App {
    screen: Screen,
    // Folder browser
    current_dir: PathBuf,
    entries: Vec<DirEntry>,
    browser_cursor: usize,
    browser_scroll: usize,
    // File select
    pdf_files: Vec<PathBuf>,
    selected: Vec<bool>,
    file_cursor: usize,
    file_scroll: usize,
    // Output
    output_input: String,
    output_cursor: usize,
    message: String,
    merge_result: Option<Arc<Mutex<Option<Result<u32, String>>>>>,
    merge_progress: Option<Arc<Mutex<MergeProgress>>>,
}

struct MergeProgress {
    current: usize,
    total: usize,
    current_file: String,
}

impl App {
    fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let mut app = Self {
            screen: Screen::FolderBrowser,
            current_dir: cwd,
            entries: Vec::new(),
            browser_cursor: 0,
            browser_scroll: 0,
            pdf_files: Vec::new(),
            selected: Vec::new(),
            file_cursor: 0,
            file_scroll: 0,
            output_input: String::from("merged.pdf"),
            output_cursor: 10,
            message: String::new(),
            merge_result: None,
            merge_progress: None,
        };
        app.load_dir();
        app
    }

    fn load_dir(&mut self) {
        let mut entries = Vec::new();

        // Parent directory (unless at root)
        if self.current_dir.parent().is_some() {
            entries.push(DirEntry::ParentDir);
        }

        if let Ok(read_dir) = std::fs::read_dir(&self.current_dir) {
            let mut dirs = Vec::new();
            let mut pdfs = Vec::new();

            for entry in read_dir.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_dir() {
                    // Skip hidden directories
                    if let Some(name) = path.file_name() {
                        if !name.to_string_lossy().starts_with('.') {
                            dirs.push(path);
                        }
                    }
                } else if path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
                {
                    pdfs.push(path);
                }
            }

            dirs.sort();
            pdfs.sort();

            for d in dirs {
                entries.push(DirEntry::Dir(d));
            }
            for p in pdfs {
                entries.push(DirEntry::Pdf(p));
            }
        }

        self.entries = entries;
        self.browser_cursor = 0;
        self.browser_scroll = 0;
    }

    fn enter_dir(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        match &self.entries[self.browser_cursor] {
            DirEntry::ParentDir => {
                if let Some(parent) = self.current_dir.parent() {
                    self.current_dir = parent.to_path_buf();
                    self.load_dir();
                }
            }
            DirEntry::Dir(path) => {
                self.current_dir = path.clone();
                self.load_dir();
            }
            DirEntry::Pdf(_) => {} // Can't enter a file
        }
    }

    fn select_folder(&mut self) {
        // If cursor is on a subdirectory, navigate into it first
        if !self.entries.is_empty() {
            if let DirEntry::Dir(path) = &self.entries[self.browser_cursor] {
                self.current_dir = path.clone();
                self.load_dir();
            }
        }

        let pdfs: Vec<PathBuf> = self
            .entries
            .iter()
            .filter_map(|e| match e {
                DirEntry::Pdf(p) => Some(p.clone()),
                _ => None,
            })
            .collect();

        if pdfs.is_empty() {
            self.message = "No PDF files in this folder".to_string();
            self.screen = Screen::Error;
            return;
        }

        self.selected = vec![true; pdfs.len()];
        self.pdf_files = pdfs;
        self.file_cursor = 0;
        self.file_scroll = 0;
        self.screen = Screen::FileSelect;
    }

    fn pdf_count_in_current(&self) -> usize {
        self.entries.iter().filter(|e| e.is_pdf()).count()
    }

    fn selected_count(&self) -> usize {
        self.selected.iter().filter(|&&s| s).count()
    }

    fn toggle_current(&mut self) {
        if !self.pdf_files.is_empty() {
            self.selected[self.file_cursor] = !self.selected[self.file_cursor];
        }
    }

    fn toggle_all(&mut self) {
        let all_selected = self.selected.iter().all(|&s| s);
        for s in &mut self.selected {
            *s = !all_selected;
        }
    }

    fn check_and_merge(&mut self) {
        let output = PathBuf::from(&self.output_input);
        // Also check if output name conflicts with any selected input file
        let is_input_file = self
            .pdf_files
            .iter()
            .enumerate()
            .any(|(i, p)| self.selected[i] && p.file_name() == output.file_name());

        if is_input_file {
            self.message = format!(
                "'{}' conflicts with an input file. Please choose a different name.",
                self.output_input
            );
            self.screen = Screen::Error;
            return;
        }

        if output.exists() {
            self.screen = Screen::ConfirmOverwrite;
            return;
        }

        self.start_merge();
    }

    fn start_merge(&mut self) {
        let files: Vec<PathBuf> = self
            .pdf_files
            .iter()
            .enumerate()
            .filter(|(i, _)| self.selected[*i])
            .map(|(_, p)| p.clone())
            .collect();

        let output = PathBuf::from(&self.output_input);
        let result = Arc::new(Mutex::new(None));
        self.merge_result = Some(Arc::clone(&result));

        let file_count = files.len();
        let progress = Arc::new(Mutex::new(MergeProgress {
            current: 0,
            total: file_count,
            current_file: String::new(),
        }));
        self.merge_progress = Some(Arc::clone(&progress));

        thread::spawn(move || {
            let r = match merge_pdfs(&files, &output, &progress) {
                Ok(page_count) => Ok(page_count),
                Err(e) => Err(e.to_string()),
            };
            *result.lock().unwrap() = Some(r);
        });

        self.message = format!("{}", file_count);
        self.screen = Screen::Merging;
    }

    fn check_merge_done(&mut self) {
        if let Some(ref result) = self.merge_result {
            if let Ok(guard) = result.try_lock() {
                if let Some(ref r) = *guard {
                    match r {
                        Ok(page_count) => {
                            let file_count = self
                                .selected
                                .iter()
                                .filter(|&&s| s)
                                .count();
                            self.message = format!(
                                "Merged {} pages from {} files into '{}'",
                                page_count, file_count, self.output_input
                            );
                            self.screen = Screen::Done;
                        }
                        Err(e) => {
                            self.message = format!("Merge failed: {}", e);
                            self.screen = Screen::Error;
                        }
                    }
                }
            }
        }
    }
}

fn find_merge_backend() -> &'static str {
    // Prefer pdfunite (structural merge, much faster) over gs (re-encodes)
    for cmd in &["pdfunite", "gs"] {
        if std::process::Command::new(cmd)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return cmd;
        }
    }
    "gs"
}

fn merge_pdfs(
    files: &[PathBuf],
    output: &PathBuf,
    progress: &Arc<Mutex<MergeProgress>>,
) -> Result<u32, Box<dyn std::error::Error>> {
    if files.is_empty() {
        return Err("No files to merge".into());
    }

    let total = files.len();
    {
        let mut p = progress.lock().unwrap();
        p.total = total;
        p.current = 0;
    }

    if files.len() == 1 {
        let name = files[0].file_name().unwrap().to_string_lossy().to_string();
        {
            let mut p = progress.lock().unwrap();
            p.current = 1;
            p.current_file = name;
        }
        std::fs::copy(&files[0], output)?;
        let doc = lopdf::Document::load(output)?;
        return Ok(doc.get_pages().len() as u32);
    }

    let backend = find_merge_backend();

    // Update progress with file names as we prepare
    for (i, file) in files.iter().enumerate() {
        let mut p = progress.lock().unwrap();
        p.current = i;
        p.current_file = file.file_name().unwrap().to_string_lossy().to_string();
    }

    {
        let mut p = progress.lock().unwrap();
        p.current_file = format!("Merging with {}...", backend);
    }

    let result = if backend == "pdfunite" {
        // pdfunite input1.pdf input2.pdf ... output.pdf
        let mut args: Vec<String> = files.iter().map(|f| f.to_string_lossy().to_string()).collect();
        args.push(output.to_string_lossy().to_string());
        std::process::Command::new("pdfunite").args(&args).output()?
    } else {
        // gs -dBATCH -dNOPAUSE -q -sDEVICE=pdfwrite -sOutputFile=out input1 input2 ...
        let mut args = vec![
            "-dBATCH".to_string(),
            "-dNOPAUSE".to_string(),
            "-q".to_string(),
            "-sDEVICE=pdfwrite".to_string(),
            format!("-sOutputFile={}", output.display()),
        ];
        for file in files {
            args.push(file.to_string_lossy().to_string());
        }
        std::process::Command::new("gs").args(&args).output()?
    };

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(format!("{} failed: {}", backend, stderr).into());
    }

    {
        let mut p = progress.lock().unwrap();
        p.current = total;
        p.current_file = "Counting pages...".to_string();
    }

    let doc = lopdf::Document::load(output)?;
    let page_count = doc.get_pages().len() as u32;

    {
        let mut p = progress.lock().unwrap();
        p.current = total;
        p.current_file = "Done!".to_string();
    }

    Ok(page_count)
}

fn scroll_cursor(cursor: usize, scroll: &mut usize, visible: usize) {
    if cursor < *scroll {
        *scroll = cursor;
    } else if cursor >= *scroll + visible {
        *scroll = cursor - visible + 1;
    }
}

fn draw(frame: &mut Frame, app: &App) {
    frame.render_widget(Clear, frame.area());

    match app.screen {
        Screen::FolderBrowser => draw_folder_browser(frame, app, frame.area()),
        Screen::FileSelect => draw_file_select(frame, app, frame.area()),
        Screen::OutputInput => draw_output_input(frame, app, frame.area()),
        Screen::ConfirmOverwrite => draw_confirm_overwrite(frame, app, frame.area()),
        Screen::Merging => draw_merging(frame, app, frame.area()),
        Screen::Done => draw_message(frame, &app.message, Color::Green, frame.area()),
        Screen::Error => draw_message(frame, &app.message, Color::Red, frame.area()),
    }
}

fn draw_folder_browser(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .margin(1)
    .split(area);

    let header = Paragraph::new(Line::from(vec![
        Span::styled("mergepdf", Style::default().fg(Color::Cyan).bold()),
        Span::raw("  "),
        Span::styled(
            app.current_dir.to_string_lossy().to_string(),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{} PDFs", app.pdf_count_in_current()),
            Style::default().fg(Color::Yellow),
        ),
    ]));
    frame.render_widget(header, chunks[0]);

    let visible_height = chunks[1].height.saturating_sub(2) as usize;

    let items: Vec<Line> = app
        .entries
        .iter()
        .enumerate()
        .skip(app.browser_scroll)
        .take(visible_height)
        .map(|(i, entry)| {
            let is_cursor = i == app.browser_cursor;
            let name = entry.display_name();

            let (icon, color) = match entry {
                DirEntry::ParentDir => ("  ", Color::Blue),
                DirEntry::Dir(_) => ("  ", Color::Blue),
                DirEntry::Pdf(_) => ("  ", Color::White),
            };

            let style = if is_cursor {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(color)
            };

            let prefix = if is_cursor { "> " } else { "  " };

            Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(icon, Style::default().fg(color)),
                Span::styled(name, style),
            ])
        })
        .collect();

    let list = Paragraph::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Browse folders "),
    );
    frame.render_widget(list, chunks[1]);

    let help = Paragraph::new(Line::from(vec![
        Span::styled("↑↓", Style::default().fg(Color::Cyan)),
        Span::raw(" navigate  "),
        Span::styled("enter", Style::default().fg(Color::Cyan)),
        Span::raw(" open folder  "),
        Span::styled("s", Style::default().fg(Color::Cyan)),
        Span::raw(" select this folder  "),
        Span::styled("esc", Style::default().fg(Color::Cyan)),
        Span::raw(" quit"),
    ]))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[2]);
}

fn draw_file_select(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .margin(1)
    .split(area);

    let header = Paragraph::new(Line::from(vec![
        Span::styled("mergepdf", Style::default().fg(Color::Cyan).bold()),
        Span::raw("  "),
        Span::styled(
            format!(
                "{}/{} selected",
                app.selected_count(),
                app.pdf_files.len()
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(header, chunks[0]);

    let visible_height = chunks[1].height.saturating_sub(2) as usize;

    let items: Vec<Line> = app
        .pdf_files
        .iter()
        .enumerate()
        .skip(app.file_scroll)
        .take(visible_height)
        .map(|(i, path)| {
            let name = path.file_name().unwrap().to_string_lossy();
            let checkbox = if app.selected[i] { "[x]" } else { "[ ]" };
            let is_cursor = i == app.file_cursor;

            let style = if is_cursor {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if app.selected[i] {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let prefix = if is_cursor { "> " } else { "  " };

            Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(
                    checkbox,
                    if app.selected[i] {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::raw(" "),
                Span::styled(name.to_string(), style),
            ])
        })
        .collect();

    let list = Paragraph::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Select PDFs "),
    );
    frame.render_widget(list, chunks[1]);

    let help = Paragraph::new(Line::from(vec![
        Span::styled("space", Style::default().fg(Color::Cyan)),
        Span::raw(" toggle  "),
        Span::styled("a", Style::default().fg(Color::Cyan)),
        Span::raw(" all  "),
        Span::styled("enter", Style::default().fg(Color::Cyan)),
        Span::raw(" next  "),
        Span::styled("esc", Style::default().fg(Color::Cyan)),
        Span::raw(" back"),
    ]))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[2]);
}

fn draw_output_input(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .margin(1)
    .split(area);

    let title = Paragraph::new(Line::from(vec![
        Span::styled("mergepdf", Style::default().fg(Color::Cyan).bold()),
        Span::raw("  "),
        Span::styled(
            format!("{} files selected", app.selected_count()),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(title, chunks[0]);

    let label = Paragraph::new(Span::styled(
        "Name your output file:",
        Style::default().fg(Color::White),
    ));
    frame.render_widget(label, chunks[1]);

    let input = Paragraph::new(Line::from(vec![
        Span::raw(&app.output_input[..app.output_cursor]),
        Span::styled(
            if app.output_cursor < app.output_input.len() {
                &app.output_input[app.output_cursor..app.output_cursor + 1]
            } else {
                " "
            },
            Style::default().bg(Color::White).fg(Color::Black),
        ),
        Span::raw(if app.output_cursor < app.output_input.len() {
            &app.output_input[app.output_cursor + 1..]
        } else {
            ""
        }),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Output filename "),
    );
    frame.render_widget(input, chunks[2]);

    let help = Paragraph::new(Line::from(vec![
        Span::styled("enter", Style::default().fg(Color::Cyan)),
        Span::raw(" merge  "),
        Span::styled("esc", Style::default().fg(Color::Cyan)),
        Span::raw(" back"),
    ]))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[3]);
}

fn draw_merging(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(0),
    ])
    .margin(1)
    .split(area);

    let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let idx = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        / 80) as usize
        % spinner_chars.len();

    let (current, total, current_file) = if let Some(ref progress) = app.merge_progress {
        if let Ok(p) = progress.try_lock() {
            (p.current, p.total, p.current_file.clone())
        } else {
            (0, app.selected_count(), String::new())
        }
    } else {
        (0, app.selected_count(), String::new())
    };

    let pct = if total > 0 {
        (current * 100) / total
    } else {
        0
    };

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            format!("{} ", spinner_chars[idx]),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled("Merging", Style::default().fg(Color::White).bold()),
        Span::raw("  "),
        Span::styled(
            format!("{}/{} files  {}%", current, total, pct),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(title, chunks[0]);

    // Progress bar
    let bar_width = chunks[1].width.saturating_sub(2) as usize;
    let filled = if total > 0 {
        (current * bar_width) / total
    } else {
        0
    };
    let empty = bar_width.saturating_sub(filled);

    let bar = Paragraph::new(Line::from(vec![
        Span::styled(
            "█".repeat(filled),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(
            "░".repeat(empty),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(bar, chunks[1]);

    if !current_file.is_empty() {
        let file_info = Paragraph::new(Line::from(vec![
            Span::styled("  → ", Style::default().fg(Color::DarkGray)),
            Span::styled(current_file, Style::default().fg(Color::White)),
        ]));
        frame.render_widget(file_info, chunks[2]);
    }
}

fn draw_confirm_overwrite(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(2)])
        .margin(1)
        .split(area);

    let text = Paragraph::new(Line::from(vec![
        Span::styled("'", Style::default().fg(Color::Yellow)),
        Span::styled(&app.output_input, Style::default().fg(Color::Yellow).bold()),
        Span::styled(
            "' already exists. Overwrite?",
            Style::default().fg(Color::Yellow),
        ),
    ]));
    frame.render_widget(text, chunks[0]);

    let help = Paragraph::new(Line::from(vec![
        Span::styled("y", Style::default().fg(Color::Cyan)),
        Span::raw(" overwrite  "),
        Span::styled("n/esc", Style::default().fg(Color::Cyan)),
        Span::raw(" go back and rename"),
    ]))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[1]);
}

fn draw_message(frame: &mut Frame, msg: &str, color: Color, area: Rect) {
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(2)])
        .margin(1)
        .split(area);

    let text = Paragraph::new(Line::from(Span::styled(
        msg,
        Style::default().fg(color).bold(),
    )));
    frame.render_widget(text, chunks[0]);

    let help = Paragraph::new(Line::from(vec![
        Span::styled("enter/esc", Style::default().fg(Color::Cyan)),
        Span::raw(" quit"),
    ]))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[1]);
}

fn handle_text_input(input: &mut String, cursor: &mut usize, code: KeyCode) {
    match code {
        KeyCode::Char(c) => {
            input.insert(*cursor, c);
            *cursor += 1;
        }
        KeyCode::Backspace => {
            if *cursor > 0 {
                *cursor -= 1;
                input.remove(*cursor);
            }
        }
        KeyCode::Delete => {
            if *cursor < input.len() {
                input.remove(*cursor);
            }
        }
        KeyCode::Left => {
            if *cursor > 0 {
                *cursor -= 1;
            }
        }
        KeyCode::Right => {
            if *cursor < input.len() {
                *cursor += 1;
            }
        }
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = input.len(),
        _ => {}
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;

    let backend = ratatui::backend::CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new();

    loop {
        terminal.draw(|f| draw(f, &app))?;

        // While merging, poll for completion with short timeout so spinner animates
        if app.screen == Screen::Merging {
            app.check_merge_done();
            if app.screen == Screen::Merging {
                // Poll for key events with a short timeout to keep spinner animating
                if event::poll(std::time::Duration::from_millis(80))? {
                    if let Event::Key(key) = event::read()? {
                        if key.kind == KeyEventKind::Press && key.code == KeyCode::Esc {
                            break;
                        }
                    }
                }
                continue;
            } else {
                continue;
            }
        }

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match app.screen {
                Screen::FolderBrowser => match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        if app.browser_cursor > 0 {
                            app.browser_cursor -= 1;
                            let visible = terminal.size()?.height.saturating_sub(8) as usize;
                            scroll_cursor(app.browser_cursor, &mut app.browser_scroll, visible);
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if app.browser_cursor + 1 < app.entries.len() {
                            app.browser_cursor += 1;
                            let visible = terminal.size()?.height.saturating_sub(8) as usize;
                            scroll_cursor(app.browser_cursor, &mut app.browser_scroll, visible);
                        }
                    }
                    KeyCode::Enter => app.enter_dir(),
                    KeyCode::Char('s') => app.select_folder(),
                    KeyCode::Backspace => {
                        // Go to parent
                        if app.current_dir.parent().is_some() {
                            app.current_dir = app.current_dir.parent().unwrap().to_path_buf();
                            app.load_dir();
                        }
                    }
                    KeyCode::Esc => break,
                    _ => {}
                },

                Screen::FileSelect => match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        if app.file_cursor > 0 {
                            app.file_cursor -= 1;
                            let visible = terminal.size()?.height.saturating_sub(8) as usize;
                            scroll_cursor(app.file_cursor, &mut app.file_scroll, visible);
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if app.file_cursor + 1 < app.pdf_files.len() {
                            app.file_cursor += 1;
                            let visible = terminal.size()?.height.saturating_sub(8) as usize;
                            scroll_cursor(app.file_cursor, &mut app.file_scroll, visible);
                        }
                    }
                    KeyCode::Char(' ') => app.toggle_current(),
                    KeyCode::Char('a') => app.toggle_all(),
                    KeyCode::Enter => {
                        if app.selected_count() == 0 {
                            app.message = "Select at least one file".to_string();
                            app.screen = Screen::Error;
                        } else {
                            app.screen = Screen::OutputInput;
                        }
                    }
                    KeyCode::Esc => {
                        app.screen = Screen::FolderBrowser;
                    }
                    _ => {}
                },

                Screen::OutputInput => match key.code {
                    KeyCode::Enter => {
                        if app.output_input.trim().is_empty() {
                            app.output_input = "merged.pdf".to_string();
                            app.output_cursor = app.output_input.len();
                        }
                        app.check_and_merge();
                    }
                    KeyCode::Esc => app.screen = Screen::FileSelect,
                    other => handle_text_input(
                        &mut app.output_input,
                        &mut app.output_cursor,
                        other,
                    ),
                },

                Screen::ConfirmOverwrite => match key.code {
                    KeyCode::Char('y') => app.start_merge(),
                    KeyCode::Char('n') | KeyCode::Esc => app.screen = Screen::OutputInput,
                    _ => {}
                },

                Screen::Merging => {} // handled above before event::read

                Screen::Done | Screen::Error => match key.code {
                    KeyCode::Enter | KeyCode::Esc => break,
                    _ => {}
                },
            }
        }
    }

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    if app.screen == Screen::Done {
        println!("{}", app.message);
    }

    Ok(())
}
