#![windows_subsystem = "windows"]

use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, Stroke, Vec2};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use unity_extractor::{extract_unitypackage, ExtractOptions, ExtractReport, QueueItem};

// Palette maquette A2
const BG: Color32 = Color32::from_rgb(18, 22, 28);
const PANEL: Color32 = Color32::from_rgb(28, 34, 44);
const PANEL_ALT: Color32 = Color32::from_rgb(22, 28, 36);
const CYAN: Color32 = Color32::from_rgb(46, 196, 255);
const CYAN_DIM: Color32 = Color32::from_rgb(30, 120, 160);
const TEXT: Color32 = Color32::from_rgb(235, 240, 245);
const TEXT_DIM: Color32 = Color32::from_rgb(150, 165, 180);
const OK_COL: Color32 = Color32::from_rgb(110, 210, 150);
const WARN_COL: Color32 = Color32::from_rgb(255, 200, 90);
const ERR_COL: Color32 = Color32::from_rgb(255, 110, 110);
const INPUT_BG: Color32 = Color32::from_rgb(36, 44, 56);

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1024.0, 600.0])
            .with_min_inner_size([1024.0, 600.0])
            .with_max_inner_size([1024.0, 600.0])
            .with_resizable(false)
            .with_title("Unity Package Extractor"),
        ..Default::default()
    };

    eframe::run_native(
        "Unity Package Extractor",
        options,
        Box::new(|cc| {
            configure_style(&cc.egui_ctx);
            Ok(Box::new(App::default()))
        }),
    )
}

fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.visuals = egui::Visuals::dark();
    style.visuals.window_fill = BG;
    style.visuals.panel_fill = BG;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.widgets.inactive.bg_fill = INPUT_BG;
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(48, 62, 82);
    style.visuals.widgets.active.bg_fill = CYAN_DIM;
    style.visuals.selection.bg_fill = CYAN_DIM;
    style.visuals.widgets.noninteractive.bg_stroke =
        Stroke::new(1.0_f32, Color32::from_rgb(55, 70, 90));
    ctx.set_style(style);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LogLevel {
    Info,
    Ok,
    Warn,
    Error,
}

struct LogLine {
    level: LogLevel,
    text: String,
}

enum WorkerMsg {
    Progress {
        index: usize,
        total: usize,
        fraction: f32,
        message: String,
    },
    ItemDone {
        index: usize,
        folder_name: String,
        result: Result<ExtractReport, String>,
    },
    QueueDone { total: usize },
}

#[derive(Clone)]
struct QueueRow {
    item: QueueItem,
}

struct App {
    package_path_str: String,
    output_parent_str: String,
    draft_folder_name: String,
    queue: Vec<QueueRow>,
    include_meta: bool,
    include_preview: bool,
    parallel: bool,
    max_workers: usize,
    busy: bool,
    progress: f32,
    status: String,
    footer: String,
    logs: Vec<LogLine>,
    drag_hover: bool,
    rx: Option<Receiver<WorkerMsg>>,
    last_logged_progress: String,
    done_count: usize,
    total_jobs: usize,
}

impl Default for App {
    fn default() -> Self {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get().clamp(2, 8))
            .unwrap_or(4);
        Self {
            package_path_str: String::new(),
            output_parent_str: String::new(),
            draft_folder_name: String::new(),
            queue: Vec::new(),
            include_meta: true,
            include_preview: false,
            parallel: true,
            max_workers: workers,
            busy: false,
            progress: 0.0,
            status: String::new(),
            footer: "Prêt.".into(),
            logs: Vec::new(),
            drag_hover: false,
            rx: None,
            last_logged_progress: String::new(),
            done_count: 0,
            total_jobs: 0,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker(ctx);

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::same(14)))
            .show(ctx, |ui| {
                // Header
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("Unity Package Extractor")
                            .size(22.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        RichText::new("Extract contents from Unity .unitypackage files.")
                            .size(12.0)
                            .color(TEXT_DIM),
                    );
                });
                ui.add_space(8.0);

                self.drop_zone(ui, ctx);
                ui.add_space(8.0);

                // Mid: left form + right queue
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(490.0, 195.0),
                        Layout::top_down(Align::Min),
                        |ui| self.left_form(ui),
                    );
                    ui.add_space(8.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(480.0, 195.0),
                        Layout::top_down(Align::Min),
                        |ui| self.queue_panel(ui),
                    );
                });

                ui.add_space(6.0);

                // Bottom: progress/actions + logs
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(320.0, 210.0),
                        Layout::top_down(Align::Min),
                        |ui| self.progress_panel(ui),
                    );
                    ui.add_space(8.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(650.0, 210.0),
                        Layout::top_down(Align::Min),
                        |ui| self.logs_panel(ui),
                    );
                });
            });

        if self.busy {
            ctx.request_repaint();
        }
    }
}

impl App {
    fn push_log(&mut self, level: LogLevel, text: impl Into<String>) {
        let ts = timestamp_now();
        self.logs.push(LogLine {
            level,
            text: format!("[{ts}] {}", text.into()),
        });
        if self.logs.len() > 2000 {
            let n = self.logs.len() - 1500;
            self.logs.drain(0..n);
        }
    }

    fn drop_zone(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let desired = Vec2::new(ui.available_width(), 64.0);
        let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
        let hovered = response.hovered() || self.drag_hover;
        let stroke_col = if hovered { CYAN } else { CYAN_DIM };
        let fill = if hovered {
            Color32::from_rgb(24, 40, 52)
        } else {
            PANEL_ALT
        };

        ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
        // dashed-like border via stroke
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.5_f32, stroke_col),
            egui::StrokeKind::Inside,
        );

        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "⬇  Drop .unitypackage file here or click to browse",
            FontId::proportional(14.0),
            CYAN,
        );

        if response.clicked() && !self.busy {
            if let Some(paths) = rfd::FileDialog::new()
                .add_filter("Unity Package", &["unitypackage"])
                .pick_files()
            {
                for path in paths {
                    self.enqueue_path(path);
                }
            }
        }

        self.drag_hover = false;
        let mut dropped = Vec::new();
        ctx.input(|i| {
            if !i.raw.hovered_files.is_empty() {
                self.drag_hover = true;
            }
            for f in &i.raw.dropped_files {
                if let Some(p) = &f.path {
                    dropped.push(p.clone());
                }
            }
        });
        for p in dropped {
            self.enqueue_path(p);
        }
    }

    fn left_form(&mut self, ui: &mut egui::Ui) {
        Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_min_height(185.0);

                field_label(ui, "Package Path");
                ui.horizontal(|ui| {
                    path_edit(ui, &mut self.package_path_str, !self.busy, 380.0);
                    if ui
                        .add_enabled(!self.busy, cyan_icon_btn("📂"))
                        .clicked()
                    {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Unity Package", &["unitypackage"])
                            .pick_file()
                        {
                            self.set_package(path);
                        }
                    }
                });

                ui.add_space(4.0);
                field_label(ui, "Output Parent");
                ui.horizontal(|ui| {
                    path_edit(ui, &mut self.output_parent_str, !self.busy, 380.0);
                    if ui
                        .add_enabled(!self.busy, cyan_icon_btn("📂"))
                        .clicked()
                    {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.output_parent_str = path.display().to_string();
                        }
                    }
                });

                ui.add_space(4.0);
                field_label(ui, "Folder name (editable)");
                ui.add_enabled(
                    !self.busy,
                    egui::TextEdit::singleline(&mut self.draft_folder_name)
                        .desired_width(420.0)
                        .hint_text("HeroAssets"),
                );

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let can_add = !self.busy
                        && !self.package_path_str.trim().is_empty()
                        && !self.output_parent_str.trim().is_empty()
                        && !self.draft_folder_name.trim().is_empty();

                    if ui
                        .add_enabled(can_add, cyan_btn("+  Add to queue", Vec2::new(150.0, 28.0)))
                        .clicked()
                    {
                        self.add_draft_to_queue();
                    }

                    ui.add_space(12.0);
                    ui.checkbox(&mut self.include_meta, "Include .meta");
                    ui.checkbox(&mut self.include_preview, "Include previews");
                });
            });
    }

    fn queue_panel(&mut self, ui: &mut egui::Ui) {
        Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Extraction queue").strong().color(TEXT));
                    ui.label(
                        RichText::new(format!("{} items", self.queue.len()))
                            .color(CYAN)
                            .strong(),
                    );
                });
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .max_height(150.0)
                    .auto_shrink([false, false])
                    .id_salt("queue_scroll")
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        if self.queue.is_empty() {
                            ui.label(
                                RichText::new("Queue is empty — drop files or use Add to queue.")
                                    .color(TEXT_DIM),
                            );
                            return;
                        }

                        let busy = self.busy;
                        let mut remove_idx = None;
                        for (i, row) in self.queue.iter_mut().enumerate() {
                            Frame::new()
                                .fill(PANEL_ALT)
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(Margin::symmetric(8, 6))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new("📦").size(16.0));
                                        ui.vertical(|ui| {
                                            let name = row
                                                .item
                                                .package_path
                                                .file_name()
                                                .map(|n| n.to_string_lossy().into_owned())
                                                .unwrap_or_default();
                                            ui.label(RichText::new(name).strong().color(TEXT));
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new("→").small().color(TEXT_DIM),
                                                );
                                                ui.add_enabled(
                                                    !busy,
                                                    egui::TextEdit::singleline(
                                                        &mut row.item.folder_name,
                                                    )
                                                    .desired_width(220.0)
                                                    .font(FontId::monospace(12.0)),
                                                );
                                            });
                                            ui.label(
                                                RichText::new(
                                                    row.item.package_path.display().to_string(),
                                                )
                                                .small()
                                                .monospace()
                                                .color(TEXT_DIM),
                                            );
                                        });
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                let remove = ui.add_enabled(
                                                    !busy,
                                                    egui::Button::new(
                                                        RichText::new("✕").color(ERR_COL),
                                                    )
                                                    .fill(Color32::TRANSPARENT),
                                                );
                                                if remove.clicked() {
                                                    remove_idx = Some(i);
                                                }
                                            },
                                        );
                                    });
                                });
                            ui.add_space(4.0);
                        }
                        if let Some(i) = remove_idx {
                            let removed = self.queue.remove(i);
                            self.push_log(
                                LogLevel::Info,
                                format!(
                                    "Removed from queue: {}",
                                    removed.item.folder_name
                                ),
                            );
                        }
                    });
            });
    }

    fn progress_panel(&mut self, ui: &mut egui::Ui) {
        Frame::new()
            .fill(PANEL)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_min_height(190.0);
                ui.label(RichText::new("Extraction progress").strong().color(TEXT));
                ui.add_space(6.0);

                let bar = egui::ProgressBar::new(self.progress)
                    .show_percentage()
                    .animate(self.busy)
                    .fill(CYAN);
                ui.add(bar);

                if self.total_jobs > 0 {
                    ui.label(
                        RichText::new(format!(
                            "{} of {} packages extracted.",
                            self.done_count, self.total_jobs
                        ))
                        .color(TEXT_DIM),
                    );
                } else {
                    ui.label(RichText::new(&self.status).color(TEXT_DIM));
                }

                ui.add_space(8.0);
                let can_run = !self.busy && !self.queue.is_empty();
                if ui
                    .add_enabled(
                        can_run,
                        cyan_btn("▶  Extract queue", Vec2::new(200.0, 34.0)),
                    )
                    .clicked()
                {
                    self.start_queue();
                }

                ui.label(
                    RichText::new(format!("{} package(s) queued.", self.queue.len()))
                        .color(CYAN)
                        .small(),
                );

                ui.add_space(6.0);
                ui.checkbox(
                    &mut self.parallel,
                    RichText::new("Multithread / parallel extract").color(TEXT),
                );
                if self.parallel {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Workers:").small().color(TEXT_DIM));
                        ui.add(
                            egui::DragValue::new(&mut self.max_workers)
                                .range(1..=16)
                                .speed(1),
                        );
                    });
                }

                if ui
                    .add_enabled(
                        !self.busy && !self.queue.is_empty(),
                        egui::Button::new("Clear queue"),
                    )
                    .clicked()
                {
                    self.queue.clear();
                    self.push_log(LogLevel::Info, "Queue cleared.");
                }
            });
    }

    fn logs_panel(&mut self, ui: &mut egui::Ui) {
        Frame::new()
            .fill(Color32::from_rgb(10, 12, 16))
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(45, 60, 80)))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Extraction logs").strong().color(CYAN));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button(RichText::new("Clear logs").small().color(TEXT_DIM))
                            .clicked()
                        {
                            self.logs.clear();
                        }
                    });
                });
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .id_salt("logs_scroll")
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        if self.logs.is_empty() {
                            ui.label(
                                RichText::new("Waiting for extraction…")
                                    .monospace()
                                    .color(TEXT_DIM),
                            );
                        }
                        for line in &self.logs {
                            let color = match line.level {
                                LogLevel::Info => TEXT_DIM,
                                LogLevel::Ok => OK_COL,
                                LogLevel::Warn => WARN_COL,
                                LogLevel::Error => ERR_COL,
                            };
                            ui.label(
                                RichText::new(&line.text)
                                    .monospace()
                                    .size(12.0)
                                    .color(color),
                            );
                        }
                    });

                ui.add_space(4.0);
                ui.label(RichText::new(&self.footer).small().color(CYAN));
            });
    }

    fn set_package(&mut self, path: PathBuf) {
        if !is_unitypackage(&path) {
            self.push_log(LogLevel::Error, "Invalid file: expected .unitypackage");
            return;
        }
        self.package_path_str = path.display().to_string();
        if self.output_parent_str.trim().is_empty() {
            if let Some(parent) = path.parent() {
                self.output_parent_str = parent.display().to_string();
            }
        }
        self.draft_folder_name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "extracted".into());
        self.push_log(
            LogLevel::Info,
            format!("Selected: {}", path.display()),
        );
    }

    fn add_draft_to_queue(&mut self) {
        let package = PathBuf::from(self.package_path_str.trim());
        let parent = PathBuf::from(self.output_parent_str.trim());
        if !package.exists() {
            self.push_log(LogLevel::Error, "Package path does not exist.");
            return;
        }
        let mut item = QueueItem::from_package(package, Some(parent));
        item.folder_name = self.draft_folder_name.trim().to_string();
        if item.folder_name.is_empty() {
            return;
        }

        let exists = self.queue.iter().any(|r| {
            r.item.package_path == item.package_path && r.item.folder_name == item.folder_name
        });
        if !exists {
            self.push_log(
                LogLevel::Info,
                format!(
                    "Queued: {} → {}",
                    item.package_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    item.folder_name
                ),
            );
            self.queue.push(QueueRow { item });
        }
        self.package_path_str.clear();
        self.draft_folder_name.clear();
    }

    fn enqueue_path(&mut self, path: PathBuf) {
        if !is_unitypackage(&path) {
            self.push_log(
                LogLevel::Warn,
                format!("Ignored (not .unitypackage): {}", path.display()),
            );
            return;
        }
        if self.output_parent_str.trim().is_empty() {
            if let Some(parent) = path.parent() {
                self.output_parent_str = parent.display().to_string();
            }
        }
        let parent = PathBuf::from(self.output_parent_str.trim());
        let item = QueueItem::from_package(path, Some(parent));
        let exists = self
            .queue
            .iter()
            .any(|r| r.item.package_path == item.package_path);
        if !exists {
            self.push_log(
                LogLevel::Info,
                format!(
                    "Queued: {} → {}",
                    item.package_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    item.folder_name
                ),
            );
            // Prefill draft for convenience
            self.draft_folder_name = item.folder_name.clone();
            self.package_path_str = item.package_path.display().to_string();
            self.queue.push(QueueRow { item });
        }
    }

    fn start_queue(&mut self) {
        if self.queue.is_empty() {
            return;
        }

        let items: Vec<QueueItem> = self.queue.iter().map(|r| r.item.clone()).collect();
        let options = ExtractOptions {
            include_meta: self.include_meta,
            include_preview: self.include_preview,
        };
        let parallel = self.parallel;
        let workers = if parallel {
            self.max_workers.clamp(1, 16)
        } else {
            1
        };

        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        self.rx = Some(rx);
        self.busy = true;
        self.progress = 0.0;
        self.done_count = 0;
        self.total_jobs = items.len();
        self.last_logged_progress.clear();
        self.status = format!("Extracting {} package(s)…", items.len());
        self.footer = if parallel {
            format!("Parallel extraction… {} workers", workers)
        } else {
            "Sequential extraction…".into()
        };
        self.push_log(
            LogLevel::Info,
            format!(
                "=== Start queue: {} package(s), workers={} ===",
                items.len(),
                workers
            ),
        );

        thread::spawn(move || run_queue_jobs(items, options, workers, tx));
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        let messages: Vec<WorkerMsg> = {
            let Some(rx) = &self.rx else {
                return;
            };
            let mut batch = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                batch.push(msg);
            }
            batch
        };
        if messages.is_empty() {
            return;
        }

        let mut finished_total = None;
        for msg in messages {
            match msg {
                WorkerMsg::Progress {
                    index,
                    total,
                    fraction,
                    message,
                } => {
                    // Approx overall progress from done + current
                    let overall = (self.done_count as f32 + fraction) / total.max(1) as f32;
                    self.progress = overall.clamp(0.0, 0.99);
                    let status = format!("[{}/{}] {message}", index + 1, total);
                    self.status = status.clone();

                    let milestone = (fraction * 10.0).floor() as i32;
                    let key = format!("{index}:{milestone}");
                    if key != self.last_logged_progress {
                        self.last_logged_progress = key;
                        self.push_log(LogLevel::Info, status);
                    }
                }
                WorkerMsg::ItemDone {
                    index,
                    folder_name,
                    result,
                } => {
                    self.done_count += 1;
                    match result {
                        Ok(report) => {
                            self.push_log(
                                LogLevel::Ok,
                                format!(
                                    "OK {} files extracted — {} → {}",
                                    report.extracted,
                                    folder_name,
                                    report.output_dir.display()
                                ),
                            );
                            for warn in report.errors.iter().take(8) {
                                self.push_log(LogLevel::Warn, format!("WARNING {warn}"));
                            }
                            if report.errors.len() > 8 {
                                self.push_log(
                                    LogLevel::Warn,
                                    format!("… {} more warnings", report.errors.len() - 8),
                                );
                            }
                        }
                        Err(err) => {
                            self.push_log(
                                LogLevel::Error,
                                format!("FAILED [{}] {folder_name}: {err}", index + 1),
                            );
                        }
                    }
                    if self.total_jobs > 0 {
                        self.progress =
                            self.done_count as f32 / self.total_jobs as f32;
                        self.footer = format!(
                            "Extraction in progress… {} of {} completed",
                            self.done_count, self.total_jobs
                        );
                    }
                }
                WorkerMsg::QueueDone { total } => {
                    finished_total = Some(total);
                }
            }
        }

        if let Some(total) = finished_total {
            self.busy = false;
            self.rx = None;
            self.progress = 1.0;
            self.done_count = total;
            self.status = format!("Done — {total} package(s).");
            self.footer = format!("Extraction complete — {total} of {total} completed");
            self.push_log(LogLevel::Ok, format!("=== Finished: {total} package(s) ==="));
            self.queue.clear();
            ctx.request_repaint();
        }
    }
}

fn run_queue_jobs(
    items: Vec<QueueItem>,
    options: ExtractOptions,
    workers: usize,
    tx: Sender<WorkerMsg>,
) {
    let total = items.len();
    if total == 0 {
        let _ = tx.send(WorkerMsg::QueueDone { total: 0 });
        return;
    }

    let (job_tx, job_rx) = mpsc::channel::<(usize, QueueItem)>();
    let job_rx = Arc::new(Mutex::new(job_rx));
    let remaining = Arc::new(AtomicUsize::new(total));
    let worker_count = workers.clamp(1, total);

    for _ in 0..worker_count {
        let job_rx = Arc::clone(&job_rx);
        let tx = tx.clone();
        let options = options.clone();
        let remaining = Arc::clone(&remaining);

        thread::spawn(move || loop {
            let next = {
                let guard = job_rx.lock().unwrap();
                guard.recv()
            };
            let Ok((index, item)) = next else { break };

            let output = item.output_dir();
            let folder_name = item.folder_name.clone();
            let tx_progress = tx.clone();

            let result = extract_unitypackage(
                &item.package_path,
                &output,
                &options,
                |fraction, message| {
                    let _ = tx_progress.send(WorkerMsg::Progress {
                        index,
                        total,
                        fraction,
                        message,
                    });
                },
            )
            .map_err(|e| e.to_string());

            let _ = tx.send(WorkerMsg::ItemDone {
                index,
                folder_name,
                result,
            });

            if remaining.fetch_sub(1, Ordering::SeqCst) == 1 {
                let _ = tx.send(WorkerMsg::QueueDone { total });
            }
        });
    }

    for (index, item) in items.into_iter().enumerate() {
        let _ = job_tx.send((index, item));
    }
    drop(job_tx);
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().color(TEXT_DIM));
}

fn path_edit(ui: &mut egui::Ui, value: &mut String, enabled: bool, width: f32) {
    ui.add_enabled(
        enabled,
        egui::TextEdit::singleline(value)
            .desired_width(width)
            .font(FontId::monospace(12.0)),
    );
}

fn cyan_btn(label: &str, min: Vec2) -> egui::Button<'static> {
    egui::Button::new(RichText::new(label.to_owned()).strong().color(Color32::BLACK))
        .fill(CYAN)
        .corner_radius(CornerRadius::same(6))
        .min_size(min)
}

fn cyan_icon_btn(label: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(label.to_owned()).color(Color32::BLACK))
        .fill(CYAN)
        .corner_radius(CornerRadius::same(4))
        .min_size(Vec2::new(32.0, 24.0))
}

fn is_unitypackage(path: &PathBuf) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("unitypackage"))
        == Some(true)
}

fn timestamp_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    // UTC approx — fine for log stamps; local would need chrono
    format!("{h:02}:{m:02}:{s:02}")
}
