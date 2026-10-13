//! egui desktop UI: groups | snippet list | editor/playground/system-wide panes.

use eframe::egui::{
    self, text::CCursor, text::CCursorRange, Color32, RichText, TextEdit, TextFormat,
};
use std::time::{Duration, Instant};
use typelune_core::engine::{self, Engine, KeyInput, Lint};
use typelune_core::model::{Group, Library, Snippet, SnippetTrigger, TriggerMode};
use typelune_core::native::{self, NativeController, NativeState, Permissions};
use typelune_core::store::{self, LoadOutcome, Store};
use typelune_core::template::{self, RenderContext};
use uuid::Uuid;

const ACCENT: Color32 = Color32::from_rgb(0x2f, 0x7d, 0x6d);
const WARN: Color32 = Color32::from_rgb(0xc2, 0x7c, 0x0e);
const ERROR: Color32 = Color32::from_rgb(0xc0, 0x39, 0x2b);
const OK: Color32 = Color32::from_rgb(0x2e, 0x8b, 0x57);
const MAX_UNDO: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Editor,
    Playground,
    SystemWide,
    Settings,
    Help,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Editor,
        Tab::Playground,
        Tab::SystemWide,
        Tab::Settings,
        Tab::Help,
    ];
    fn label(self) -> &'static str {
        match self {
            Tab::Editor => "✏ Editor",
            Tab::Playground => "⌨ Playground",
            Tab::SystemWide => "🖥 System-wide",
            Tab::Settings => "⚙ Settings",
            Tab::Help => "❓ Syntax & rules",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupFilter {
    All,
    Group(Uuid),
}

struct PendingImport {
    path: String,
    library: Library,
}

struct Playground {
    text: String,
    prev_text: String,
    prev_cursor: usize,
    engine: Engine,
    log: Vec<String>,
    show_buffer: bool,
    pending_cursor: Option<usize>,
}

pub struct TypeluneApp {
    store: Store,
    library: Library,
    dirty_since: Option<Instant>,
    save_error: Option<String>,
    undo: Vec<(String, Library)>,
    status: String,
    filter: GroupFilter,
    selected: Option<Uuid>,
    search: String,
    tab: Tab,
    tags_buffer: String,
    tags_for: Option<Uuid>,
    lints: Vec<Lint>,
    playground: Playground,
    native: NativeController,
    perms: Permissions,
    perms_checked: Option<Instant>,
    layout_refreshed: Option<Instant>,
    native_message: Option<String>,
    pending_import: Option<PendingImport>,
    confirm_delete_group: Option<Uuid>,
    clipboard_cache: Option<(Instant, Option<String>)>,
}

impl TypeluneApp {
    pub fn new(cc: &eframe::CreationContext<'_>, data_dir: std::path::PathBuf) -> Self {
        install_fonts(&cc.egui_ctx);
        cc.egui_ctx.set_theme(egui::Theme::Light);

        let store = Store::new(data_dir); // explicit, isolated developer folder (see main.rs)
        let (library, status, save_error) = match store.load_or_init() {
            Ok(LoadOutcome::Loaded(l)) => (l, "Library loaded.".to_owned(), None),
            Ok(LoadOutcome::CreatedDemo(l)) => (
                l,
                "Created a new library with fabricated demo snippets.".to_owned(),
                None,
            ),
            Ok(LoadOutcome::Recovered {
                library,
                backup,
                error,
            }) => (
                library,
                format!(
                    "Existing library could not be read and was moved to {} — demo library created.",
                    backup.display()
                ),
                Some(error),
            ),
            Err(e) => (
                Library::demo(),
                "Could not open the library folder; working in memory only.".to_owned(),
                Some(e.to_string()),
            ),
        };
        let engine = Engine::new(&library);
        let lints = engine::lint(&library);
        let selected = library
            .groups
            .first()
            .and_then(|g| g.snippets.first())
            .map(|s| s.id);
        Self {
            store,
            library,
            dirty_since: None,
            save_error,
            undo: Vec::new(),
            status,
            filter: GroupFilter::All,
            selected,
            search: String::new(),
            tab: Tab::Editor,
            tags_buffer: String::new(),
            tags_for: None,
            lints,
            playground: Playground {
                text: String::new(),
                prev_text: String::new(),
                prev_cursor: 0,
                engine,
                log: Vec::new(),
                show_buffer: false,
                pending_cursor: None,
            },
            native: NativeController::new(),
            perms: Permissions::default(),
            perms_checked: None,
            layout_refreshed: None,
            native_message: None,
            pending_import: None,
            confirm_delete_group: None,
            clipboard_cache: None,
        }
    }

    // ---- library mutations -------------------------------------------------------------

    fn mark_changed(&mut self) {
        self.dirty_since = Some(Instant::now());
        self.playground.engine.load(&self.library);
        self.native.update_library(&self.library);
        self.lints = engine::lint(&self.library);
    }

    fn push_undo(&mut self, label: impl Into<String>) {
        self.undo.push((label.into(), self.library.clone()));
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
    }

    fn do_undo(&mut self) {
        if let Some((label, lib)) = self.undo.pop() {
            self.library = lib;
            self.status = format!("Undid: {label}");
            self.tags_for = None;
            self.mark_changed();
        }
    }

    fn save_if_due(&mut self, force: bool) {
        let Some(since) = self.dirty_since else {
            return;
        };
        if !force && since.elapsed() < Duration::from_millis(500) {
            return;
        }
        match self.store.save(&self.library) {
            Ok(()) => {
                self.save_error = None;
                self.dirty_since = None;
            }
            Err(e) => {
                self.save_error = Some(e.to_string());
                // Retry after the user fixes the problem.
                self.dirty_since = Some(Instant::now() + Duration::from_secs(2));
            }
        }
    }

    fn target_group_index(&mut self) -> usize {
        if let GroupFilter::Group(id) = self.filter {
            if let Some(i) = self.library.group_index(id) {
                return i;
            }
        }
        if let Some(sel) = self.selected {
            if let Some((gi, _)) = self.library.find_snippet(sel) {
                return gi;
            }
        }
        if self.library.groups.is_empty() {
            self.library.groups.push(Group::new("General"));
        }
        0
    }

    fn new_snippet(&mut self) {
        let gi = self.target_group_index();
        let s = Snippet::new("New snippet", "", "");
        let id = s.id;
        self.library.groups[gi].snippets.push(s);
        self.selected = Some(id);
        self.search.clear();
        self.tab = Tab::Editor;
        self.status = "Snippet created — give it an abbreviation.".into();
        self.mark_changed();
    }

    fn new_group(&mut self) {
        let mut n = 1;
        let name = loop {
            let candidate = if n == 1 {
                "New group".to_owned()
            } else {
                format!("New group {n}")
            };
            if !self.library.groups.iter().any(|g| g.name == candidate) {
                break candidate;
            }
            n += 1;
        };
        let g = Group::new(&name);
        self.filter = GroupFilter::Group(g.id);
        self.library.groups.push(g);
        self.status = format!("Created group '{name}'.");
        self.mark_changed();
    }

    fn duplicate_selected(&mut self) {
        let Some((gi, si)) = self.selected.and_then(|id| self.library.find_snippet(id)) else {
            return;
        };
        let mut copy = self.library.groups[gi].snippets[si].clone();
        copy.id = Uuid::new_v4();
        copy.title = format!("{} (copy)", copy.display_name());
        copy.enabled = false;
        let id = copy.id;
        self.library.groups[gi].snippets.insert(si + 1, copy);
        self.selected = Some(id);
        self.status = "Duplicated (copy starts disabled to avoid a duplicate abbreviation).".into();
        self.mark_changed();
    }

    fn delete_selected(&mut self) {
        let Some((gi, si)) = self.selected.and_then(|id| self.library.find_snippet(id)) else {
            return;
        };
        let name = self.library.groups[gi].snippets[si].display_name();
        self.push_undo(format!("delete snippet '{name}'"));
        self.library.groups[gi].snippets.remove(si);
        let g = &self.library.groups[gi];
        self.selected = g
            .snippets
            .get(si)
            .or_else(|| g.snippets.last())
            .map(|s| s.id);
        self.status = format!("Deleted '{name}'. Use Undo (⌘Z) to restore.");
        self.mark_changed();
    }

    fn delete_group(&mut self, id: Uuid) {
        let Some(gi) = self.library.group_index(id) else {
            return;
        };
        let name = self.library.groups[gi].name.clone();
        let count = self.library.groups[gi].snippets.len();
        self.push_undo(format!("delete group '{name}'"));
        self.library.groups.remove(gi);
        self.filter = GroupFilter::All;
        if self
            .selected
            .and_then(|s| self.library.find_snippet(s))
            .is_none()
        {
            self.selected = None;
        }
        self.status = format!("Deleted group '{name}' with {count} snippet(s). Undo is available.");
        self.mark_changed();
    }

    fn export(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export Typelune library")
            .set_file_name("typelune-export.json")
            .add_filter("JSON", &["json"])
            .save_file()
        else {
            return;
        };
        match store::export_to(&path, &self.library) {
            Ok(()) => {
                self.status = format!(
                    "Exported {} snippets to {}",
                    self.library.snippet_count(),
                    path.display()
                )
            }
            Err(e) => self.status = format!("Export failed: {e}"),
        }
    }

    fn import(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import Typelune library")
            .add_filter("JSON", &["json"])
            .pick_file()
        else {
            return;
        };
        match store::import_from(&path) {
            Ok(library) => {
                self.pending_import = Some(PendingImport {
                    path: path.display().to_string(),
                    library,
                })
            }
            Err(e) => self.status = format!("Import failed: {e}"),
        }
    }

    fn clipboard_text(&mut self) -> Option<String> {
        if let Some((t, v)) = &self.clipboard_cache {
            if t.elapsed() < Duration::from_secs(1) {
                return v.clone();
            }
        }
        let v = arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_text().ok());
        self.clipboard_cache = Some((Instant::now(), v.clone()));
        v
    }

    fn render_ctx(&mut self, body: &str) -> RenderContext {
        let clipboard = if template::uses_clipboard(body) {
            self.clipboard_text()
        } else {
            None
        };
        RenderContext::new(chrono::Local::now(), clipboard)
    }

    fn refresh_permissions(&mut self, force: bool) {
        let due = self
            .perms_checked
            .is_none_or(|t| t.elapsed() > Duration::from_secs(2));
        if force || due {
            self.perms = self.native.permissions();
            self.perms_checked = Some(Instant::now());
        }
    }

    // ---- UI ----------------------------------------------------------------------------

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Typelune").strong().size(18.0).color(ACCENT));
            ui.label(
                RichText::new(concat!("legacy egui UI · v", env!("CARGO_PKG_VERSION"))).weak(),
            );
            ui.separator();
            if ui
                .button("＋ Snippet")
                .on_hover_text("New snippet (⌘N)")
                .clicked()
            {
                self.new_snippet();
            }
            if ui.button("＋ Group").clicked() {
                self.new_group();
            }
            let has_sel = self.selected.is_some();
            if ui
                .add_enabled(has_sel, egui::Button::new("⧉ Duplicate"))
                .clicked()
            {
                self.duplicate_selected();
            }
            if ui
                .add_enabled(has_sel, egui::Button::new("🗑 Delete"))
                .on_hover_text("Delete the selected snippet (undoable)")
                .clicked()
            {
                self.delete_selected();
            }
            let undo_hint = self
                .undo
                .last()
                .map(|(l, _)| format!("Undo {l} (⌘Z)"))
                .unwrap_or_else(|| "Nothing to undo".into());
            if ui
                .add_enabled(!self.undo.is_empty(), egui::Button::new("↶ Undo"))
                .on_hover_text(undo_hint)
                .clicked()
            {
                self.do_undo();
            }
            ui.separator();
            if ui.button("⬇ Import…").clicked() {
                self.import();
            }
            if ui.button("⬆ Export…").clicked() {
                self.export();
            }
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            for tab in Tab::ALL {
                ui.selectable_value(&mut self.tab, tab, tab.label());
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let save = if let Some(err) = &self.save_error {
                RichText::new(format!("⚠ Save problem: {err}")).color(ERROR)
            } else if self.dirty_since.is_some() {
                RichText::new("● Saving…").color(WARN)
            } else {
                RichText::new("✔ Saved").color(OK)
            };
            ui.label(save);
            ui.separator();
            ui.label(format!(
                "{} snippets · {} active rules",
                self.library.snippet_count(),
                self.playground.engine.active_rule_count()
            ));
            ui.separator();
            let (txt, color) = native_state_label(&self.native.state());
            ui.label(RichText::new(format!("System-wide: {txt}")).color(color));
            ui.separator();
            ui.label(RichText::new(&self.status).weak());
        });
    }

    fn groups_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Groups");
        ui.add_space(4.0);
        let total = self.library.snippet_count();
        if ui
            .selectable_label(
                self.filter == GroupFilter::All,
                format!("📚 All snippets ({total})"),
            )
            .clicked()
        {
            self.filter = GroupFilter::All;
        }
        ui.separator();
        let mut changed = false;
        egui::ScrollArea::vertical()
            .id_salt("groups_scroll")
            .max_height((ui.available_height() - 150.0).max(80.0))
            .show(ui, |ui| {
                for g in &mut self.library.groups {
                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(&mut g.enabled, "")
                            .on_hover_text("Enable/disable every snippet in this group")
                            .changed()
                        {
                            changed = true;
                        }
                        let mut text = RichText::new(format!("{} ({})", g.name, g.snippets.len()));
                        if !g.enabled {
                            text = text.weak().strikethrough();
                        }
                        if ui
                            .selectable_label(self.filter == GroupFilter::Group(g.id), text)
                            .clicked()
                        {
                            self.filter = GroupFilter::Group(g.id);
                        }
                    });
                }
            });
        if changed {
            self.mark_changed();
        }

        if let GroupFilter::Group(id) = self.filter {
            if let Some(gi) = self.library.group_index(id) {
                ui.separator();
                ui.label(RichText::new("Selected group").strong());
                let mut name = self.library.groups[gi].name.clone();
                let resp = ui.add(
                    TextEdit::singleline(&mut name)
                        .char_limit(store::MAX_NAME_CHARS)
                        .hint_text("Group name"),
                );
                if resp.changed() && !name.trim().is_empty() {
                    self.library.groups[gi].name = name;
                    self.mark_changed();
                }
                if ui.button("🗑 Delete group…").clicked() {
                    self.confirm_delete_group = Some(id);
                }
            }
        }
    }

    fn snippet_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("🔍");
            ui.add(
                TextEdit::singleline(&mut self.search)
                    .hint_text("Search title, abbreviation, body, tags")
                    .desired_width(f32::INFINITY),
            );
        });
        ui.add_space(4.0);
        let lint_ids: Vec<Uuid> = self.lints.iter().map(|l| l.snippet_id).collect();
        let mut rows: Vec<(Uuid, String, String, bool, bool, String)> = Vec::new();
        for g in &self.library.groups {
            if let GroupFilter::Group(id) = self.filter {
                if g.id != id {
                    continue;
                }
            }
            for s in &g.snippets {
                if s.matches_query(&self.search) {
                    rows.push((
                        s.id,
                        s.abbreviation.clone(),
                        s.display_name(),
                        s.enabled && g.enabled,
                        lint_ids.contains(&s.id),
                        g.name.clone(),
                    ));
                }
            }
        }
        ui.label(
            RichText::new(format!("{} shown", rows.len()))
                .weak()
                .small(),
        );
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("snippet_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.label(RichText::new("No snippets match.").weak());
                }
                for (id, abbr, name, active, has_lint, group) in rows {
                    let selected = self.selected == Some(id);
                    let abbr_text = if abbr.is_empty() {
                        "—".to_owned()
                    } else {
                        abbr
                    };
                    let mut job = egui::text::LayoutJob::default();
                    let mono = egui::FontId::monospace(13.0);
                    let prop = egui::FontId::proportional(13.0);
                    let fg = if active {
                        ui.visuals().text_color()
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    job.append(
                        &format!("{abbr_text:<8}"),
                        0.0,
                        TextFormat::simple(mono, if active { ACCENT } else { fg }),
                    );
                    job.append(
                        &format!("  {name}"),
                        0.0,
                        TextFormat::simple(prop.clone(), fg),
                    );
                    if !active {
                        job.append("  (off)", 0.0, TextFormat::simple(prop.clone(), fg));
                    }
                    if has_lint {
                        job.append("  ⚠", 0.0, TextFormat::simple(prop.clone(), WARN));
                    }
                    if self.filter == GroupFilter::All {
                        job.append(
                            &format!("  · {group}"),
                            0.0,
                            TextFormat::simple(
                                egui::FontId::proportional(11.0),
                                ui.visuals().weak_text_color(),
                            ),
                        );
                    }
                    let resp = ui.add(
                        egui::Button::selectable(selected, job)
                            .wrap_mode(egui::TextWrapMode::Truncate),
                    );
                    if resp.clicked() {
                        self.selected = Some(id);
                        if self.tab != Tab::Editor && self.tab != Tab::Playground {
                            self.tab = Tab::Editor;
                        }
                    }
                }
            });
    }

    fn editor(&mut self, ui: &mut egui::Ui) {
        let Some((gi, si)) = self.selected.and_then(|id| self.library.find_snippet(id)) else {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No snippet selected").size(18.0));
                ui.label("Choose a snippet from the list, or create one with ＋ Snippet.");
            });
            return;
        };
        let id = self.library.groups[gi].snippets[si].id;
        if self.tags_for != Some(id) {
            self.tags_buffer = self.library.groups[gi].snippets[si].tags.join(", ");
            self.tags_for = Some(id);
        }
        let group_names: Vec<(Uuid, String)> = self
            .library
            .groups
            .iter()
            .map(|g| (g.id, g.name.clone()))
            .collect();
        let group_enabled = self.library.groups[gi].enabled;
        let default_trigger = self.library.settings.default_trigger;
        let mut changed = false;
        let mut move_to: Option<usize> = None;
        let mut insert_token: Option<&'static str> = None;

        egui::ScrollArea::vertical()
            .id_salt("editor_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let s = &mut self.library.groups[gi].snippets[si];
                egui::Grid::new("snippet_fields")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Title");
                        changed |= ui
                            .add(
                                TextEdit::singleline(&mut s.title)
                                    .char_limit(store::MAX_NAME_CHARS)
                                    .hint_text("Optional label")
                                    .desired_width(360.0),
                            )
                            .changed();
                        ui.end_row();

                        ui.label("Abbreviation");
                        ui.horizontal(|ui| {
                            changed |= ui
                                .add(
                                    TextEdit::singleline(&mut s.abbreviation)
                                        .char_limit(engine::MAX_ABBREVIATION_CHARS)
                                        .font(egui::TextStyle::Monospace)
                                        .hint_text("e.g. sig")
                                        .desired_width(160.0),
                                )
                                .changed();
                            changed |= ui.checkbox(&mut s.case_sensitive, "Case-sensitive").changed();
                            changed |= ui.checkbox(&mut s.enabled, "Enabled").changed();
                        });
                        ui.end_row();

                        ui.label("Trigger");
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("trigger_combo")
                                .selected_text(s.trigger.label())
                                .show_ui(ui, |ui| {
                                    for t in [
                                        SnippetTrigger::UseDefault,
                                        SnippetTrigger::Delimiter,
                                        SnippetTrigger::Immediate,
                                    ] {
                                        changed |= ui.selectable_value(&mut s.trigger, t, t.label()).changed();
                                    }
                                });
                            ui.label(
                                RichText::new(format!(
                                    "→ effective: {}",
                                    s.trigger.resolve(default_trigger).label()
                                ))
                                .weak(),
                            );
                        });
                        ui.end_row();

                        ui.label("Tags");
                        let resp = ui.add(
                            TextEdit::singleline(&mut self.tags_buffer)
                                .hint_text("comma, separated")
                                .desired_width(360.0),
                        );
                        if resp.changed() {
                            s.tags = self
                                .tags_buffer
                                .split(',')
                                .map(|t| t.trim().to_owned())
                                .filter(|t| !t.is_empty())
                                .take(store::MAX_TAGS)
                                .map(|t| t.chars().take(store::MAX_TAG_CHARS).collect())
                                .collect();
                            changed = true;
                        }
                        ui.end_row();

                        ui.label("Group");
                        egui::ComboBox::from_id_salt("group_combo")
                            .selected_text(group_names[gi].1.clone())
                            .show_ui(ui, |ui| {
                                for (i, (_, name)) in group_names.iter().enumerate() {
                                    if ui.selectable_label(i == gi, name).clicked() && i != gi {
                                        move_to = Some(i);
                                    }
                                }
                            });
                        ui.end_row();
                    });

                if !group_enabled {
                    ui.label(
                        RichText::new("This snippet's group is disabled, so it will not expand.")
                            .color(WARN),
                    );
                }
                for l in self.lints.iter().filter(|l| l.snippet_id == id) {
                    ui.label(RichText::new(format!("⚠ {}", l.message)).color(WARN));
                }

                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Body").strong());
                    ui.label(RichText::new("Insert:").weak());
                    for (label, token) in [
                        ("Date", "{{date}}"),
                        ("Time", "{{time}}"),
                        ("Date+time", "{{datetime}}"),
                        ("Custom date", "{{date:%d %b %Y}}"),
                        ("Clipboard", "{{clipboard}}"),
                        ("Cursor", "{{cursor}}"),
                    ] {
                        if ui.small_button(label).on_hover_text(token).clicked() {
                            insert_token = Some(token);
                        }
                    }
                });
                let s = &mut self.library.groups[gi].snippets[si];
                let body_id = egui::Id::new("body_editor");
                let out = TextEdit::multiline(&mut s.body)
                    .id(body_id)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(8)
                    .desired_width(f32::INFINITY)
                    .hint_text("Expansion text. Plain text only in v0.1; see Syntax & rules for placeholders.")
                    .show(ui);
                if out.response.response.changed() {
                    changed = true;
                }
                if let Some(token) = insert_token.take() {
                    let at = out
                        .cursor_range
                        .map(|r| r.primary.index.0)
                        .unwrap_or_else(|| s.body.chars().count());
                    let byte = s
                        .body
                        .char_indices()
                        .nth(at)
                        .map(|(b, _)| b)
                        .unwrap_or(s.body.len());
                    s.body.insert_str(byte, token);
                    let mut state = out.state.clone();
                    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(
                        at + token.chars().count(),
                    ))));
                    state.store(ui.ctx(), body_id);
                    changed = true;
                }

                ui.add_space(8.0);
                ui.label(RichText::new("Preview (rendered now)").strong());
                let body = self.library.groups[gi].snippets[si].body.clone();
                let ctx = self.render_ctx(&body);
                let rendered = template::render(&body, &ctx);
                preview_box(ui, &rendered);
                if !rendered.warnings.is_empty() {
                    for w in &rendered.warnings {
                        ui.label(RichText::new(format!("⚠ {w}")).color(WARN));
                    }
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Try it for real in the ⌨ Playground tab — it uses the same engine as system-wide expansion.",
                    )
                    .weak(),
                );
            });

        if let Some(to) = move_to {
            let s = self.library.groups[gi].snippets.remove(si);
            self.status = format!("Moved to '{}'.", self.library.groups[to].name);
            self.library.groups[to].snippets.push(s);
            changed = true;
        }
        if changed {
            self.mark_changed();
        }
    }

    fn playground_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Typing playground");
        ui.label(
            "Type below. Abbreviations expand inside this box using the real engine and your current \
             library — nothing leaves this window. Clicking or using arrow keys resets matching, like \
             system-wide mode.",
        );
        ui.horizontal(|ui| {
            let s = self.playground.engine.settings();
            ui.label(
                RichText::new(format!(
                    "Default trigger: {} · word boundary: {} · keep delimiter: {}",
                    s.default_trigger.label(),
                    if s.require_word_boundary { "on" } else { "off" },
                    if s.keep_delimiter { "on" } else { "off" }
                ))
                .weak(),
            );
            if ui.small_button("Change…").clicked() {
                self.tab = Tab::Settings;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Try:");
            for hint in ["hlo␠", "sig⏎", ";d", "xc␠", "cmơn␠", "API␠", "->>", "mtg␠"] {
                ui.label(RichText::new(hint).monospace().color(ACCENT));
            }
        });
        let id = egui::Id::new("playground_text");
        let out = TextEdit::multiline(&mut self.playground.text)
            .id(id)
            .desired_rows(10)
            .desired_width(f32::INFINITY)
            .lock_focus(true)
            .hint_text("Type here…")
            .show(ui);
        let cursor = out
            .cursor_range
            .map(|r| r.primary.index.0)
            .unwrap_or(self.playground.text.chars().count());

        if let Some(new_cursor) = self.playground.pending_cursor.take() {
            // Cursor set programmatically last frame; just record the state.
            self.playground.prev_cursor = new_cursor;
            self.playground.prev_text = self.playground.text.clone();
        } else if self.playground.text != self.playground.prev_text {
            self.playground_handle_edit(cursor, out.state.clone(), ui.ctx(), id);
        } else if cursor != self.playground.prev_cursor {
            self.playground.engine.feed(KeyInput::Reset);
            self.playground.prev_cursor = cursor;
        }

        ui.horizontal(|ui| {
            if ui.button("Clear").clicked() {
                self.playground.text.clear();
                self.playground.prev_text.clear();
                self.playground.prev_cursor = 0;
                self.playground.engine.reset();
                self.playground.log.clear();
            }
            ui.checkbox(
                &mut self.playground.show_buffer,
                "Show engine buffer (debug)",
            );
        });
        if self.playground.show_buffer {
            ui.label(
                RichText::new(format!("buffer: {:?}", self.playground.engine.buffer()))
                    .monospace()
                    .weak(),
            );
        }
        ui.separator();
        ui.label(RichText::new("Expansion log (this session only, not saved)").strong());
        for line in self.playground.log.iter().rev().take(8) {
            ui.label(RichText::new(line).monospace());
        }
    }

    /// Translate a text-box edit into engine key events (diff between previous and new text).
    fn playground_handle_edit(
        &mut self,
        cursor: usize,
        mut state: egui::text_edit::TextEditState,
        ctx: &egui::Context,
        id: egui::Id,
    ) {
        let pg = &mut self.playground;
        let old: Vec<char> = pg.prev_text.chars().collect();
        let new: Vec<char> = pg.text.chars().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let max_suffix = old.len().min(new.len()) - prefix;
        let suffix = old
            .iter()
            .rev()
            .zip(new.iter().rev())
            .take(max_suffix)
            .take_while(|(a, b)| a == b)
            .count();
        let removed = old.len() - prefix - suffix;
        let inserted: Vec<char> = new[prefix..new.len() - suffix].to_vec();

        let plausible_typing = pg.prev_cursor == prefix + removed
            && cursor == prefix + inserted.len()
            && removed <= 4
            && inserted.len() <= 4;

        let mut expansion = None;
        if plausible_typing {
            for _ in 0..removed {
                pg.engine.feed(KeyInput::Backspace);
            }
            for (i, c) in inserted.iter().enumerate() {
                if let Some(exp) = pg.engine.feed(KeyInput::Char(*c)) {
                    expansion = Some((exp, prefix + i + 1));
                    break;
                }
            }
        } else {
            pg.engine.feed(KeyInput::Reset);
        }

        let mut new_cursor = cursor;
        if let Some((exp, caret)) = expansion {
            let rctx = self.render_ctx(&exp.body);
            let pg = &mut self.playground;
            let rep = exp.render(&rctx);
            let mut chars: Vec<char> = pg.text.chars().collect();
            let start = caret.saturating_sub(rep.delete_chars);
            let typed: String = chars[start..caret].iter().collect();
            if typed == rep.delete_text {
                let ins: Vec<char> = rep.insert.chars().collect();
                chars.splice(start..caret, ins.iter().copied());
                pg.text = chars.into_iter().collect();
                new_cursor = start + ins.len() - rep.after_cursor.chars().count();
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(new_cursor))));
                state.store(ctx, id);
                pg.pending_cursor = Some(new_cursor);
                let mut line = format!(
                    "{}  {:?} → {}",
                    chrono::Local::now().format("%H:%M:%S"),
                    exp.typed,
                    exp.title
                );
                if !rep.warnings.is_empty() {
                    line.push_str(&format!("  ⚠ {}", rep.warnings.join("; ")));
                }
                pg.log.push(line);
            } else {
                pg.log.push(format!(
                    "skipped: text before caret {typed:?} did not match {:?}",
                    rep.delete_text
                ));
            }
        }
        let pg = &mut self.playground;
        pg.prev_text = pg.text.clone();
        pg.prev_cursor = new_cursor;
    }

    fn system_wide_ui(&mut self, ui: &mut egui::Ui) {
        self.refresh_permissions(false);
        ui.heading("System-wide expansion (experimental, opt-in)");
        ui.label(
            "When started, Typelune watches key presses in other apps through a macOS event tap \
             and replaces abbreviations by sending Backspace + text. It is OFF at every launch.",
        );
        ui.add_space(6.0);
        let p = self.perms;
        egui::Grid::new("perm_grid")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                let yes_no = |ok: bool, yes: &str, no: &str| {
                    if ok {
                        RichText::new(format!("✔ {yes}")).color(OK)
                    } else {
                        RichText::new(format!("✖ {no}")).color(ERROR)
                    }
                };
                ui.label("Platform");
                ui.label(yes_no(p.supported, "macOS", "Not supported"));
                ui.end_row();
                ui.label("Accessibility (required)");
                ui.label(yes_no(p.accessibility, "Granted", "Not granted"));
                ui.end_row();
                ui.label("Event posting preflight");
                ui.label(yes_no(p.post_events, "Allowed", "Not allowed"));
                ui.end_row();
                ui.label("Input Monitoring preflight");
                ui.label(if p.listen_events {
                    RichText::new("✔ Allowed").color(OK)
                } else {
                    RichText::new("– Not granted (informational)").weak()
                });
                ui.end_row();
                ui.label("Secure keyboard entry");
                ui.label(if p.secure_input {
                    RichText::new("Active now — typing is not observed").color(WARN)
                } else {
                    RichText::new("Inactive").weak()
                });
                ui.end_row();
                ui.label("Process needing permission");
                ui.label(
                    RichText::new(
                        std::env::current_exe()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default(),
                    )
                    .monospace()
                    .small(),
                );
                ui.end_row();
            });
        if ui.small_button("Re-check permissions").clicked() {
            self.refresh_permissions(true);
        }

        ui.add_space(8.0);
        let state = self.native.state();
        let (txt, color) = native_state_label(&state);
        ui.label(
            RichText::new(format!("Status: {txt}"))
                .strong()
                .color(color),
        );
        if let NativeState::Error(e) = &state {
            ui.label(RichText::new(e).color(ERROR));
        }
        ui.horizontal(|ui| {
            let running = matches!(
                state,
                NativeState::Running | NativeState::Paused | NativeState::Disabled
            );
            if !running {
                let can_start = p.supported && p.accessibility;
                let resp = ui.add_enabled(
                    can_start,
                    egui::Button::new("▶ Start system-wide expansion"),
                );
                let resp = if can_start {
                    resp
                } else {
                    resp.on_disabled_hover_text(
                        "Requires Accessibility permission (granted manually by you).",
                    )
                };
                if resp.clicked() {
                    match self.native.start(&self.library) {
                        Ok(()) => {
                            self.native_message = Some(
                                "Started. Type an abbreviation in any app (e.g. TextEdit).".into(),
                            )
                        }
                        Err(e) => self.native_message = Some(format!("Could not start: {e}")),
                    }
                }
            } else {
                if state == NativeState::Paused {
                    if ui.button("▶ Resume").clicked() {
                        self.native.set_paused(false);
                    }
                } else if ui.button("⏸ Pause").clicked() {
                    self.native.set_paused(true);
                }
                if ui.button("⏹ Stop").clicked() {
                    self.native.stop();
                    self.native_message = Some("Stopped. The event tap was removed.".into());
                }
            }
            if !p.accessibility
                && p.supported
                && ui.button("Open Accessibility settings…").clicked()
            {
                if let Err(e) = native::open_accessibility_settings() {
                    self.native_message = Some(format!("Could not open System Settings: {e}"));
                }
            }
        });
        if let Some(m) = &self.native_message {
            ui.label(RichText::new(m).weak());
        }

        let stats = self.native.stats();
        ui.add_space(6.0);
        egui::Grid::new("native_stats")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Expansions this session");
                ui.label(stats.expansions.to_string());
                ui.end_row();
                ui.label("Last snippet");
                ui.label(match (&stats.last_snippet, stats.last_at) {
                    (Some(t), Some(at)) => format!("{t} at {}", at.format("%H:%M:%S")),
                    _ => "—".into(),
                });
                ui.end_row();
                ui.label("Keys skipped (secure input)");
                ui.label(stats.secure_input_skips.to_string());
                ui.end_row();
                ui.label("Tap re-enabled after system timeout");
                ui.label(stats.tap_reenabled.to_string());
                ui.end_row();
                if let Some(w) = &stats.last_warning {
                    ui.label("Last render warning");
                    ui.label(RichText::new(w).color(WARN));
                    ui.end_row();
                }
                ui.label("Aborted (not injected / stopped early)");
                ui.label(stats.aborted.to_string());
                ui.end_row();
                ui.label("Last abort reason");
                ui.label(match &stats.last_abort {
                    Some(r) => RichText::new(r).color(WARN),
                    None => RichText::new("—"),
                });
                ui.end_row();
            });

        ui.add_space(6.0);
        egui::CollapsingHeader::new("Diagnostics (session-only counts, no typed text)")
            .default_open(true)
            .show(ui, |ui| {
                let d = &stats.diag;
                egui::Grid::new("native_diag")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (label, value) in [
                            ("Key-down events seen by tap", d.key_events_seen),
                            ("  usable text inputs", d.text_inputs),
                            ("  empty-Unicode key events", d.empty_text_events),
                            ("  recovered via keyboard layout", d.layout_translated),
                            ("  resets (shortcut / navigation / control)", d.reset_inputs),
                            ("  backspaces", d.backspaces),
                            ("  own synthetic events skipped", d.own_event_skips),
                            ("  typed into this app (not matched globally)", d.own_app_skips),
                            ("  skipped while paused", d.paused_skips),
                            ("  typing that cancelled a pending expansion", d.typed_during_injection),
                            ("  events with a target PID", d.target_pid_present),
                            ("Mouse clicks (reset)", d.mouse_resets),
                            ("  clicks that cancelled a pending expansion", d.mouse_during_injection),
                            ("Matches queued for injection", d.matches_queued),
                            ("Successful expansions", stats.expansions),
                            ("Aborted", stats.aborted),
                        ] {
                            ui.label(label);
                            ui.label(RichText::new(value.to_string()).monospace());
                            ui.end_row();
                        }
                    });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Last cancelling event:");
                    ui.label(match &d.last_cancel_event {
                        Some(n) => RichText::new(n).monospace().color(WARN),
                        None => RichText::new("—"),
                    });
                });
                ui.label(
                    RichText::new(
                        "Reading these: key events seen stays 0 while typing → input is not passing the session event tap \
                         (e.g. delivered straight to an app, or keyboard monitoring not permitted). \
                         Text inputs rise but matches stay 0 → abbreviation/trigger/boundary did not match. \
                         Matches rise but expansions do not → see the abort reason. Counters reset on every Start and are never saved.",
                    )
                    .weak()
                    .small(),
                );
            });

        ui.add_space(10.0);
        ui.collapsing("How permission works / privacy", |ui| {
            ui.label("• Typelune never changes permissions itself. Grant Accessibility manually in System Settings → Privacy & Security → Accessibility, adding the app you run (or the terminal you launched it from).");
            ui.label("• Permission is tied to the app's code signature. The POC is ad-hoc signed, so after rebuilding you may need to remove and re-add it.");
            ui.label("• Typed characters are kept only in a 64-character in-memory buffer for matching. Nothing typed is logged, saved, or sent anywhere.");
            ui.label("• Password fields (secure keyboard entry) are skipped. ⌘/⌃ shortcuts, arrow keys, mouse clicks and Escape reset matching.");
            ui.label("• Our own synthetic key events are tagged and ignored, so expansions never re-trigger.");
            ui.label("• Known limits: input methods that compose text (e.g. Vietnamese Telex/VNI IMEs, dead keys) are seen as raw keys, so composed abbreviations may not match system-wide. The Playground handles composed text.");
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Expansion settings");
        let mut changed = false;
        let s = &mut self.library.settings;
        egui::Grid::new("settings_grid").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            ui.label("Default trigger");
            ui.horizontal(|ui| {
                changed |= ui
                    .radio_value(&mut s.default_trigger, TriggerMode::Delimiter, TriggerMode::Delimiter.label())
                    .changed();
                changed |= ui
                    .radio_value(&mut s.default_trigger, TriggerMode::Immediate, TriggerMode::Immediate.label())
                    .changed();
            });
            ui.end_row();

            ui.label("Delimiters");
            ui.vertical(|ui| {
                let mut space = s.delimiters.contains(' ');
                let mut tab = s.delimiters.contains('\t');
                let mut ret = s.delimiters.contains('\n');
                let mut punct: String = s
                    .delimiters
                    .chars()
                    .filter(|c| !matches!(c, ' ' | '\t' | '\n'))
                    .collect();
                ui.horizontal(|ui| {
                    let a = ui.checkbox(&mut space, "Space").changed();
                    let b = ui.checkbox(&mut tab, "Tab").changed();
                    let c = ui.checkbox(&mut ret, "Return").changed();
                    ui.label("Other:");
                    let d = ui
                        .add(TextEdit::singleline(&mut punct).font(egui::TextStyle::Monospace).desired_width(160.0))
                        .changed();
                    if a || b || c || d {
                        let mut out = String::new();
                        if space {
                            out.push(' ');
                        }
                        if tab {
                            out.push('\t');
                        }
                        if ret {
                            out.push('\n');
                        }
                        for ch in punct.chars() {
                            if !ch.is_alphanumeric() && !ch.is_whitespace() && !out.contains(ch) {
                                out.push(ch);
                            }
                        }
                        if !out.is_empty() {
                            s.delimiters = out;
                            changed = true;
                        }
                    }
                });
                ui.label(RichText::new("Letters and digits are never delimiters.").weak().small());
            });
            ui.end_row();

            ui.label("Keep delimiter");
            changed |= ui
                .checkbox(&mut s.keep_delimiter, "Re-insert the delimiter after the expansion")
                .changed();
            ui.end_row();

            ui.label("Word boundary");
            changed |= ui
                .checkbox(
                    &mut s.require_word_boundary,
                    "Only expand when the abbreviation starts a word (abbreviations starting with punctuation are exempt)",
                )
                .changed();
            ui.end_row();
        });
        if changed {
            self.mark_changed();
        }

        ui.add_space(12.0);
        ui.heading("Data");
        ui.label(format!(
            "Library file: {}",
            self.store.library_path().display()
        ));
        ui.label(RichText::new("Previous version kept as library.json.bak. The legacy UI always uses the folder in TYPELUNE_DATA_DIR.").weak());
        ui.horizontal(|ui| {
            if ui.button("Reveal in Finder").clicked() {
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg("-R")
                    .arg(self.store.library_path())
                    .spawn();
            }
            if ui.button("Save now").clicked() {
                self.dirty_since.get_or_insert_with(Instant::now);
                self.save_if_due(true);
                if self.save_error.is_none() {
                    self.status = "Saved.".into();
                }
            }
        });
    }

    fn help_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().id_salt("help_scroll").show(ui, |ui| {
            ui.heading("Placeholder syntax (v0.1)");
            egui::Grid::new("syntax").num_columns(2).striped(true).spacing([20.0, 6.0]).show(ui, |ui| {
                for (tok, desc) in [
                    ("{{date}}", "Today, YYYY-MM-DD"),
                    ("{{time}}", "Current time, HH:MM (24h)"),
                    ("{{datetime}}", "YYYY-MM-DD HH:MM"),
                    ("{{date:FORMAT}}", "strftime format, e.g. {{date:%A, %d %B %Y}}"),
                    ("{{time:FORMAT}}", "Same as date:FORMAT"),
                    ("{{clipboard}}", "Current plain-text clipboard (kept visibly if unavailable)"),
                    ("{{cursor}}", "Caret position after expansion (first occurrence only)"),
                    ("\\{{", "A literal {{"),
                ] {
                    ui.label(RichText::new(tok).monospace().color(ACCENT));
                    ui.label(desc);
                    ui.end_row();
                }
            });
            ui.label(RichText::new("Unknown or invalid placeholders are left in the text exactly as written and shown as warnings — they are never silently removed.").weak());

            ui.add_space(10.0);
            ui.heading("Matching rules");
            for line in [
                "• Delimiter trigger: abbreviation expands after you type a delimiter (default: space, tab, return and common punctuation). The delimiter is kept by default.",
                "• Immediate trigger: abbreviation expands as soon as its last character is typed. Shorter immediate abbreviations shadow longer ones that start the same way (shown as a ⚠).",
                "• Word boundary: letters/digits before the abbreviation prevent expansion (\"ohlo\" does not expand \"hlo\"). Abbreviations that start with punctuation (\";d\") are exempt.",
                "• Longest match wins when several abbreviations end at the caret; ties go to case-sensitive snippets, then to the earlier snippet in the library.",
                "• Case-insensitive by default: \"BRB\" matches \"brb\". The body is inserted unchanged.",
                "• Disabled snippets and all snippets in disabled groups never expand.",
                "• Unicode: matching uses NFC, so Vietnamese and other accented text works whether typed composed or decomposed. Backspace and caret moves count user-perceived characters.",
                "• Backspace edits the match buffer; clicks, arrows, Escape and ⌘/⌃ shortcuts reset it. Expansions never chain into other expansions.",
                "• Plain text only. Rich text, pictures, scripts, fill-in fields, nested snippets and app-specific rules are not part of v0.1.",
            ] {
                ui.label(line);
            }
        });
    }

    fn modals(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.confirm_delete_group {
            let info = self.library.group_index(id).map(|gi| {
                (
                    self.library.groups[gi].name.clone(),
                    self.library.groups[gi].snippets.len(),
                )
            });
            let Some((name, count)) = info else {
                self.confirm_delete_group = None;
                return;
            };
            let mut close = false;
            let mut confirm = false;
            egui::Modal::new(egui::Id::new("confirm_delete_group")).show(ctx, |ui| {
                ui.heading("Delete group?");
                ui.label(format!("“{name}” and its {count} snippet(s) will be removed. You can undo this until you quit."));
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Delete group").color(ERROR)).clicked() {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if confirm {
                self.delete_group(id);
                close = true;
            }
            if close {
                self.confirm_delete_group = None;
            }
        }

        if self.pending_import.is_some() {
            let (path, groups, snippets) = {
                let p = self.pending_import.as_ref().unwrap();
                (
                    p.path.clone(),
                    p.library.groups.len(),
                    p.library.snippet_count(),
                )
            };
            let mut action = 0;
            egui::Modal::new(egui::Id::new("import_modal")).show(ctx, |ui| {
                ui.heading("Import library");
                ui.label(format!("{path}\nValid Typelune library: {groups} group(s), {snippets} snippet(s)."));
                ui.label(RichText::new("Merge adds the groups next to yours (new IDs). Replace swaps the whole library, keeping an undo snapshot.").weak());
                ui.horizontal(|ui| {
                    if ui.button("Merge").clicked() {
                        action = 1;
                    }
                    if ui.button("Replace library").clicked() {
                        action = 2;
                    }
                    if ui.button("Cancel").clicked() {
                        action = 3;
                    }
                });
            });
            if action != 0 {
                let pending = self.pending_import.take().unwrap();
                match action {
                    1 => {
                        self.push_undo("import (merge)");
                        let sum = store::merge_into(&mut self.library, pending.library);
                        self.status = format!(
                            "Imported {} group(s), {} snippet(s).",
                            sum.groups, sum.snippets
                        );
                        self.mark_changed();
                    }
                    2 => {
                        self.push_undo("import (replace)");
                        self.library = pending.library;
                        self.filter = GroupFilter::All;
                        self.selected = None;
                        self.tags_for = None;
                        self.status = format!(
                            "Library replaced with {snippets} snippet(s). Undo is available."
                        );
                        self.mark_changed();
                    }
                    _ => self.status = "Import cancelled.".into(),
                }
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // ⌘Z belongs to text fields while one has focus; otherwise it undoes library changes.
        let text_focused = ctx.egui_wants_keyboard_input();
        let (new, undo) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::N),
                !text_focused && i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z),
            )
        });
        if new {
            self.new_snippet();
        }
        if undo {
            self.do_undo();
        }
    }
}

impl eframe::App for TypeluneApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);

        egui::Panel::top("toolbar")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(8.0))
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(6.0))
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("groups")
            .default_size(210.0)
            .min_size(160.0)
            .show(ui, |ui| self.groups_panel(ui));
        egui::Panel::left("snippets")
            .default_size(300.0)
            .min_size(220.0)
            .show(ui, |ui| self.snippet_list(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| match self.tab {
            Tab::Editor => self.editor(ui),
            Tab::Playground => self.playground_ui(ui),
            Tab::SystemWide => self.system_wide_ui(ui),
            Tab::Settings => self.settings_ui(ui),
            Tab::Help => self.help_ui(ui),
        });
        self.modals(&ctx);

        self.save_if_due(false);
        if self.dirty_since.is_some() {
            ctx.request_repaint_after(Duration::from_millis(300));
        }
        let native_active = matches!(
            self.native.state(),
            NativeState::Running | NativeState::Paused | NativeState::Disabled
        );
        if native_active
            && self
                .layout_refreshed
                .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
        {
            // Main thread: keeps the keyboard-layout fallback current if the user switches layouts.
            self.native.refresh_keyboard_layout();
            self.layout_refreshed = Some(Instant::now());
        }
        if self.native.state() != NativeState::Off || self.tab == Tab::SystemWide {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.native.stop();
        self.save_if_due(true);
    }
}

fn native_state_label(state: &NativeState) -> (&'static str, Color32) {
    match state {
        NativeState::Off => ("Off", Color32::GRAY),
        NativeState::Running => ("Running", OK),
        NativeState::Paused => ("Paused", WARN),
        NativeState::Disabled => ("Disabled by macOS (retrying)", WARN),
        NativeState::Stopping => ("Stopping…", WARN),
        NativeState::Error(_) => ("Error", ERROR),
    }
}

fn preview_box(ui: &mut egui::Ui, rendered: &template::Rendered) {
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::monospace(13.0);
    let normal = TextFormat::simple(font.clone(), ui.visuals().text_color());
    let caret = TextFormat {
        background: ACCENT,
        ..TextFormat::simple(font.clone(), Color32::WHITE)
    };
    match rendered.cursor {
        Some(c) => {
            let before: String = rendered.text.chars().take(c).collect();
            let after: String = rendered.text.chars().skip(c).collect();
            job.append(&before, 0.0, normal.clone());
            job.append("▏", 0.0, caret);
            job.append(&after, 0.0, normal);
        }
        None => job.append(&rendered.text, 0.0, normal),
    }
    if rendered.text.is_empty() && rendered.cursor.is_none() {
        job = egui::text::LayoutJob::simple(
            "(empty)".into(),
            font,
            ui.visuals().weak_text_color(),
            f32::INFINITY,
        );
    }
    egui::Frame::group(ui.style())
        .fill(ui.visuals().extreme_bg_color)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(job);
        });
    if rendered.cursor.is_some() {
        ui.label(
            RichText::new("▏ marks where the caret lands after expansion.")
                .weak()
                .small(),
        );
    }
}

/// Add a broad-coverage macOS system font as fallback so Vietnamese/CJK render correctly.
fn install_fonts(ctx: &egui::Context) {
    let candidates = [
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/Library/Fonts/Arial Unicode.ttf",
    ];
    let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "unicode_fallback".into(),
        egui::FontData::from_owned(bytes).into(),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("unicode_fallback".into());
    }
    ctx.set_fonts(fonts);
}
