// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

pub mod completion;
pub mod delimiter;
pub mod formatter;
pub mod harper;
pub mod hover;
pub mod search_toolbar;
pub mod view;

use adw::{prelude::*, subclass::prelude::*};
use gtk::CompositeTemplate;
use sourceview::prelude::*;
use std::{
    cell::{Cell, OnceCell, RefCell},
    rc::Rc,
};

use crate::editor::harper::{
    DirtyRanges, HarperLint, LintSuggestion, apply_lint_results, apply_suggestion, clear_all_lints,
    dialect_from_bcp47, harper_should_be_active, paragraph_bounds,
};
use crate::utils;
use view::TypesetterEditorView;

// ─────────────────────────────────────────────────────────────────────────────
// § 1  GTK subclass
// ─────────────────────────────────────────────────────────────────────────────

mod imp {
    use super::*;

    #[derive(CompositeTemplate, Debug)]
    #[template(resource = "/dev/whetstone/editor/gtk/editor.ui")]
    pub struct TypesetterEditor {
        // ── Buffer ────────────────────────────────────────────────────────
        pub(crate) buffer: OnceCell<sourceview::Buffer>,
        pub(crate) debounce_timeout_id: Rc<RefCell<Option<gtk::glib::SourceId>>>,

        // ── Template children ─────────────────────────────────────────────
        #[template_child]
        pub(super) scroller: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub(crate) search_toolbar: TemplateChild<search_toolbar::SearchToolbar>,
        #[template_child]
        pub(crate) view: TemplateChild<TypesetterEditorView>,

        // ── Signal handler tracking for cleanup ───────────────────────────
        pub(crate) signal_handlers: RefCell<Vec<(gtk::glib::Object, gtk::glib::SignalHandlerId)>>,

        // ── Harper state ──────────────────────────────────────────────────
        pub(crate) dirty_ranges: Rc<RefCell<DirtyRanges>>,
        pub(crate) harper_worker: RefCell<Option<crate::editor::harper::HarperWorker>>,
        pub(crate) harper_lints: Rc<RefCell<Vec<HarperLint>>>,
        /// Tracks whether Harper is currently enabled so that redundant
        /// enable/disable transitions are no-ops.
        pub(crate) harper_enabled: Cell<bool>,
        /// The persistent action group registered on the view for Harper
        /// context menu actions. Stored here so `sync_grammar_toggle_state`
        /// can reach the `toggle-grammar` action without going through the
        /// widget action group API.
        pub(crate) harper_action_group: RefCell<Option<gtk::gio::SimpleActionGroup>>,
        /// Single permanent wrapper inserted once into the extra menu.
        /// Its contents are rebuilt by `rebuild_harper_menu` whenever
        /// `harper_enabled` changes or the spellchecker language changes.
        /// Never removed from the outer menu — only its contents change.
        pub(crate) harper_menu_wrapper: RefCell<Option<gtk::gio::Menu>>,
        /// The menu section holding lint fix suggestions, rebuilt on each
        /// right-click / Menu key press. Appended into `harper_menu_wrapper`
        /// only when Harper is fully active.
        pub(crate) harper_lint_section: RefCell<Option<gtk::gio::Menu>>,
        /// The menu section holding the "Check Grammar" toggle item.
        /// Populated once at `realize` time and appended into
        /// `harper_menu_wrapper` whenever the language is English.
        pub(crate) harper_toggle_section: RefCell<Option<gtk::gio::Menu>>,
        /// The gesture controller that populates lint suggestions on right-click.
        /// Stored for explicit removal from the view in `dispose`.
        pub(crate) harper_gesture: OnceCell<gtk::GestureClick>,
        /// The key controller that populates lint suggestions on Menu/Shift+F10.
        /// Stored for explicit removal from the view in `dispose`.
        pub(crate) harper_key_controller: OnceCell<gtk::EventControllerKey>,

        pub(crate) diagnostic_lints: Rc<RefCell<Vec<hover::DiagnosticLint>>>,
        pub(crate) diagnostic_hover_provider:
            RefCell<Option<hover::TypesetterDiagnosticHoverProvider>>,

        // ── Objects that need explicit cleanup ────────────────────────────
        pub(crate) spell_adapter: Rc<RefCell<Option<libspelling::TextBufferAdapter>>>,
        pub(crate) completion_provider: RefCell<Option<completion::TypesetterCompletionProvider>>,
        pub(crate) hover_provider: RefCell<Option<hover::TypesetterTypstHoverProvider>>,
        pub(crate) harper_hover_provider:
            RefCell<Option<crate::editor::hover::TypesetterHarperHoverProvider>>,
    }

    impl Default for TypesetterEditor {
        fn default() -> Self {
            Self {
                buffer: OnceCell::new(),
                debounce_timeout_id: Rc::new(RefCell::new(None)),
                scroller: TemplateChild::default(),
                search_toolbar: TemplateChild::default(),
                view: TemplateChild::default(),
                signal_handlers: RefCell::new(Vec::new()),
                dirty_ranges: Rc::new(RefCell::new(DirtyRanges::new())),
                harper_worker: RefCell::new(None),
                harper_lints: Rc::new(RefCell::new(Vec::new())),
                harper_enabled: Cell::new(false),
                harper_action_group: RefCell::new(None),
                harper_menu_wrapper: RefCell::new(None),
                harper_lint_section: RefCell::new(None),
                harper_toggle_section: RefCell::new(None),
                harper_gesture: OnceCell::new(),
                harper_key_controller: OnceCell::new(),
                diagnostic_lints: Rc::new(RefCell::new(Vec::new())),
                diagnostic_hover_provider: RefCell::new(None),
                spell_adapter: Rc::new(RefCell::new(None)),
                completion_provider: RefCell::new(None),
                hover_provider: RefCell::new(None),
                harper_hover_provider: RefCell::new(None),
            }
        }
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for TypesetterEditor {
        const NAME: &'static str = "TypesetterEditor";
        type Type = super::TypesetterEditor;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TypesetterEditor {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            obj.initialize_buffer();
            obj.initialize_text_tags();
            obj.initialize_search_toolbar();
            obj.initialize_spell_checking();
            obj.initialize_harper_signals();
            obj.initialize_harper_context_menu();
            obj.initialize_format_context_menu();
        }

        fn dispose(&self) {
            // Clean up any pending timeout
            if let Some(id) = self.debounce_timeout_id.borrow_mut().take() {
                id.remove();
            }

            // Clean up signal handlers first
            for (object, handler_id) in self.signal_handlers.borrow_mut().drain(..) {
                object.disconnect(handler_id);
            }

            // Cancel and drop Harper worker
            if let Some(worker) = self.harper_worker.borrow_mut().take() {
                worker.cancel();
            }

            // Clear Harper lints and delete their marks before the buffer is gone.
            if let Some(buffer) = self.buffer.get() {
                clear_all_lints(buffer, &mut self.harper_lints.borrow_mut());
            }
            self.dirty_ranges.borrow_mut().clear();

            // Clear diagnostic lints and delete their marks before the buffer is gone.
            if let Some(buffer) = self.buffer.get() {
                let mut lints = self.diagnostic_lints.borrow_mut();
                for lint in lints.drain(..) {
                    lint.delete_marks(buffer);
                }
            }

            // Unregister the diagnostic hover provider from the source view.
            if let Some(provider) = self.diagnostic_hover_provider.borrow_mut().take() {
                let hover = self.view.hover();
                hover.remove_provider(&provider.upcast::<sourceview::HoverProvider>());
            }

            // Clean up spell checking adapter
            if let Some(adapter) = self.spell_adapter.borrow_mut().take() {
                // Disable the adapter to disconnect its internal handlers
                adapter.set_enabled(false);

                // Remove it from the view's action group
                self.view
                    .insert_action_group("spelling", gtk::gio::ActionGroup::NONE);
                self.view.set_extra_menu(gtk::gio::MenuModel::NONE);
            }

            // Unregister the Harper action group. The view holds a strong
            // reference to it via insert_action_group, so without this the
            // action group and its closures would leak past dispose.
            self.view
                .insert_action_group("harper", gtk::gio::ActionGroup::NONE);

            // Explicitly remove the Harper gesture and key controller from the
            // view so their closures (which hold strong Rc references to
            // harper_lints, lint_section, and action_group) are released.
            if let Some(gesture) = self.harper_gesture.get() {
                self.view.remove_controller(gesture);
            }
            if let Some(controller) = self.harper_key_controller.get() {
                self.view.remove_controller(controller);
            }

            // Clean up completion provider
            if let Some(provider) = self.completion_provider.borrow_mut().take() {
                let completion = self.view.completion();
                completion.remove_provider(&provider.upcast::<sourceview::CompletionProvider>());
            }

            // Clean up hover provider
            if let Some(provider) = self.hover_provider.borrow_mut().take() {
                let hover = self.view.hover();
                hover.remove_provider(&provider.upcast::<sourceview::HoverProvider>());
            }

            // Clean up Harper hover provider
            if let Some(provider) = self.harper_hover_provider.borrow_mut().take() {
                let hover = self.view.hover();
                hover.remove_provider(&provider.upcast::<sourceview::HoverProvider>());
            }

            // Unparent all children to break reference cycles
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for TypesetterEditor {
        fn realize(&self) {
            self.parent_realize();

            // Apply spell checking setting
            let enabled = self
                .get_settings_from_window()
                .map(|s| s.boolean("check-spelling"))
                .unwrap_or(true);

            if let Some(adapter) = self.spell_adapter.borrow().as_ref() {
                adapter.set_enabled(enabled);
            }

            // Initial Harper enable check — settings and language are both
            // accessible now that the widget is attached to a window.
            self.obj()
                .set_harper_enabled(self.obj().harper_should_be_enabled());

            // Populate the toggle section and trigger the initial menu build.
            self.obj().setup_harper_grammar_toggle();
        }
    }

    impl BoxImpl for TypesetterEditor {}

    #[gtk::template_callbacks]
    impl TypesetterEditor {
        /// Callback to move the cursor when right-clicking so that spell
        /// checking suggestions are offered for the word under the pointer
        /// rather than the word at the old cursor position.
        #[template_callback]
        fn _on_right_click(gesture: &gtk::GestureClick, _n_press: i32, x: f64, y: f64) {
            let view = match gesture
                .widget()
                .and_then(|w| w.downcast::<TypesetterEditorView>().ok())
            {
                Some(v) => v,
                None => return,
            };

            let buffer = view.buffer();
            let (bx, by) =
                view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);

            if let Some(iter) = view.iter_at_location(bx, by) {
                // Only move cursor if we don't have a selection, or if the
                // click is outside the current selection.
                if buffer.has_selection() {
                    let (start, end) = buffer.selection_bounds().unwrap();
                    let offset = iter.offset();
                    if offset >= start.offset() && offset <= end.offset() {
                        return;
                    }
                }

                buffer.place_cursor(&iter);
            }
        }

        pub(super) fn get_settings_from_window(&self) -> Option<gtk::gio::Settings> {
            let mut widget = Some(self.obj().clone().upcast::<gtk::Widget>());
            while let Some(w) = widget {
                if let Ok(window) = w.clone().downcast::<crate::TypesetterWindow>() {
                    return Some(window.settings());
                }
                widget = w.parent();
            }
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// § 2  GLib wrapper & public accessors
// ─────────────────────────────────────────────────────────────────────────────

gtk::glib::wrapper! {
    pub struct TypesetterEditor(ObjectSubclass<imp::TypesetterEditor>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Buildable, gtk::ConstraintTarget, gtk::Accessible;
}

#[allow(clippy::new_without_default)]
impl TypesetterEditor {
    pub fn new() -> Self {
        gtk::glib::Object::new::<Self>()
    }

    /// Returns the editor's `GtkSourceBuffer`.
    pub fn buffer(&self) -> &sourceview::Buffer {
        self.imp()
            .buffer
            .get()
            .expect("Editor buffer has not been initialized")
    }

    /// Returns the full contents of the editor buffer as a `String`.
    pub fn contents(&self) -> String {
        self.buffer()
            .text(
                &self.buffer().start_iter(),
                &self.buffer().end_iter(),
                false,
            )
            .to_string()
    }

    /// Returns the scrolled window containing the editor view.
    pub fn scroller(&self) -> &gtk::ScrolledWindow {
        self.imp().scroller.as_ref()
    }

    /// Returns the main `GtkSourceView`.
    pub fn view(&self) -> &TypesetterEditorView {
        self.imp().view.as_ref()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// § 3  Buffer & tag initialization
// ─────────────────────────────────────────────────────────────────────────────

impl TypesetterEditor {
    /// Extracts the `GtkSourceBuffer` from the view, stores it in the
    /// `OnceCell`, configures Typst syntax highlighting, and sets up
    /// automatic delimiter handling.
    fn initialize_buffer(&self) {
        let imp = self.imp();
        let buffer = imp
            .view
            .buffer()
            .downcast::<sourceview::Buffer>()
            .expect("Expected a GtkSourceBuffer");
        imp.buffer
            .set(buffer.clone())
            .expect("Editor buffer already initialized");

        // Configure Typst syntax highlighting via the custom language spec
        // bundled as a GResource.
        let language_manager = sourceview::LanguageManager::new();
        let mut search_paths = language_manager.search_path();
        search_paths.push("resource:///dev/whetstone/editor/languages/".into());
        language_manager
            .set_search_path(&search_paths.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        if let Some(language) = language_manager.language("typst") {
            buffer.set_language(Some(&language));
        }

        // Set up automatic delimiter (bracket/quote) handling. The returned
        // signal handler IDs are stored for cleanup in `dispose`.
        let handlers = delimiter::setup_delimiter_handler(self.view());
        imp.signal_handlers.borrow_mut().extend(handlers);
    }

    /// Hooks the search toolbar up to the buffer and view.
    fn initialize_search_toolbar(&self) {
        let imp = self.imp();
        imp.search_toolbar.set_context(self.buffer(), self.view());
    }

    /// Creates the `"error"` and `"warning"` text tags and ensures the `"harper-lint"` tag exists.
    fn initialize_text_tags(&self) {
        if let Some(buffer) = self.imp().buffer.get() {
            let tag_table = buffer.tag_table();

            // Error: red wavy underline + semi-transparent red background
            if tag_table.lookup("error").is_none() {
                let error_color = gtk::gdk::RGBA::parse("#e01b24").unwrap();
                let mut error_bg = error_color;
                error_bg.set_alpha(0.20);
                let tag = gtk::TextTag::new(Some("error"));
                tag.set_property("underline", gtk::pango::Underline::Single);
                tag.set_property("underline-rgba", error_color);
                tag.set_property("background-rgba", error_bg);
                tag_table.add(&tag);
            }

            // Warning: yellow wavy underline + semi-transparent yellow background
            if tag_table.lookup("warning").is_none() {
                let warning_color = gtk::gdk::RGBA::parse("#f5c211").unwrap();
                let mut warning_bg = warning_color;
                warning_bg.set_alpha(0.20);
                let tag = gtk::TextTag::new(Some("warning"));
                tag.set_property("underline", gtk::pango::Underline::Single);
                tag.set_property("underline-rgba", warning_color);
                tag.set_property("background-rgba", warning_bg);
                tag_table.add(&tag);
            }

            crate::editor::harper::ensure_harper_tag(buffer);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// § 4  Spell checking
// ─────────────────────────────────────────────────────────────────────────────

impl TypesetterEditor {
    /// Attaches libspelling to the buffer and view, wires the Harper language
    /// guard to the checker, and persists the enabled/disabled state to
    /// settings whenever it changes.
    fn initialize_spell_checking(&self) {
        let imp = self.imp();

        let checker = libspelling::Checker::default();
        let buffer = self.buffer();
        let adapter = libspelling::TextBufferAdapter::new(buffer, &checker);
        let extra_menu = adapter.menu_model();
        let view = self.view();

        view.set_extra_menu(Some(&extra_menu));
        view.insert_action_group("spelling", Some(&adapter));

        // Wire up Harper's language guard — when the checker's locale
        // changes, Harper is enabled or disabled accordingly.
        self.initialize_harper_language_guard(&checker);

        // Persist the enabled/disabled state to GSettings whenever the
        // adapter's "enabled" property changes (e.g. via the context menu).
        let editor_weak = self.downgrade();
        let handler_id = adapter.connect_notify_local(Some("enabled"), move |adapter, _| {
            if let Some(editor) = editor_weak.upgrade()
                && let Some(settings) = editor.imp().get_settings_from_window()
            {
                let _ = settings.set_boolean("check-spelling", adapter.is_enabled());
            }
        });

        imp.signal_handlers
            .borrow_mut()
            .push((adapter.clone().upcast(), handler_id));

        *imp.spell_adapter.borrow_mut() = Some(adapter);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// § 5  Buffer change debounce
// ─────────────────────────────────────────────────────────────────────────────

impl TypesetterEditor {
    /// Attaches a 350 ms debounced buffer-change handler.
    ///
    /// The same debounce fires two things in sequence:
    /// 1. The `callback` supplied by the caller (Typst preview update).
    /// 2. A Harper lint pass over the paragraphs dirtied since the last fire.
    pub(super) fn on_buffer_changed<F: Fn(String) + 'static>(
        &self,
        buffer: &sourceview::Buffer,
        callback: F,
    ) {
        let imp = self.imp();
        let callback = Rc::new(callback);
        let timeout_id = imp.debounce_timeout_id.clone();
        let dirty_ranges = Rc::clone(&imp.dirty_ranges);
        let harper_lints = Rc::clone(&imp.harper_lints);
        let spell_adapter = Rc::clone(&imp.spell_adapter);

        let handler_id = buffer.connect_changed(gtk::glib::clone! {
            #[weak(rename_to = obj)] self,
            #[strong] timeout_id,
            #[strong] callback,
            #[strong] dirty_ranges,
            #[strong] harper_lints,
            #[strong] spell_adapter,
            move |_| {
                if let Some(id) = timeout_id.borrow_mut().take() {
                    id.remove();
                }

                let id = gtk::glib::timeout_add_local(
                    std::time::Duration::from_millis(350),
                    gtk::glib::clone! {
                        #[weak] obj,
                        #[strong] timeout_id,
                        #[strong] callback,
                        #[strong] dirty_ranges,
                        #[strong] harper_lints,
                        #[strong] spell_adapter,
                        #[upgrade_or] gtk::glib::ControlFlow::Break,
                        move || {
                            let text = obj.contents();
                            callback(text);

                            obj.spawn_harper_lint(
                                &dirty_ranges,
                                &harper_lints,
                                &spell_adapter,
                            );

                            *timeout_id.borrow_mut() = None;
                            gtk::glib::ControlFlow::Break
                        }
                    },
                );

                *timeout_id.borrow_mut() = Some(id);
            }
        });

        imp.signal_handlers
            .borrow_mut()
            .push((buffer.clone().upcast(), handler_id));
    }

    /// Cancel any pending editor debounce compilation/lint timer
    pub fn cancel_debounce(&self) {
        if let Some(id) = self.imp().debounce_timeout_id.borrow_mut().take() {
            id.remove();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// § 6  Harper integration
// ─────────────────────────────────────────────────────────────────────────────

impl TypesetterEditor {
    /// Returns `true` if both conditions for Harper to be active are met:
    /// - The `check-grammar` GSetting is enabled, and
    /// - The current language reported by libspelling is English.
    ///
    /// Returns `false` if settings are not yet accessible (e.g. before
    /// `realize`), so Harper is never accidentally enabled too early.
    pub(crate) fn harper_should_be_enabled(&self) -> bool {
        let grammar_enabled = self
            .imp()
            .get_settings_from_window()
            .map(|s| s.boolean("check-grammar"))
            .unwrap_or(false);

        if !grammar_enabled {
            return false;
        }

        let language_tag = self
            .imp()
            .spell_adapter
            .borrow()
            .as_ref()
            .and_then(|adapter| adapter.checker())
            .and_then(|checker| checker.language())
            .unwrap_or_default();

        harper_should_be_active(&language_tag)
    }

    /// Enable or disable Harper, guarded against redundant transitions.
    ///
    /// If Harper is already in the requested state, this is a no-op.
    pub(crate) fn set_harper_enabled(&self, enabled: bool) {
        let imp = self.imp();
        if imp.harper_enabled.get() == enabled {
            return;
        }
        imp.harper_enabled.set(enabled);
        if enabled {
            self.enable_harper();
        } else {
            self.disable_harper();
        }
    }

    /// Rebuild the contents of `harper_menu_wrapper` to reflect the current
    /// values of `harper_enabled` and the live spellchecker language.
    ///
    /// This is the single point of truth for what appears in the Harper
    /// section of the context menu:
    ///
    /// - Language not English → wrapper is empty (nothing shown)
    /// - Language is English, grammar off → toggle section only
    /// - Language is English, grammar on → lint section + toggle section
    ///
    /// The lint section is always above the toggle so suggestions appear
    /// before the checkbox.
    fn rebuild_harper_menu(&self) {
        let imp = self.imp();
        let Some(wrapper) = imp.harper_menu_wrapper.borrow().as_ref().cloned() else {
            return;
        };

        wrapper.remove_all();

        let language_tag = imp
            .spell_adapter
            .borrow()
            .as_ref()
            .and_then(|adapter| adapter.checker())
            .and_then(|checker| checker.language())
            .unwrap_or_default();

        if !harper_should_be_active(&language_tag) {
            return;
        }

        // Lint suggestions first (only when Harper is fully active).
        if imp.harper_enabled.get()
            && let Some(lint_section) = imp.harper_lint_section.borrow().as_ref().cloned()
        {
            wrapper.append_section(None, &lint_section);
        }

        // Toggle always present when language is English.
        if let Some(toggle_section) = imp.harper_toggle_section.borrow().as_ref().cloned() {
            wrapper.append_section(None, &toggle_section);
        }
    }

    /// Synchronise the `harper.toggle-grammar` action's checked state with
    /// the current value of the `check-grammar` GSettings key.
    ///
    /// Called from `window.rs` whenever the `check-grammar` setting changes
    /// externally (e.g. via `gsettings` CLI or another window), so the
    /// checkmark in the context menu stays accurate.
    pub(crate) fn sync_grammar_toggle_state(&self) {
        let enabled = self
            .imp()
            .get_settings_from_window()
            .map(|s| s.boolean("check-grammar"))
            .unwrap_or(false);

        if let Some(group) = self.imp().harper_action_group.borrow().as_ref()
            && let Some(action) = group
                .lookup_action("toggle-grammar")
                .and_then(|a| a.downcast::<gtk::gio::SimpleAction>().ok())
        {
            action.set_state(&enabled.to_variant());
        }
    }

    /// Populates `harper_toggle_section` with the "Check Grammar" stateful
    /// toggle and registers it in the action group.
    ///
    /// Called once at `realize` time when GSettings become accessible.
    /// Triggers a menu rebuild afterwards so the toggle appears immediately
    /// if the language is already English.
    fn setup_harper_grammar_toggle(&self) {
        let imp = self.imp();

        let Some(settings) = imp.get_settings_from_window() else {
            return;
        };
        let Some(group) = imp.harper_action_group.borrow().clone() else {
            return;
        };
        let Some(toggle_section) = imp.harper_toggle_section.borrow().clone() else {
            return;
        };

        let enabled = settings.boolean("check-grammar");
        let toggle =
            gtk::gio::SimpleAction::new_stateful("toggle-grammar", None, &enabled.to_variant());

        let editor_weak = self.downgrade();
        toggle.connect_activate(move |action, _| {
            let current = action
                .state()
                .and_then(|s| s.get::<bool>())
                .unwrap_or(false);
            let new_value = !current;
            action.set_state(&new_value.to_variant());
            if let Some(editor) = editor_weak.upgrade()
                && let Some(settings) = editor.imp().get_settings_from_window()
            {
                let _ = settings.set_boolean("check-grammar", new_value);
            }
        });

        group.add_action(&toggle);
        toggle_section.append(
            Some(&gettextrs::gettext("Check Grammar")),
            Some("harper.toggle-grammar"),
        );

        // Toggle section is now populated — rebuild so it appears in the
        // menu if the language is already English.
        self.rebuild_harper_menu();
    }

    /// Initialise the Harper worker and fire an immediate full-document lint.
    ///
    /// The menu is rebuilt via `rebuild_harper_menu` to show lint suggestions
    /// now that Harper is active. The worker will park until its dictionary is
    /// loaded before processing the first batch.
    fn enable_harper(&self) {
        let imp = self.imp();

        if imp.harper_worker.borrow().is_none() {
            *imp.harper_worker.borrow_mut() = Some(crate::editor::harper::HarperWorker::new());
        }

        self.rebuild_harper_menu();

        let buffer = self.buffer();
        let start = buffer.start_iter().offset();
        let end = buffer.end_iter().offset();
        if start < end {
            imp.dirty_ranges.borrow_mut().mark_dirty(start, end);
            self.spawn_harper_lint(&imp.dirty_ranges, &imp.harper_lints, &imp.spell_adapter);
        }
    }

    /// Cancel and drop the Harper worker and clear all existing lints.
    ///
    /// The menu is rebuilt via `rebuild_harper_menu` — if the language is
    /// still English the toggle remains visible so the user can re-enable;
    /// if the language changed away from English the wrapper is emptied.
    fn disable_harper(&self) {
        let imp = self.imp();

        if let Some(worker) = imp.harper_worker.borrow_mut().take() {
            worker.cancel();
        }

        imp.dirty_ranges.borrow_mut().clear();

        if let Some(buffer) = imp.buffer.get() {
            clear_all_lints(buffer, &mut imp.harper_lints.borrow_mut());
        }

        self.rebuild_harper_menu();
    }

    /// Returns a clone of the active Harper worker, if one exists.
    ///
    /// Returns `None` if Harper is currently disabled.
    fn harper_worker(&self) -> Option<crate::editor::harper::HarperWorker> {
        self.imp().harper_worker.borrow().clone()
    }

    /// Connects `insert-text` and `delete-range` signals to the buffer so
    /// that every edit marks the affected paragraph range as dirty.
    ///
    /// Must be called after `initialize_buffer`. The dirty ranges are
    /// consumed by `spawn_harper_lint` on each debounce fire.
    pub(super) fn initialize_harper_signals(&self) {
        let imp = self.imp();
        let buffer = self.buffer();
        let dirty_ranges = Rc::clone(&imp.dirty_ranges);

        // ── insert-text ────────────────────────────────────────────────────
        let dirty_insert = Rc::clone(&dirty_ranges);
        let handler_insert = buffer.connect_insert_text(move |_buf, iter, _text| {
            let (para_start, para_end) = paragraph_bounds(iter);
            dirty_insert.borrow_mut().mark_dirty(para_start, para_end);
        });

        // ── delete-range ───────────────────────────────────────────────────
        // When a deletion spans a newline the paragraph boundaries of both
        // the start and end positions must be captured, because the two
        // paragraphs are about to merge into one.
        let dirty_delete = Rc::clone(&dirty_ranges);
        let handler_delete = buffer.connect_delete_range(move |buf, start, end| {
            let deleted_text = buf.text(start, end, false);
            let contains_newline = deleted_text.contains('\n');

            let (para_start, _) = paragraph_bounds(start);
            let para_end = if contains_newline {
                let (_, end_para_end) = paragraph_bounds(end);
                end_para_end
            } else {
                let (_, start_para_end) = paragraph_bounds(start);
                start_para_end
            };

            dirty_delete.borrow_mut().mark_dirty(para_start, para_end);
        });

        imp.signal_handlers
            .borrow_mut()
            .push((buffer.clone().upcast(), handler_insert));
        imp.signal_handlers
            .borrow_mut()
            .push((buffer.clone().upcast(), handler_delete));
    }

    /// Connects to the spelling checker's `language` property so that the
    /// Harper enabled state and menu contents are updated when the locale
    /// changes. Both transitions call `rebuild_harper_menu` which reads the
    /// live language from the adapter, so no separate language state is needed.
    pub(super) fn initialize_harper_language_guard(&self, checker: &libspelling::Checker) {
        let imp = self.imp();
        let editor_weak = self.downgrade();

        let handler_id = checker.connect_notify_local(Some("language"), move |_checker, _| {
            if let Some(editor) = editor_weak.upgrade() {
                // set_harper_enabled calls rebuild_harper_menu internally via
                // enable_harper / disable_harper. If the enabled state doesn't
                // change (e.g. grammar was already off) we still need a rebuild
                // to show or hide the toggle, so call it explicitly afterwards.
                editor.set_harper_enabled(editor.harper_should_be_enabled());
                editor.rebuild_harper_menu();
            }
        });

        imp.signal_handlers
            .borrow_mut()
            .push((checker.clone().upcast(), handler_id));
    }

    /// Drains the dirty-range tracker and dispatches a Harper lint pass.
    ///
    /// All buffer reads happen synchronously on the main thread before the
    /// async task is spawned. The task sends the batch to the worker (which
    /// waits for the dictionary if it is still loading) and applies the
    /// results back on the main thread when they arrive.
    ///
    /// Exits early without spawning if:
    /// - Harper is currently disabled (no worker), or
    /// - There are no dirty ranges to process.
    fn spawn_harper_lint(
        &self,
        dirty_ranges: &Rc<RefCell<DirtyRanges>>,
        harper_lints: &Rc<RefCell<Vec<HarperLint>>>,
        spell_adapter: &Rc<RefCell<Option<libspelling::TextBufferAdapter>>>,
    ) {
        // Exit early if Harper is disabled.
        let Some(worker) = self.harper_worker() else {
            return;
        };

        let language_tag = spell_adapter
            .borrow()
            .as_ref()
            .and_then(|adapter| adapter.checker())
            .and_then(|checker| checker.language())
            .unwrap_or_default();

        let dialect = if language_tag.is_empty() {
            harper_core::Dialect::American
        } else {
            dialect_from_bcp47(&language_tag)
        };

        let ranges = dirty_ranges.borrow_mut().take_dirty();
        if ranges.is_empty() {
            return;
        }

        let buffer = self.buffer();
        let char_count = buffer.char_count();

        // Collect paragraph texts synchronously while we have buffer access.
        let mut batch = crate::editor::harper::LintBatch::new();
        for range in ranges {
            let para_start = range.gtk_start.clamp(0, char_count);
            let para_end = range.gtk_end.clamp(0, char_count);

            if para_start >= para_end {
                continue;
            }

            let start_iter = buffer.iter_at_offset(para_start);
            let end_iter = buffer.iter_at_offset(para_end);
            let text = buffer.text(&start_iter, &end_iter, false).to_string();
            batch.push((text, para_start, para_end));
        }

        if batch.is_empty() {
            return;
        }

        let harper_lints = Rc::clone(harper_lints);
        let editor_weak = self.downgrade();

        gtk::glib::spawn_future_local(async move {
            match worker.lint_batch(batch, dialect).await {
                Ok(results) => {
                    let Some(editor) = editor_weak.upgrade() else {
                        return;
                    };

                    // Don't apply results if Harper was disabled while we were waiting
                    if !editor.imp().harper_enabled.get() {
                        return;
                    }

                    let buffer = editor.buffer();
                    let mut lints = harper_lints.borrow_mut();

                    for para_result in results {
                        apply_lint_results(
                            buffer,
                            &mut lints,
                            para_result.lints,
                            para_result.para_gtk_start,
                            para_result.para_gtk_end,
                        );
                    }
                }
                Err(e) => {
                    eprintln!("HARPER: worker error: {}", e);
                }
            }
        });
    }

    /// Marks the entire buffer dirty and immediately fires a Harper lint pass.
    ///
    /// Called once after a document is loaded so lints appear without waiting
    /// for the first keystroke. The worker will park until its dictionary is
    /// ready if called very shortly after startup.
    ///
    /// No-ops if Harper is currently disabled — `enable_harper` will trigger
    /// a full-document lint if Harper is later enabled.
    pub(crate) fn lint_full_document(&self) {
        if !self.imp().harper_enabled.get() {
            return;
        }

        let imp = self.imp();
        let buffer = self.buffer();
        let start = buffer.start_iter().offset();
        let end = buffer.end_iter().offset();
        imp.dirty_ranges.borrow_mut().mark_dirty(start, end);

        self.spawn_harper_lint(&imp.dirty_ranges, &imp.harper_lints, &imp.spell_adapter);
    }

    /// Installs the gesture and key controller that inject Harper fix
    /// suggestions into the context menu alongside libspelling's items.
    ///
    /// A single permanent wrapper menu is prepended to the extra menu here
    /// and never removed. `rebuild_harper_menu` fills it with whichever
    /// combination of lint and toggle sections is appropriate for the current
    /// state. This keeps lint suggestions and the "Check Grammar" toggle
    /// adjacent in one section, with no positional index assumptions.
    pub(super) fn initialize_harper_context_menu(&self) {
        let imp = self.imp();
        let harper_lints = Rc::clone(&imp.harper_lints);

        // ── Single wrapper + two content sections ──────────────────────────
        let wrapper = gtk::gio::Menu::new();
        let lint_section = gtk::gio::Menu::new();
        let toggle_section = gtk::gio::Menu::new();

        // Insert the wrapper once, permanently, at the front of the extra
        // menu. It starts empty — rebuild_harper_menu fills it as needed.
        if let Some(Ok(menu)) = self
            .view()
            .upcast_ref::<gtk::TextView>()
            .extra_menu()
            .map(|m| m.downcast::<gtk::gio::Menu>())
        {
            menu.prepend_section(None, &wrapper);
        }

        *imp.harper_menu_wrapper.borrow_mut() = Some(wrapper);
        *imp.harper_lint_section.borrow_mut() = Some(lint_section.clone());
        *imp.harper_toggle_section.borrow_mut() = Some(toggle_section);

        // ── Action group — registered once, lint actions rebuilt per invocation
        let action_group = gtk::gio::SimpleActionGroup::new();
        *imp.harper_action_group.borrow_mut() = Some(action_group.clone());

        self.view()
            .insert_action_group("harper", Some(&action_group));

        // ── GestureClick — right-click ─────────────────────────────────────
        let view_weak = self.view().downgrade();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(0); // any button — triggers_context_menu filters below
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);

        let gesture_lints = Rc::clone(&harper_lints);
        let gesture_section = lint_section.clone();
        let gesture_group = action_group.clone();

        gesture.connect_pressed(move |gesture, n_press, x, y| {
            let sequence = gesture.current_sequence();
            let event = gesture.last_event(sequence.as_ref());
            if n_press != 1 || !event.is_some_and(|e| e.triggers_context_menu()) {
                return;
            }

            let Some(view) = view_weak.upgrade() else {
                return;
            };

            let buffer = view
                .buffer()
                .downcast::<sourceview::Buffer>()
                .expect("Expected sourceview::Buffer");

            let (buf_x, buf_y) = view.upcast_ref::<gtk::TextView>().window_to_buffer_coords(
                gtk::TextWindowType::Widget,
                x as i32,
                y as i32,
            );
            let cursor_offset = match view
                .upcast_ref::<gtk::TextView>()
                .iter_at_location(buf_x, buf_y)
            {
                Some(iter) => iter.offset(),
                None => return,
            };

            Self::populate_lint_section(
                &buffer,
                &gesture_lints,
                &gesture_section,
                &gesture_group,
                cursor_offset,
            );
        });

        self.view().add_controller(gesture.clone());
        let _ = imp.harper_gesture.set(gesture);

        // ── EventControllerKey — Menu key and Shift+F10 ────────────────────
        let view_weak = self.view().downgrade();
        let key_lints = Rc::clone(&harper_lints);
        let key_section = lint_section.clone();
        let key_group = action_group.clone();

        let key_controller = gtk::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);

        key_controller.connect_key_pressed(move |_, key, _, modifier| {
            let is_menu_key = key == gtk::gdk::Key::Menu;
            let is_shift_f10 =
                key == gtk::gdk::Key::F10 && modifier.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            if !is_menu_key && !is_shift_f10 {
                return gtk::glib::Propagation::Proceed;
            }

            let Some(view) = view_weak.upgrade() else {
                return gtk::glib::Propagation::Proceed;
            };

            let buffer = view
                .buffer()
                .downcast::<sourceview::Buffer>()
                .expect("Expected sourceview::Buffer");

            let cursor_offset = buffer.iter_at_mark(&buffer.get_insert()).offset();

            Self::populate_lint_section(
                &buffer,
                &key_lints,
                &key_section,
                &key_group,
                cursor_offset,
            );

            gtk::glib::Propagation::Proceed
        });

        self.view().add_controller(key_controller.clone());
        let _ = imp.harper_key_controller.set(key_controller);
    }

    fn initialize_format_context_menu(&self) {
        let format_section = gtk::gio::Menu::new();
        format_section.append(
            Some(&gettextrs::gettext("Tidy Formatting")),
            Some("window.format-document"),
        );

        if let Some(Ok(menu)) = self
            .view()
            .upcast_ref::<gtk::TextView>()
            .extra_menu()
            .map(|m| m.downcast::<gtk::gio::Menu>())
        {
            menu.prepend_section(None, &format_section);
        }
    }

    /// Clear and repopulate `lint_section` and the lint fix actions in
    /// `action_group` with suggestions for any lints covering `cursor_offset`.
    ///
    /// Only the lint fix actions (prefixed `harper-fix-`) are removed —
    /// the persistent `toggle-grammar` action is left untouched.
    ///
    /// Shared by the gesture (mouse) and key controller (keyboard) paths.
    fn populate_lint_section(
        buffer: &sourceview::Buffer,
        harper_lints: &Rc<RefCell<Vec<HarperLint>>>,
        lint_section: &gtk::gio::Menu,
        action_group: &gtk::gio::SimpleActionGroup,
        cursor_offset: i32,
    ) {
        // Remove only lint fix actions, leaving toggle-grammar intact.
        for name in action_group.list_actions() {
            if name.starts_with("harper-fix-") {
                action_group.remove_action(&name);
            }
        }
        lint_section.remove_all();

        // Collect lints whose span covers the cursor/pointer position.
        let matching: Vec<(String, Vec<LintSuggestion>, gtk::TextMark, gtk::TextMark)> =
            harper_lints
                .borrow()
                .iter()
                .filter_map(|lint| {
                    let start = lint.current_start(buffer)?;
                    let end = lint.current_end(buffer)?;
                    if cursor_offset >= start && cursor_offset <= end {
                        Some((
                            lint.message.clone(),
                            lint.suggestions.clone(),
                            lint.start_mark.clone(),
                            lint.end_mark.clone(),
                        ))
                    } else {
                        None
                    }
                })
                .collect();

        if matching.is_empty() {
            return;
        }

        let mut action_index: u32 = 0;

        for (_message, suggestions, start_mark, end_mark) in matching {
            // Resolve the span text for context-aware labels, e.g.
            // "emails," instead of "Insert: ,".
            let span_text = if !start_mark.is_deleted() && !end_mark.is_deleted() {
                let start_iter = buffer.iter_at_mark(&start_mark);
                let end_iter = buffer.iter_at_mark(&end_mark);
                buffer.text(&start_iter, &end_iter, false).to_string()
            } else {
                String::new()
            };

            // Fall back to a plain Remove if the lint has no suggestions.
            let suggestions_to_show: Vec<LintSuggestion> = if suggestions.is_empty() {
                vec![LintSuggestion::Remove]
            } else {
                suggestions
            };

            for suggestion in suggestions_to_show {
                let label = suggestion.label_with_context(&span_text);
                let action_name = format!("harper-fix-{}", action_index);
                action_index += 1;

                let action = gtk::gio::SimpleAction::new(&action_name, None);
                let buffer_weak = buffer.downgrade();
                let harper_lints_weak = Rc::downgrade(harper_lints);
                let start_mark = start_mark.clone();
                let end_mark = end_mark.clone();
                let suggestion = suggestion.clone();

                action.connect_activate(move |_, _| {
                    let Some(buffer) = buffer_weak.upgrade() else {
                        return;
                    };
                    let Some(harper_lints) = harper_lints_weak.upgrade() else {
                        return;
                    };
                    apply_suggestion(
                        &buffer,
                        &mut harper_lints.borrow_mut(),
                        &start_mark,
                        &end_mark,
                        &suggestion,
                    );
                });

                action_group.add_action(&action);
                lint_section.append(Some(&label), Some(&format!("harper.{}", action_name)));
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// § 7  Typst / IDE integration
// ─────────────────────────────────────────────────────────────────────────────

impl TypesetterEditor {
    /// Registers the autocompletion provider with the source view.
    pub fn initialize_completion(&self) {
        let imp = self.imp();
        let view = self.view();
        let completion_obj = view.completion();

        completion_obj.set_page_size(8);
        let provider = completion::TypesetterCompletionProvider::new();

        *imp.completion_provider.borrow_mut() = Some(provider.clone());
        completion_obj.add_provider(&provider.upcast::<sourceview::CompletionProvider>());
    }

    /// Registers the Typst, Harper, and diagnostic hover providers with the source view.
    pub fn initialize_hover(&self) {
        let imp = self.imp();
        let view = self.view();
        let harper_lints = Rc::clone(&imp.harper_lints);
        let diagnostic_lints = Rc::clone(&imp.diagnostic_lints);

        let (typst_provider, harper_provider, diagnostic_provider) =
            crate::editor::hover::setup_hover(view, harper_lints, diagnostic_lints);

        *imp.hover_provider.borrow_mut() = Some(typst_provider);
        *imp.harper_hover_provider.borrow_mut() = Some(harper_provider);
        *imp.diagnostic_hover_provider.borrow_mut() = Some(diagnostic_provider);
    }

    /// Converts a `GtkTextIter` to a UTF-8 byte offset for use with the
    /// Typst compiler, which expects byte offsets rather than codepoint counts.
    pub fn iter_to_byte_offset(&self, iter: &gtk::TextIter) -> usize {
        let buffer = self.buffer();
        let start = buffer.start_iter();
        let text = buffer.text(&start, iter, false);
        text.len()
    }

    /// Returns the current cursor position as a UTF-8 byte offset.
    pub fn cursor_byte_offset(&self) -> usize {
        let buffer = self.buffer();
        let cursor_mark = buffer.get_insert();
        let iter = buffer.iter_at_mark(&cursor_mark);
        self.iter_to_byte_offset(&iter)
    }

    /// Converts a UTF-8 byte offset from the Typst compiler to a
    /// `GtkTextIter` (which uses codepoint counts internally).
    pub fn byte_offset_to_iter(&self, byte_offset: usize) -> gtk::TextIter {
        let buffer = self.buffer();
        let text = self.contents();
        let char_offset = utils::byte_to_char_offset(&text, byte_offset);
        buffer.iter_at_offset(char_offset as i32)
    }

    /// Converts a UTF-8 byte range from the Typst compiler to a pair of
    /// `GtkTextIter`s.
    pub fn byte_range_to_iters(&self, start: usize, end: usize) -> (gtk::TextIter, gtk::TextIter) {
        (
            self.byte_offset_to_iter(start),
            self.byte_offset_to_iter(end),
        )
    }

    /// Replace all diagnostic lints with fresh ones from the latest compilation.
    ///
    /// Clears existing marks, then creates new mark pairs for each diagnostic
    /// that has a location range. Diagnostics without a range (e.g. file-level
    /// errors) are silently skipped — they have no span to anchor to.
    pub fn update_diagnostic_lints(&self, diagnostics: &[crate::typst_system::PreviewDiagnostic]) {
        let imp = self.imp();
        let buffer = self.buffer();
        let mut lints = imp.diagnostic_lints.borrow_mut();

        // Clear existing marks before dropping the old lints
        for lint in lints.drain(..) {
            lint.delete_marks(buffer);
        }

        for diag in diagnostics {
            let Some((start, end)) = diag.location_range else {
                continue;
            };
            // Clamp to buffer bounds defensively
            let char_count = buffer.char_count();
            let start = (start as i32).min(char_count);
            let end = (end as i32).min(char_count);
            if start >= end {
                continue;
            }
            lints.push(hover::DiagnosticLint::new(
                buffer,
                start,
                end,
                &diag.message,
                diag.hints.clone(),
            ));
        }
    }
}
