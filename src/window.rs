// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::{prelude::*, subclass::prelude::*};
use gettextrs::gettext;
use gtk::{
    CompositeTemplate, TemplateChild, gdk,
    gio::{self},
    glib::{Properties, clone},
};
use std::{
    cell::{Cell, OnceCell, RefCell},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

use crate::{
    config::APP_ID, editor, explorer, file_io, preview, recents_menu, templates_menu,
    theme_selector::ThemeSelector, typst_system::TypstSystem, utils,
};

pub const SHOW_HEADERBAR_DISTANCE_PX: f64 = 5.0;
pub const FULLSCREEN_MARGIN: i32 = 64;
pub const NORMAL_MARGIN: i32 = 16;
pub const PROGRESS_PULSE_INTERVAL: Duration = Duration::from_millis(100);
pub const TOAST_TIMEOUT: u32 = 5;
pub const PREVIEW_JUMP_COOLDOWN: Duration = Duration::from_secs(1);
pub const AUTOSAVE_DEBOUNCE_DELAY: Duration = Duration::from_secs(5);
pub const MIN_EDITOR_WIDTH: i32 = 250;
pub const MIN_PREVIEW_WIDTH: i32 = 270;
pub const DIVIDER_DEBOUNCE_DELAY: Duration = Duration::from_millis(250);

/// Recursively clears any stale `PRELIGHT`/`ACTIVE` CSS state flags from
/// `widget` and its descendants.
fn clear_stale_hover(widget: &gtk::Widget) {
    let flags = widget.state_flags();
    if flags.intersects(gtk::StateFlags::PRELIGHT | gtk::StateFlags::ACTIVE) {
        widget.set_state_flags(
            flags & !(gtk::StateFlags::PRELIGHT | gtk::StateFlags::ACTIVE),
            true,
        );
    }

    let mut child = widget.first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        clear_stale_hover(&c);
    }
}

mod imp {
    use super::*;
    #[derive(CompositeTemplate, Debug, Default, Properties)]
    #[properties(wrapper_type = super::TypesetterWindow)]
    #[template(resource = "/dev/whetstone/editor/gtk/window.ui")]
    pub struct TypesetterWindow {
        // Application settings
        pub(crate) settings: OnceCell<gio::Settings>,
        // File state
        #[property(get, set, default = false)]
        pub(crate) current_file_is_unsaved: Cell<bool>,
        #[property(get, set, default = None)]
        pub(crate) current_file_path: RefCell<Option<PathBuf>>,
        #[property(get, set, nullable)]
        pub(crate) display_file_directory: RefCell<Option<String>>,
        pub(crate) force_close: Cell<bool>,
        #[property(get, set, default = None)]
        pub(crate) project_root: RefCell<Option<PathBuf>>,
        pub(crate) dependency_monitor:
            RefCell<Option<crate::file_io::monitoring::DependencyMonitor>>,
        // Autosave debounce
        pub(crate) autosave_debounce_id: RefCell<Option<gtk::glib::SourceId>>,
        // Template widgets
        #[template_child]
        pub(crate) explorer_split_view: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub(crate) explorer_container: TemplateChild<explorer::TypesetterExplorer>,
        #[template_child]
        pub(crate) explorer_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub(crate) editor_container: TemplateChild<editor::TypesetterEditor>,
        #[template_child]
        pub(crate) preview_container: TemplateChild<preview::TypesetterPreview>,
        #[template_child]
        pub(crate) split_view: TemplateChild<gtk::Paned>,
        pub(crate) saved_divider_position: Cell<i32>,
        pub(crate) divider_debounce_id: RefCell<Option<gtk::glib::SourceId>>,
        #[template_child]
        pub(crate) toolbar_view: TemplateChild<adw::ToolbarView>,
        // Toolbar and controls
        #[template_child]
        pub(crate) header_bar_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub(crate) header_bar_subtitle: TemplateChild<gtk::Label>,
        #[template_child]
        pub(crate) unsaved_indicator: TemplateChild<gtk::Label>,
        #[template_child]
        pub(crate) main_menu_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub(crate) new_file_button: TemplateChild<adw::SplitButton>,
        #[template_child]
        pub(crate) open_file_button: TemplateChild<adw::SplitButton>,
        #[template_child]
        pub(crate) save_file_button: TemplateChild<adw::SplitButton>,
        #[template_child]
        pub(crate) search_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub(crate) preview_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub(crate) preview_error_badge: TemplateChild<gtk::Label>,
        #[template_child]
        pub(crate) project_folder_banner: TemplateChild<adw::Banner>,
        // Progress bar and feedback
        #[template_child]
        pub(crate) progress_bar: TemplateChild<gtk::ProgressBar>,
        pub(crate) pulse_timeout_id: RefCell<Option<gtk::glib::SourceId>>,
        #[template_child]
        pub(crate) toast_overlay: TemplateChild<adw::ToastOverlay>,
        // Typst system
        pub(crate) compiler_system: OnceCell<TypstSystem>,
        pub(crate) next_allowed_jump: RefCell<Option<Instant>>,
        pub(crate) pending_trailing: RefCell<Option<gtk::glib::SourceId>>,
        // Fullscreen mode
        pub(crate) motion_controller: OnceCell<gtk::EventControllerMotion>,
        pub(crate) touch_gesture: OnceCell<gtk::GestureClick>,
        pub(crate) magnifier_key_controller: OnceCell<gtk::EventControllerKey>,
        pub(crate) last_mouse_y: Cell<f64>,
        pub(crate) is_touch: Cell<bool>,
        // Signal handler tracking for cleanup
        pub(crate) signal_handlers: RefCell<Vec<(gtk::glib::Object, gtk::glib::SignalHandlerId)>>,
        // Property bindings for cleanup
        pub(crate) property_bindings: RefCell<Vec<gtk::glib::Binding>>,
        // Objects that need explicit cleanup
        pub(crate) theme_selector: RefCell<Option<ThemeSelector>>,
        pub(crate) recent_files_popover:
            RefCell<Option<recents_menu::TypesetterRecentFilesPopover>>,
        pub(crate) templates_popover: RefCell<Option<templates_menu::TypesetterTemplatesPopover>>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for TypesetterWindow {
        const NAME: &'static str = "TypesetterWindow";
        type Type = super::TypesetterWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            explorer::TypesetterExplorer::ensure_type();
            klass.bind_template();

            klass.install_action("window.toggle-explorer", None, |window, _, _| {
                let button = &window.imp().explorer_button;
                button.set_active(!button.is_active());
            });

            klass.install_action("window.close", None, |window, _, _| {
                window.close();
            });

            klass.install_action("window.file-new", None, |window, _, _| {
                window.new_file();
            });

            klass.install_action("window.file-open", None, |window, _, _| {
                gtk::glib::MainContext::default().spawn_local(gtk::glib::clone! {
                    #[strong] window,
                    async move {
                        match window
                            .open_file(window.imp().editor_container.buffer(), None)
                            .await
                        {
                            Ok(Some(opened_file)) => {
                                let project_root = window.project_root()
                                    .map(|p| gio::File::for_path(&p));
                                file_io::recent::add_recent_file(
                                    &opened_file,
                                    project_root.as_ref(),
                                );
                            }
                            Ok(None) => {}
                            Err(err) => {
                                window.show_toast(
                                    &format!("{}{}", &gettext("Unable to open file: "), err),
                                    None,
                                );
                            }
                        }
                    }
                });
            });

            klass.install_action("window.file-recents", None, |window, _, _| {
                if let Some(popover) = window.imp().open_file_button.popover() {
                    popover.popup();
                }
            });

            klass.install_action("window.file-save", None, |window, _, _| {
                gtk::glib::MainContext::default().spawn_local(gtk::glib::clone! {
                    #[strong] window,
                    async move {
                        match window
                            .save_file(
                                window.imp().editor_container.buffer(),
                                window.current_file_path().map(|p| gio::File::for_path(&p)),
                            )
                            .await
                        {
                            Ok(Some(saved_file)) => {
                                let project_root = window.project_root()
                                    .map(|p| gio::File::for_path(&p));
                                file_io::recent::add_recent_file(
                                    &saved_file,
                                    project_root.as_ref(),
                                );
                            }
                            Ok(None) => {}
                            Err(err) => {
                                window.show_toast(&format!("{}{}", &gettext("Save failed: "), err), None);
                            }
                        }
                    }
                });
            });

            klass.install_action("window.file-save-as", None, |window, _, _| {
                gtk::glib::MainContext::default().spawn_local(gtk::glib::clone! {
                    #[strong] window,
                    async move {
                        match window
                            .save_file(window.imp().editor_container.buffer(), None)
                            .await
                        {
                            Ok(Some(saved_file)) => {
                                let project_root = window.project_root()
                                    .map(|p| gio::File::for_path(&p));
                                file_io::recent::add_recent_file(
                                    &saved_file,
                                    project_root.as_ref(),
                                );
                            }
                            Ok(None) => {}
                            Err(err) => {
                                window.show_toast(&format!("{}{}", &gettext("Save failed: "), err), None);
                            }
                        }
                    }
                });
            });

            klass.install_action("window.file-export-html", None, |window, _, _| {
                gtk::glib::MainContext::default().spawn_local(gtk::glib::clone! {
                    #[strong] window,
                    async move {
                        match window
                            .export_html_file(
                                window.imp().editor_container.contents(),
                                window.current_file_path(),
                                window.project_root(),
                            )
                            .await
                        {
                            Ok(Some(path)) => {
                                window.stop_progress_pulse();
                                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                                    window.show_toast(
                                        &format!("{} \"{}\"", &gettext("Exported"), file_name),
                                        Some(&path),
                                    );
                                } else {
                                    window.show_toast(&gettext("Exported file"), Some(&path));
                                }
                            }
                            Ok(None) => {
                                window.stop_progress_pulse();
                            }
                            Err(err) => {
                                window.stop_progress_pulse();
                                window.show_toast(
                                    &format!("{}{}", &gettext("Export failed: "), err),
                                    None,
                                );
                            }
                        }
                    }
                });
            });

            klass.install_action("window.file-export-markdown", None, |window, _, _| {
                gtk::glib::MainContext::default().spawn_local(gtk::glib::clone! {
                    #[strong] window,
                    async move {
                        match window
                            .export_markdown_file(
                                window.imp().editor_container.contents(),
                                window.current_file_path(),
                                window.project_root(),
                            )
                            .await
                        {
                            Ok(Some(path)) => {
                                window.stop_progress_pulse();
                                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                                    window.show_toast(
                                        &format!("{} \"{}\"", &gettext("Exported"), file_name),
                                        Some(&path),
                                    );
                                } else {
                                    window.show_toast(&gettext("Exported file"), Some(&path));
                                }
                            }
                            Ok(None) => {
                                window.stop_progress_pulse();
                            }
                            Err(err) => {
                                window.stop_progress_pulse();
                                window.show_toast(
                                    &format!("{}{}", &gettext("Export failed: "), err),
                                    None,
                                );
                            }
                        }
                    }
                });
            });

            klass.install_action("window.file-export-pdf", None, |window, _, _| {
                gtk::glib::MainContext::default().spawn_local(gtk::glib::clone! {
                    #[strong] window,
                    async move {
                        match window
                            .export_pdf_file(
                                window.imp().editor_container.contents(),
                                window.current_file_path(),
                                window.project_root(),
                            )
                            .await
                        {
                            Ok(Some(path)) => {
                                window.stop_progress_pulse();
                                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                                    window.show_toast(
                                        &format!("{} \"{}\"", &gettext("Exported"), file_name),
                                        Some(&path),
                                    );
                                } else {
                                    window.show_toast(&gettext("Exported file"), Some(&path));
                                }
                            }
                            Ok(None) => {
                                window.stop_progress_pulse();
                            }
                            Err(err) => {
                                window.stop_progress_pulse();
                                window.show_toast(
                                    &format!("{}{}", &gettext("Export failed: "), err),
                                    None,
                                );
                            }
                        }
                    }
                });
            });

            klass.install_action("window.find-next", None, move |window, _, _| {
                window.search_toolbar().find_next();
            });

            klass.install_action("window.find-previous", None, move |window, _, _| {
                window.search_toolbar().find_previous();
            });

            klass.install_action("window.document-outline", None, |window, _, _| {
                let outline_dialog = crate::outline::TypesetterOutline::new();
                outline_dialog.present(Some(window));
            });

            klass.install_action("window.document-statistics", None, |window, _, _| {
                let statistics_dialog = crate::statistics::TypesetterStatistics::new();
                statistics_dialog.present(Some(window));
            });

            klass.install_action("window.format-document", None, |window, _, _| {
                let imp = window.imp();
                let editor = imp.editor_container.get();
                crate::editor::formatter::format_document(&editor);
            });

            klass.install_action("window.fullscreen", None, |window, _, _| {
                if window.is_fullscreen() {
                    window.unfullscreen();
                } else {
                    window.fullscreen();
                }
            });

            klass.install_action("window.preview", None, |window, _, _| {
                let btn = window.imp().preview_button.get();
                btn.set_active(!btn.is_active());
            });

            klass.install_action("window.replace-all", None, move |window, _, _| {
                window.search_toolbar().replace_all();
            });

            klass.install_action("window.replace-one", None, move |window, _, _| {
                window.search_toolbar().replace_next();
            });

            klass.install_action("window.search-cancel", None, move |window, _, _| {
                window.search_toolbar().search_cancel();
                let imp = window.imp();
                let editor = imp.editor_container.get();
                let view = editor.view();
                let buffer = editor.buffer();
                let insert_mark = buffer.get_insert();
                let mut iter = buffer.iter_at_mark(&insert_mark);
                view.grab_focus();
                let fraction = view.scroll_fraction();
                view.scroll_to_iter(&mut iter, 0.0, true, 0.0, fraction);
            });

            klass.install_action("window.search-find", None, move |window, _, _| {
                window.search_toolbar().search_find();
            });

            klass.install_action("window.search-replace", None, move |window, _, _| {
                window.search_toolbar().search_replace();
            });

            klass.install_action("window.toggle-case-sensitive", None, |window, _, _| {
                let toolbar = window.search_toolbar();
                let current = toolbar.case_sensitive_mode();
                toolbar.set_case_sensitive_mode(!current);
            });

            klass.install_action("window.toggle-regex", None, |window, _, _| {
                let toolbar = window.search_toolbar();
                let current = toolbar.regex_mode();
                toolbar.set_regex_mode(!current);
            });

            #[cfg(debug_assertions)]
            klass.install_action("window.memory-diagnostic", None, |window, _, _| {
                // Report memory counters
                crate::memory_tracking::MEMORY_COUNTERS.report();

                // Report cache and preview state
                let imp = window.imp();
                let cache_size = imp.preview_container.imp().texture_cache.size();
                let objects_with_textures = (0..imp.preview_container.imp().page_store.n_items())
                    .filter_map(|i| imp.preview_container.imp().page_store.item(i))
                    .filter_map(|obj| obj.downcast::<crate::preview::PreviewObject>().ok())
                    .filter(|page| page.has_texture())
                    .count();
                let visible_pages = imp.preview_container.imp().visible_pages.borrow().len();

                println!("\nPREVIEW STATE:");
                println!("  Visible pages: {}", visible_pages);
                println!("  Cache size: {}", cache_size);
                println!("  Objects with textures: {}", objects_with_textures);

                // Report compiler system state
                if let Some(compiler) = imp.compiler_system.get() {
                    #[cfg(debug_assertions)]
                    compiler.debug_memory_state();
                }

                println!();
            });

            // Keyboard bindings
            let key_bindings: &[(gdk::Key, gdk::ModifierType, &str)] = &[
                (gdk::Key::w, gdk::ModifierType::CONTROL_MASK, "window.close"),
                (
                    gdk::Key::n,
                    gdk::ModifierType::CONTROL_MASK,
                    "window.file-new",
                ),
                (
                    gdk::Key::o,
                    gdk::ModifierType::CONTROL_MASK,
                    "window.file-open",
                ),
                (
                    gdk::Key::r,
                    gdk::ModifierType::CONTROL_MASK,
                    "window.file-recents",
                ),
                (
                    gdk::Key::s,
                    gdk::ModifierType::CONTROL_MASK,
                    "window.file-save",
                ),
                (
                    gdk::Key::s,
                    gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK,
                    "window.file-save-as",
                ),
                (
                    gdk::Key::e,
                    gdk::ModifierType::CONTROL_MASK,
                    "window.file-export-pdf",
                ),
                (
                    gdk::Key::g,
                    gdk::ModifierType::CONTROL_MASK,
                    "window.find-next",
                ),
                (
                    gdk::Key::g,
                    gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK,
                    "window.find-previous",
                ),
                (
                    gdk::Key::F6,
                    gdk::ModifierType::empty(),
                    "window.document-outline",
                ),
                (
                    gdk::Key::F8,
                    gdk::ModifierType::empty(),
                    "window.document-statistics",
                ),
                (
                    gdk::Key::t,
                    gdk::ModifierType::CONTROL_MASK,
                    "window.format-document",
                ),
                (
                    gdk::Key::F11,
                    gdk::ModifierType::empty(),
                    "window.fullscreen",
                ),
                (gdk::Key::F5, gdk::ModifierType::empty(), "window.preview"),
                (
                    gdk::Key::p,
                    gdk::ModifierType::CONTROL_MASK,
                    "window.preview",
                ),
                (
                    gdk::Key::Escape,
                    gdk::ModifierType::empty(),
                    "window.search-cancel",
                ),
                (
                    gdk::Key::f,
                    gdk::ModifierType::CONTROL_MASK,
                    "window.search-find",
                ),
                (
                    gdk::Key::h,
                    gdk::ModifierType::CONTROL_MASK,
                    "window.search-replace",
                ),
                (
                    gdk::Key::c,
                    gdk::ModifierType::ALT_MASK,
                    "window.toggle-case-sensitive",
                ),
                (
                    gdk::Key::r,
                    gdk::ModifierType::ALT_MASK,
                    "window.toggle-regex",
                ),
                (
                    gdk::Key::F9,
                    gdk::ModifierType::empty(),
                    "window.toggle-explorer",
                ),
                #[cfg(debug_assertions)]
                (
                    gdk::Key::m,
                    gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK,
                    "window.memory-diagnostic",
                ),
            ];

            for (key, modifier, action) in key_bindings {
                klass.add_binding_action(*key, *modifier, action);
            }
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[gtk::glib::derived_properties]
    impl ObjectImpl for TypesetterWindow {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            obj.apply_preferences();
            obj.bind_properties();
            obj.bind_settings();
            obj.initialize_editor_debounce();
            obj.initialize_fullscreen_mode();
            obj.initialize_magnifier_key_handler();
            obj.initialize_main_menu_widgets();
            obj.initialize_recent_files_popover();
            obj.initialize_templates_popover();
            obj.initialize_preview_jump();
            obj.initialize_unsaved_state_tracking();
            obj.initialize_dependency_monitor();
            obj.initialize_autocomplete();
            obj.initialize_hover();
            obj.setup_preview_paned();
            obj.restore_preview_state();
            obj.setup_project_folder_banner();
            obj.setup_explorer();

            obj.connect_close_request(move |window| window.handle_close_request());

            obj.connect_destroy(clone! {
                move |window| {
                    let imp = window.imp();
                    imp.preview_container.cleanup();
                }
            });
        }

        fn dispose(&self) {
            // Clean up preview jump state
            if let Some(source_id) = self.pending_trailing.borrow_mut().take() {
                source_id.remove();
            }
            *self.next_allowed_jump.borrow_mut() = None;

            // Clean up preview container
            self.preview_container.cleanup();

            // Disconnect all signal handlers stored with their objects
            for (object, handler_id) in self.signal_handlers.borrow_mut().drain(..) {
                object.disconnect(handler_id);
            }

            // Stop monitoring any referenced files (DependencyMonitor cancels its monitors on Drop, so just take it)
            self.dependency_monitor.borrow_mut().take();

            // Unbind all property bindings
            self.property_bindings.borrow_mut().clear();

            // Clean up all timeouts
            for timeout_id in [
                self.pulse_timeout_id.borrow_mut().take(),
                self.autosave_debounce_id.borrow_mut().take(),
                self.divider_debounce_id.borrow_mut().take(),
            ]
            .into_iter()
            .flatten()
            {
                timeout_id.remove();
            }

            // Cancel compiler operations
            if let Some(compiler) = self.compiler_system.get() {
                compiler.cancel_all();
            }

            // Remove event controllers
            let obj = self.obj();
            if let Some(controller) = self.motion_controller.get() {
                obj.remove_controller(controller);
            }
            if let Some(gesture) = self.touch_gesture.get() {
                obj.remove_controller(gesture);
            }
            if let Some(controller) = self.magnifier_key_controller.get() {
                obj.remove_controller(controller);
            }

            // Clean up stored widgets that might have signal handlers
            self.theme_selector.borrow_mut().take();
            self.recent_files_popover.borrow_mut().take();
            self.templates_popover.borrow_mut().take();
        }
    }

    impl WidgetImpl for TypesetterWindow {}

    impl WindowImpl for TypesetterWindow {}

    impl ApplicationWindowImpl for TypesetterWindow {}

    impl AdwApplicationWindowImpl for TypesetterWindow {}
}

gtk::glib::wrapper! {
    pub struct TypesetterWindow(ObjectSubclass<imp::TypesetterWindow>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow, adw::ApplicationWindow,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Buildable, gtk::ConstraintTarget, gtk::Accessible, gtk::Orientable, gtk::Actionable, gtk::ShortcutManager, gtk::Root, gtk::Native;
}

impl TypesetterWindow {
    pub fn new<P: IsA<gtk::Application>>(application: &P, file: Option<gio::File>) -> Self {
        let obj: TypesetterWindow = gtk::glib::Object::builder()
            .property("application", application)
            .build();

        if let Some(f) = file {
            let window_weak = obj.downgrade();
            gtk::glib::MainContext::default().spawn_local(async move {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                match window
                    .open_file(window.imp().editor_container.buffer(), Some(f))
                    .await
                {
                    Ok(Some(opened_file)) => {
                        let project_root = window.project_root().map(|p| gio::File::for_path(&p));
                        file_io::recent::add_recent_file(&opened_file, project_root.as_ref());
                    }
                    Ok(None) => {}
                    Err(err) => {
                        window.show_toast(
                            &format!("{}{}", &gettext("Unable to open file: "), err),
                            None,
                        );
                    }
                }
            });
        } else {
            let window_weak = obj.downgrade();
            gtk::glib::MainContext::default().spawn_local(async move {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                let text = window.imp().editor_container.contents();
                window.imp().preview_container.update_preview(&text);
            });
        }

        obj
    }

    /// Get or initialize the compiler system
    pub async fn compiler_system(&self) -> Result<&TypstSystem, gtk::glib::Error> {
        // Already initialized
        if let Some(worker) = self.imp().compiler_system.get() {
            return Ok(worker);
        }

        // Initialize
        let (sender, receiver) = oneshot::channel();

        let rt = crate::application::TypesetterApplication::tokio_runtime();
        rt.spawn(async move {
            let worker = TypstSystem::new();
            let _ = sender.send(worker);
        });

        let worker = receiver.await.map_err(|e| {
            gtk::glib::Error::new(
                gtk::glib::FileError::Failed,
                &format!("Failed to initialize compiler worker: {}", e),
            )
        })?;

        // Try to set, but if it fails (race condition), just return the existing one
        let _ = self.imp().compiler_system.set(worker);

        // Always return the value that's in the cell now
        Ok(self.imp().compiler_system.get().unwrap())
    }

    fn search_toolbar(&self) -> crate::editor::search_toolbar::SearchToolbar {
        self.imp().editor_container.imp().search_toolbar.get()
    }

    pub(crate) fn settings(&self) -> gio::Settings {
        self.imp()
            .settings
            .get_or_init(|| gio::Settings::new(APP_ID))
            .clone()
    }

    fn bind_properties(&self) {
        let imp = self.imp();

        // Bind the saved/unsaved state to the saved/unsaved indicator in the header bar
        let binding1 = self
            .bind_property(
                "current-file-is-unsaved",
                &*imp.unsaved_indicator,
                "visible",
            )
            .sync_create()
            .build();

        // Use the working file path to ascertain the filename for the window title
        let binding2 = self
            .bind_property(
                "current-file-path",
                self.upcast_ref::<gtk::ApplicationWindow>(),
                "title",
            )
            .sync_create()
            .transform_to(|_, path: Option<PathBuf>| {
                Some(utils::file_display_name(
                    path.as_ref(),
                    &gettext("New File"),
                ))
            })
            .build();

        // Use the working file path to ascertain the filename for the header bar title
        let binding3 = self
            .bind_property("current-file-path", &*imp.header_bar_title, "label")
            .sync_create()
            .transform_to(|_, path: Option<PathBuf>| {
                Some(utils::file_display_name(
                    path.as_ref(),
                    &gettext("New File"),
                ))
            })
            .build();

        // Bind the search toolbar to the search button
        let binding4 = imp
            .search_button
            .bind_property("active", self.search_toolbar().revealer(), "reveal-child")
            .bidirectional()
            .sync_create()
            .build();

        // Bind real file directory presence to subtitle visibility
        let binding5 = self
            .bind_property(
                "display-file-directory",
                &*imp.header_bar_subtitle,
                "visible",
            )
            .sync_create()
            .transform_to(|_, directory: Option<String>| Some(directory.is_some()))
            .build();

        // Bind real file path to header bar subtitle
        let binding6 = self
            .bind_property("display-file-directory", &*imp.header_bar_subtitle, "label")
            .sync_create()
            .transform_to(|_, directory: Option<String>| directory.or(Some(String::new())))
            .build();

        // Store all bindings
        imp.property_bindings
            .borrow_mut()
            .extend([binding1, binding2, binding3, binding4, binding5, binding6]);
    }

    fn bind_settings(&self) {
        let settings = self.settings();
        let imp = self.imp();

        settings
            .bind(
                "highlight-current-line",
                imp.editor_container.view(),
                "highlight-current-line",
            )
            .build();

        // Theme and style scheme changes
        for key in ["theme", "style-scheme"] {
            let handler_id = settings.connect_changed(
                Some(key),
                clone! {
                    #[weak(rename_to = window)] self,
                    move |settings, _| {
                        let style_scheme = settings.string("style-scheme");
                        let theme = settings.string("theme");
                        window.apply_theme_style(&style_scheme, &theme);
                    }
                },
            );
            imp.signal_handlers
                .borrow_mut()
                .push((settings.clone().upcast(), handler_id));
        }

        let style_manager = adw::StyleManager::default();
        let handler_id = style_manager.connect_dark_notify(clone! {
            #[weak(rename_to = window)] self,
            move |_| {
                let settings = window.settings();
                let style_scheme = settings.string("style-scheme");
                let theme = settings.string("theme");
                window.apply_theme_style(&style_scheme, &theme);
            }
        });
        imp.signal_handlers
            .borrow_mut()
            .push((style_manager.upcast(), handler_id));

        // Sync spell checking enabled state with settings
        let handler_id = settings.connect_changed(
            Some("check-spelling"),
            clone! {
                #[weak(rename_to = window)] self,
                move |settings, _| {
                    let enabled = settings.boolean("check-spelling");
                    let editor = window.imp().editor_container.get();
                    if let Some(adapter) = editor.imp().spell_adapter.borrow().as_ref() {
                        adapter.set_enabled(enabled);
                    }
                }
            },
        );
        imp.signal_handlers
            .borrow_mut()
            .push((settings.clone().upcast(), handler_id));

        // Add after the check-spelling handler:
        let handler_id = settings.connect_changed(
            Some("check-grammar"),
            clone! {
                #[weak(rename_to = window)] self,
                move |_, _| {
                    let editor = window.imp().editor_container.get();
                    editor.set_harper_enabled(editor.harper_should_be_enabled());
                    editor.sync_grammar_toggle_state();
                }
            },
        );
        imp.signal_handlers
            .borrow_mut()
            .push((settings.clone().upcast(), handler_id));
    }

    fn apply_preferences(&self) {
        let settings = self.settings();
        let style_scheme = settings.string("style-scheme");
        let theme = settings.string("theme");
        self.apply_theme_style(&style_scheme, &theme);
    }

    fn apply_theme_style(&self, style_scheme: &str, theme: &str) {
        use sourceview::prelude::BufferExt;

        let style_manager = adw::StyleManager::default();
        let color_scheme = match theme {
            "light" => adw::ColorScheme::ForceLight,
            "dark" => adw::ColorScheme::ForceDark,
            _ => adw::ColorScheme::Default,
        };
        style_manager.set_color_scheme(color_scheme);

        let editor = self.imp().editor_container.get();
        let buffer = editor.buffer();
        let resolved_scheme = match theme {
            "light" => style_scheme.to_string(),
            "dark" => format!("{}-dark", style_scheme),
            "auto" => {
                if style_manager.is_dark() {
                    format!("{}-dark", style_scheme)
                } else {
                    style_scheme.to_string()
                }
            }
            _ => style_scheme.to_string(),
        };

        if let Some(source_buffer) = buffer.downcast_ref::<sourceview::Buffer>() {
            let scheme_manager = sourceview::StyleSchemeManager::default();
            if let Some(scheme) = scheme_manager.scheme(&resolved_scheme) {
                source_buffer.set_style_scheme(Some(&scheme));
            }
        }
    }

    fn initialize_editor_debounce(&self) {
        let settings = self.settings();
        let imp = self.imp();
        let editor = imp.editor_container.get();
        let buffer = editor.buffer();
        let preview_container = imp.preview_container.clone();
        let window_weak = self.downgrade();

        // Autosave state
        let autosave_enabled = std::rc::Rc::new(Cell::new(settings.boolean("autosave")));

        let handler_id = settings.connect_changed(
            Some("autosave"),
            gtk::glib::clone! {
                #[strong] autosave_enabled,
                move |s, _| {
                    autosave_enabled.set(s.boolean("autosave"));
                }
            },
        );
        imp.signal_handlers
            .borrow_mut()
            .push((settings.clone().upcast(), handler_id));

        // Debounce 1: Document compilation — always compile, rendering is gated inside handle_compilation_result
        imp.editor_container.on_buffer_changed(
            buffer,
            gtk::glib::clone! {
                #[weak] preview_container,
                move |text| {
                    preview_container.update_preview(&text);
                }
            },
        );

        // Debounce 2: Autosave
        let pending_save = std::rc::Rc::new(Cell::new(false));

        let autosave_handler_id = buffer.connect_changed(gtk::glib::clone! {
            #[strong] window_weak,
            #[strong] autosave_enabled,
            #[strong] pending_save,
            move |_| {
                if let Some(window) = window_weak.upgrade() {
                    let imp = window.imp();

                    // Cancel any pending autosave timeout
                    if let Some(source_id) = imp.autosave_debounce_id.borrow_mut().take() {
                        source_id.remove();
                    }

                    // Schedule new autosave
                    let source_id = gtk::glib::timeout_add_local_once(
                        AUTOSAVE_DEBOUNCE_DELAY,
                        gtk::glib::clone! {
                            #[weak] window,
                            #[strong] autosave_enabled,
                            #[strong] pending_save,
                            move || {
                                // Autosave if enabled
                                if autosave_enabled.get()
                                    && !pending_save.get()
                                    && window.current_file_path().is_some()
                                {
                                    pending_save.set(true);
                                    gtk::glib::MainContext::default().spawn_local(gtk::glib::clone! {
                                        #[weak] window,
                                        #[strong] pending_save,
                                        async move {
                                            let _ = window.perform_autosave().await;
                                            pending_save.set(false);
                                        }
                                    });
                                }

                                // Clear the stored source_id
                                window.imp().autosave_debounce_id.replace(None);

                                // Trim unallocated memory at regular intervals.
                                #[cfg(target_os = "linux")]
                                unsafe extern "C" {
                                    fn malloc_trim(pad: usize) -> i32;
                                }
                                #[cfg(target_os = "linux")]
                                unsafe {
                                    malloc_trim(0);
                                }
                            }
                        },
                    );

                    imp.autosave_debounce_id.replace(Some(source_id));
                }
            }
        });

        imp.signal_handlers
            .borrow_mut()
            .push((buffer.clone().upcast(), autosave_handler_id));
    }

    async fn perform_autosave(&self) -> Result<(), gtk::glib::Error> {
        if let Some(path) = self.current_file_path() {
            match self
                .save_file(
                    self.imp().editor_container.buffer(),
                    Some(gio::File::for_path(&path)),
                )
                .await
            {
                Ok(Some(saved_file)) => {
                    let project_root = self.project_root().map(|p| gio::File::for_path(&p));
                    file_io::recent::add_recent_file(&saved_file, project_root.as_ref());
                    Ok(())
                }
                Ok(None) => Ok(()),
                Err(e) => {
                    self.show_toast(&format!("Failed to save file: {}", e), None);
                    Err(e)
                }
            }
        } else {
            Ok(())
        }
    }

    fn initialize_preview_jump(&self) {
        let settings = self.settings();
        let imp = self.imp();

        let handler_id = settings.connect_changed(
            Some("scroll-preview-on-type"),
            gtk::glib::clone! {
                #[weak(rename_to = window)] self,
                move |settings, _| {
                    window.handle_preview_jump_setting_change(settings);
                }
            },
        );
        imp.signal_handlers
            .borrow_mut()
            .push((settings.clone().upcast(), handler_id));

        if settings.boolean("scroll-preview-on-type") {
            self.setup_preview_jump();
        }
    }

    fn handle_preview_jump_setting_change(&self, settings: &gio::Settings) {
        let should_be_enabled = settings.boolean("scroll-preview-on-type");
        let imp = self.imp();

        // Check if we have a preview jump handler stored
        let is_currently_enabled = imp.signal_handlers.borrow().iter().any(|(obj, _)| {
            obj.downcast_ref::<sourceview::Buffer>()
                .map(|buf| {
                    let editor_buffer = imp.editor_container.buffer();
                    buf == editor_buffer
                })
                .unwrap_or(false)
        });

        if should_be_enabled && !is_currently_enabled {
            self.setup_preview_jump();
        } else if !should_be_enabled && is_currently_enabled {
            self.cleanup_preview_jump();
        }
    }

    fn setup_preview_jump(&self) {
        let buffer = self.imp().editor_container.get().buffer().clone();
        let window_weak = self.downgrade();
        let cooldown = PREVIEW_JUMP_COOLDOWN;

        let handler_id = buffer.connect_changed(move |_| {
            if let Some(window) = window_weak.upgrade() {
                let imp = window.imp();
                let now = Instant::now();
                let mut next_allowed = imp.next_allowed_jump.borrow_mut();

                let can_fire = next_allowed.is_none_or(|allowed| now >= allowed);

                if can_fire {
                    *next_allowed = Some(now + cooldown);
                    drop(next_allowed);
                    window.perform_preview_jump();

                    if let Some(id) = imp.pending_trailing.borrow_mut().take() {
                        id.remove();
                    }
                } else if imp.pending_trailing.borrow().is_none() {
                    let delay = next_allowed.unwrap() - now;
                    drop(next_allowed);
                    let source_id = gtk::glib::timeout_add_local_once(
                        delay,
                        glib::clone!(
                            #[weak(rename_to = w)]
                            window,
                            move || {
                                let imp = w.imp();
                                *imp.next_allowed_jump.borrow_mut() =
                                    Some(Instant::now() + cooldown);
                                w.perform_preview_jump();
                                *imp.pending_trailing.borrow_mut() = None;
                            }
                        ),
                    );
                    *imp.pending_trailing.borrow_mut() = Some(source_id);
                }
            }
        });

        self.imp()
            .signal_handlers
            .borrow_mut()
            .push((buffer.upcast(), handler_id));
    }

    fn perform_preview_jump(&self) {
        let window_weak = self.downgrade();

        gtk::glib::spawn_future_local(async move {
            if let Some(w) = window_weak.upgrade() {
                let imp = w.imp();
                let offset = imp.editor_container.cursor_byte_offset();
                if let Ok(compiler_system) = w.compiler_system().await
                    && let Ok(crate::typst_system::JumpResult::Preview(locs)) =
                        compiler_system.jump_from_source(offset).await
                    && let Some(loc) = locs.first()
                {
                    #[cfg(debug_assertions)]
                    println!(
                        "WINDOW: Jump to preview: location {} on page {}",
                        loc.y, loc.page_number
                    );
                    w.imp().preview_container.scroll_to_preview_location(loc);
                }
            }
        });
    }

    fn cleanup_preview_jump(&self) {
        let imp = self.imp();
        let editor = imp.editor_container.get();
        let buffer = editor.buffer();

        // Extract all buffer-related handlers
        let mut all_handlers = imp.signal_handlers.borrow_mut();
        let remaining: Vec<_> = all_handlers
            .drain(..)
            .filter_map(|(obj, handler_id)| {
                if let Some(buf) = obj.downcast_ref::<sourceview::Buffer>()
                    && buf == buffer
                {
                    // Disconnect this handler
                    buffer.disconnect(handler_id);
                    return None; // Don't keep this handler
                }
                Some((obj, handler_id)) // Keep other handlers
            })
            .collect();

        // Put back the non-buffer handlers
        *all_handlers = remaining;
        drop(all_handlers);

        if let Some(source_id) = imp.pending_trailing.borrow_mut().take() {
            source_id.remove();
        }

        // Clear the next allowed jump time
        *imp.next_allowed_jump.borrow_mut() = None;
    }

    fn initialize_fullscreen_mode(&self) {
        let imp = self.imp();

        // Set up motion controller for mouse tracking
        let motion_controller = gtk::EventControllerMotion::new();
        motion_controller.set_propagation_phase(gtk::PropagationPhase::Capture);

        let window_weak = self.downgrade();
        motion_controller.connect_motion(move |_, _x, y| {
            if let Some(window) = window_weak.upgrade()
                && window.is_fullscreen()
            {
                let imp = window.imp();
                imp.is_touch.set(false);
                imp.last_mouse_y.set(y);
                window.update_fullscreen_ui();
            }
        });

        let window_weak = self.downgrade();
        motion_controller.connect_enter(move |_, _x, y| {
            if let Some(window) = window_weak.upgrade()
                && window.is_fullscreen()
            {
                let imp = window.imp();
                imp.is_touch.set(false);
                imp.last_mouse_y.set(y);
                window.update_fullscreen_ui();
            }
        });

        self.add_controller(motion_controller.clone());
        let _ = imp.motion_controller.set(motion_controller);

        // Set up touch gesture for touch input
        let touch_gesture = gtk::GestureClick::new();
        touch_gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        touch_gesture.set_touch_only(true);

        let window_weak = self.downgrade();
        touch_gesture.connect_pressed(move |gesture, _n_press, _x, y| {
            gesture.set_state(gtk::EventSequenceState::Denied);

            if let Some(window) = window_weak.upgrade()
                && window.is_fullscreen()
            {
                let imp = window.imp();
                imp.is_touch.set(true);

                if y > window.get_titlebar_area_height() {
                    window.update_fullscreen_ui();
                }
            }
        });

        self.add_controller(touch_gesture.clone());
        let _ = imp.touch_gesture.set(touch_gesture);

        // Track focus changes to show toolbar when headerbar widgets are focused
        let handler_id = self.connect_notify_local(Some("focus-widget"), |window, _| {
            if window.is_fullscreen() {
                window.update_fullscreen_ui();
            }
        });
        imp.signal_handlers
            .borrow_mut()
            .push((self.clone().upcast(), handler_id));

        // Handle fullscreen state changes
        let handler_id = self.connect_notify_local(Some("fullscreened"), move |window, _| {
            let imp = window.imp();
            let is_fullscreen = window.is_fullscreen();

            imp.toolbar_view
                .set_extend_content_to_top_edge(is_fullscreen);

            let editor = imp.editor_container.get();
            let view = editor.view();

            if is_fullscreen {
                imp.toolbar_view.set_reveal_top_bars(false);
                imp.last_mouse_y.set(0.0);
                imp.is_touch.set(false);
                view.set_top_margin(FULLSCREEN_MARGIN);
            } else {
                imp.toolbar_view.set_reveal_top_bars(true);
                imp.last_mouse_y.set(0.0);
                imp.is_touch.set(false);
                view.set_top_margin(NORMAL_MARGIN);
            }
        });
        imp.signal_handlers
            .borrow_mut()
            .push((self.clone().upcast(), handler_id));
    }

    fn get_titlebar_area_height(&self) -> f64 {
        let top_bar_height = self.imp().toolbar_view.top_bar_height() as f64;
        top_bar_height.max(SHOW_HEADERBAR_DISTANCE_PX)
    }

    fn is_focusing_headerbar(&self) -> bool {
        let window = self.upcast_ref::<gtk::Window>();
        let Some(focus_widget) = gtk::prelude::GtkWindowExt::focus(window) else {
            return false;
        };

        // Check if the focused widget is a descendant of the content area
        // If it is, then it's not in the headerbar
        let content_area = self.imp().toast_overlay.upcast_ref::<gtk::Widget>();
        let is_in_content = Self::is_descendant_of(&focus_widget, content_area);

        if is_in_content {
            return false;
        }

        // If it's not in the content area but is in the toolbar view,
        // then it must be in a top or bottom bar
        let toolbar_view = &*self.imp().toolbar_view;

        Self::is_descendant_of(&focus_widget, toolbar_view.upcast_ref::<gtk::Widget>())
    }

    fn is_descendant_of(widget: &gtk::Widget, target: &gtk::Widget) -> bool {
        if widget == target {
            return true;
        }

        let mut parent = widget.parent();
        while let Some(p) = parent {
            if &p == target {
                return true;
            }
            parent = p.parent();
        }

        false
    }

    fn update_fullscreen_ui(&self) {
        if !self.is_fullscreen() {
            return;
        }

        let imp = self.imp();

        // If mouse is at top edge (and not touch), show UI
        if !imp.is_touch.get() && imp.last_mouse_y.get() <= self.get_titlebar_area_height() {
            self.show_toolbar();
            return;
        }

        // If a headerbar widget has focus, show UI
        if self.is_focusing_headerbar() {
            self.show_toolbar();
            return;
        }

        // Otherwise, hide
        self.hide_toolbar();
    }

    fn show_toolbar(&self) {
        let imp = self.imp();
        imp.toolbar_view.set_reveal_top_bars(true);
    }

    fn hide_toolbar(&self) {
        let imp = self.imp();

        if !self.is_fullscreen() {
            return;
        }

        imp.toolbar_view.set_reveal_top_bars(false);

        // Grab focus to toolbar_view to ensure no headerbar widget keeps focus
        imp.toolbar_view.grab_focus();
    }

    fn initialize_magnifier_key_handler(&self) {
        // Set up key event handler at window level to catch all keypresses
        let key_controller = gtk::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);

        let window_weak = self.downgrade();
        key_controller.connect_key_pressed(move |_ctrl, _keyval, _keycode, _state| {
            if let Some(window) = window_weak.upgrade() {
                let imp = window.imp();

                // Check if magnifier is active in preview
                if imp.preview_container.imp().magnifier.borrow().is_some() {
                    // If active, deactivate it
                    imp.preview_container.deactivate_magnifier();
                }
            }

            // Always let the key event continue to be handled normally
            glib::Propagation::Proceed
        });

        self.add_controller(key_controller.clone());
        let _ = self.imp().magnifier_key_controller.set(key_controller);
    }

    fn initialize_main_menu_widgets(&self) {
        let settings = self.settings();
        let imp = self.imp();

        let theme_selector = ThemeSelector::new(settings);
        theme_selector.setup();
        let theme_selector_widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        theme_selector_widget.append(&theme_selector);

        let menu_button = &imp.main_menu_button;
        if let Some(popover) = menu_button.popover()
            && let Ok(popover_menu) = popover.downcast::<gtk::PopoverMenu>()
        {
            popover_menu.set_child_visible(true);
            popover_menu.add_child(&theme_selector_widget, "theme-selector-widget");
        }

        // Store the theme selector for cleanup
        *imp.theme_selector.borrow_mut() = Some(theme_selector);
    }

    fn initialize_recent_files_popover(&self) {
        let imp = self.imp();
        let recent_files_popover = recents_menu::TypesetterRecentFilesPopover::new();
        imp.open_file_button
            .set_popover(Some(&recent_files_popover));

        // Reset the split button's hover/pressed state once the popover closes.
        recent_files_popover.connect_closed(clone! {
            #[weak(rename_to = window)] self,
            move |_| {
                let button = window.imp().open_file_button.get().upcast::<gtk::Widget>();
                glib::idle_add_local_once(move || {
                    clear_stale_hover(&button);
                });
            }
        });

        // Store the popover for cleanup
        *imp.recent_files_popover.borrow_mut() = Some(recent_files_popover);
    }

    fn initialize_templates_popover(&self) {
        let imp = self.imp();
        let templates_popover = templates_menu::TypesetterTemplatesPopover::new();

        let window_weak = self.downgrade();
        templates_popover.connect_closure(
            "template-activated",
            false,
            glib::closure_local!(move |_popover: templates_menu::TypesetterTemplatesPopover,
                                       path: String| {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                window.open_template(path);
            }),
        );

        // Reset the split button's hover/pressed state once the popover closes.
        templates_popover.connect_closed(clone! {
            #[weak(rename_to = window)] self,
            move |_| {
                let button = window.imp().new_file_button.get().upcast::<gtk::Widget>();
                glib::idle_add_local_once(move || {
                    clear_stale_hover(&button);
                });
            }
        });

        imp.new_file_button.set_popover(Some(&templates_popover));

        // Store the popover for cleanup
        *imp.templates_popover.borrow_mut() = Some(templates_popover);
    }

    fn open_template(&self, entrypoint_path: String) {
        let Ok(content) = std::fs::read_to_string(&entrypoint_path) else {
            eprintln!("Templates: Failed to read entrypoint: {}", entrypoint_path);
            return;
        };

        let is_unsaved = self.current_file_is_unsaved();
        let current_path: Option<PathBuf> = self.current_file_path();

        let target_window = if !is_unsaved && current_path.is_none() {
            // Pristine window — load into self
            self.clone()
        } else {
            // Window has content — open a new one
            let Some(app) = self.application() else {
                return;
            };
            let window = Self::new(&app, None);
            window.present();
            window
        };

        let buffer = target_window.imp().editor_container.buffer();
        buffer.begin_user_action();
        buffer.delete(&mut buffer.start_iter(), &mut buffer.end_iter());
        buffer.insert(&mut buffer.start_iter(), &content);
        buffer.end_user_action();
        buffer.place_cursor(&buffer.start_iter());

        let text = target_window.imp().editor_container.contents();
        target_window.imp().preview_container.update_preview(&text);

        target_window.present();
    }

    fn initialize_unsaved_state_tracking(&self) {
        let imp = self.imp();
        let editor = imp.editor_container.get();
        let buffer = editor.buffer();
        let window_weak = self.downgrade();

        let handler_id = buffer.connect_changed(move |_| {
            if let Some(window) = window_weak.upgrade()
                && !window.current_file_is_unsaved()
            {
                window.set_current_file_is_unsaved(true);
            }
        });

        imp.signal_handlers
            .borrow_mut()
            .push((buffer.clone().upcast(), handler_id));
    }

    fn initialize_dependency_monitor(&self) {
        let window_weak = self.downgrade();
        let monitor = crate::file_io::monitoring::DependencyMonitor::new(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let imp = window.imp();
            let text = imp.editor_container.contents();
            imp.preview_container.update_preview(&text);
        });
        self.imp().dependency_monitor.borrow_mut().replace(monitor);
    }

    // Register the completion provider
    fn initialize_autocomplete(&self) {
        let editor = self.imp().editor_container.get();
        editor.initialize_completion();
    }

    // Register the hover provider
    fn initialize_hover(&self) {
        let editor = self.imp().editor_container.get();
        editor.initialize_hover();
    }

    fn setup_project_folder_banner(&self) {
        let imp = self.imp();
        let banner = imp.project_folder_banner.get();

        let handler_id = banner.connect_button_clicked(clone! {
            #[weak(rename_to = window)] self,
            move |_| {
                gtk::glib::MainContext::default().spawn_local(async move {
                    if let Some(folder) = window.prompt_select_project_folder().await {
                        let imp = window.imp();

                        // Set project root
                        window.set_project_root(folder);

                        // Hide banner
                        imp.project_folder_banner.set_revealed(false);

                        // Trigger preview update
                        let editor = imp.editor_container.get();
                        let text = editor.contents();
                        imp.preview_container.update_preview(&text);
                    }
                });
            }
        });

        imp.signal_handlers
            .borrow_mut()
            .push((banner.clone().upcast(), handler_id));
    }

    async fn prompt_select_project_folder(&self) -> Option<PathBuf> {
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Select Folder"))
            .modal(true)
            .accept_label(gettext("Select"))
            .build();

        match dialog.select_folder_future(Some(self)).await {
            Ok(file) => {
                let path = file.path()?;
                Some(path)
            }
            Err(_) => None,
        }
    }

    /// In the Flatpak sandbox, we can't search the filesystem, so we rely on stored metadata.
    fn detect_project_root(file_path: &Path) -> Option<PathBuf> {
        let file_uri = gio::File::for_path(file_path).uri();
        let recent_files = file_io::recent::load_recent_files();
        recent_files
            .into_iter()
            .find(|recent| recent.uri == file_uri)
            .and_then(|recent| recent.project_root_path())
    }

    // Update the stored working file path, resolve the real file path, and set the unsaved state
    fn set_file_state(&self, path: impl AsRef<Path>, is_unsaved: bool) {
        let path = path.as_ref();

        // Update the file path
        self.set_current_file_path(path);

        // Update the directory display
        if let Some(dir) = utils::get_display_directory(path) {
            self.set_display_file_directory(Some(dir.as_str()));
        } else {
            self.set_display_file_directory(None::<&str>);
        }

        // Update the unsaved state
        self.set_current_file_is_unsaved(is_unsaved);
    }

    pub fn start_progress_pulse(&self) {
        let imp = self.imp();
        if imp.pulse_timeout_id.borrow().is_some() {
            return;
        }

        imp.progress_bar.set_fraction(0.0);
        imp.progress_bar.set_visible(true);

        let source_id = gtk::glib::timeout_add_local(
            PROGRESS_PULSE_INTERVAL,
            clone! {
                #[weak(rename_to = window)] self,
                #[upgrade_or] gtk::glib::ControlFlow::Break,
                move || {
                    window.imp().progress_bar.pulse();
                    gtk::glib::ControlFlow::Continue
                }
            },
        );

        imp.pulse_timeout_id.replace(Some(source_id));
    }

    pub fn stop_progress_pulse(&self) {
        let imp = self.imp();

        if let Some(source_id) = imp.pulse_timeout_id.borrow_mut().take() {
            source_id.remove();
        }

        imp.progress_bar.set_visible(false);
    }

    fn restore_preview_state(&self) {
        let settings = self.settings();
        let show_preview = settings.boolean("show-preview");
        let imp = self.imp();

        imp.preview_button.set_active(show_preview);
        self.update_preview_layout();
    }

    fn save_preview_state(&self, show: bool) {
        let settings = self.settings();
        let _ = settings.set_boolean("show-preview", show);
    }

    fn setup_preview_paned(&self) {
        let imp = self.imp();

        // 1. When preview toggle button changes state
        let preview_button_handler = imp.preview_button.connect_active_notify(clone! {
            #[weak(rename_to = window)] self,
            move |btn| {
                let active = btn.is_active();
                window.save_preview_state(active);
                window.update_preview_layout();
            }
        });

        // 2. When narrow-window breakpoint changes (responsive behavior)
        let breakpoint_handler = self.connect_current_breakpoint_notify(clone! {
            #[weak(rename_to = window)] self,
            move |_| {
                window.update_preview_layout();
            }
        });

        // 3. When paned divider moves (persisting position with debounce)
        let paned_pos_handler = imp.split_view.connect_position_notify(clone! {
            #[weak(rename_to = window)] self,
            move |paned| {
                let imp = window.imp();
                // Only persist when in wide mode with both panes visible
                if window.current_breakpoint().is_none()
                    && imp.preview_button.is_active()
                    && imp.editor_container.is_visible()
                    && imp.preview_container.is_visible()
                {
                    let pos = paned.position();
                    if pos >= MIN_EDITOR_WIDTH {
                        imp.saved_divider_position.set(pos);

                        if let Some(source_id) = imp.divider_debounce_id.borrow_mut().take() {
                            source_id.remove();
                        }

                        let window_weak = window.downgrade();
                        let source_id = gtk::glib::timeout_add_local_once(
                            DIVIDER_DEBOUNCE_DELAY,
                            move || {
                                if let Some(w) = window_weak.upgrade() {
                                    let _ = w.imp().divider_debounce_id.borrow_mut().take();
                                    let p = w.imp().saved_divider_position.get();
                                    if p > 0 {
                                        let settings = w.settings();
                                        let _ = settings.set_int("preview-divider-position", p);
                                    }
                                }
                            },
                        );
                        *imp.divider_debounce_id.borrow_mut() = Some(source_id);
                    }
                }
            }
        });

        // 4. When paned max-position changes (e.g. window resized while open)
        let paned_max_pos_handler = imp.split_view.connect_max_position_notify(clone! {
            #[weak(rename_to = window)] self,
            move |paned| {
                if window.current_breakpoint().is_none()
                    && window.imp().preview_button.is_active()
                    && window.imp().preview_container.is_visible()
                {
                    let max = paned.max_position();
                    if max > MIN_EDITOR_WIDTH && paned.position() > max {
                        paned.set_position(max);
                    }
                }
            }
        });

        // 5. Restore divider position when paned is first mapped
        let paned_map_handler = imp.split_view.connect_map(clone! {
            #[weak(rename_to = window)] self,
            move |_| {
                if window.current_breakpoint().is_none()
                    && window.imp().preview_button.is_active()
                {
                    window.restore_divider_position();
                }
            }
        });

        imp.signal_handlers.borrow_mut().extend([
            (imp.preview_button.clone().upcast(), preview_button_handler),
            (self.clone().upcast(), breakpoint_handler),
            (imp.split_view.clone().upcast(), paned_pos_handler),
            (imp.split_view.clone().upcast(), paned_max_pos_handler),
            (imp.split_view.clone().upcast(), paned_map_handler),
        ]);
    }

    fn clamp_divider_position(&self, pos: i32) -> i32 {
        let imp = self.imp();
        let total_width = imp.split_view.width();
        let available_width = if total_width > 0 {
            total_width
        } else {
            self.default_width()
        };

        calculate_clamped_divider_position(
            pos,
            available_width,
            MIN_EDITOR_WIDTH,
            MIN_PREVIEW_WIDTH,
        )
    }

    fn restore_divider_position(&self) {
        let imp = self.imp();
        let mut pos = imp.saved_divider_position.get();
        if pos <= 0 {
            let settings = self.settings();
            pos = settings.int("preview-divider-position");
            imp.saved_divider_position.set(pos);
        }
        let clamped = self.clamp_divider_position(pos);
        imp.split_view.set_position(clamped);
    }

    fn update_preview_layout(&self) {
        let imp = self.imp();
        let is_preview_active = imp.preview_button.is_active();
        let is_narrow = self.current_breakpoint().is_some();

        if is_narrow {
            // Narrow window mode: toggle between full editor and full preview
            if is_preview_active {
                imp.editor_container.set_visible(false);
                imp.preview_container.set_visible(true);
                imp.preview_container.set_preview_visible(true);
            } else {
                imp.editor_container.set_visible(true);
                imp.preview_container.set_visible(false);
                imp.preview_container.set_preview_visible(false);
            }
        } else {
            // Wide window mode: side-by-side with draggable divider
            imp.editor_container.set_visible(true);
            if is_preview_active {
                imp.preview_container.set_visible(true);
                imp.preview_container.set_preview_visible(true);
                self.restore_divider_position();
            } else {
                imp.preview_container.set_visible(false);
                imp.preview_container.set_preview_visible(false);
            }
        }
    }

    fn setup_explorer(&self) {
        let imp = self.imp();
        let explorer = imp.explorer_container.get();
        let split_view = imp.explorer_split_view.get();
        let button = imp.explorer_button.get();

        // Bind button active state to split_view show-sidebar bidirectionally
        button
            .bind_property("active", &split_view, "show-sidebar")
            .bidirectional()
            .sync_create()
            .build();

        // Restore and save explorer open state in GSettings
        let settings = self.settings();
        let show_explorer = settings.boolean("show-explorer");
        button.set_active(show_explorer);

        let settings_clone = settings.clone();
        let handler_id = button.connect_active_notify(move |btn| {
            let _ = settings_clone.set_boolean("show-explorer", btn.is_active());
        });
        imp.signal_handlers
            .borrow_mut()
            .push((button.clone().upcast(), handler_id));

        // When project root changes, update explorer root
        let handler_id = self.connect_notify_local(
            Some("project-root"),
            clone! {
                #[weak(rename_to = window)] self,
                move |_, _| {
                    window.update_explorer_root();
                }
            },
        );
        imp.signal_handlers
            .borrow_mut()
            .push((self.clone().upcast(), handler_id));

        // When current file changes, update explorer root if project root is not set
        let handler_id = self.connect_notify_local(
            Some("current-file-path"),
            clone! {
                #[weak(rename_to = window)] self,
                move |_, _| {
                    window.update_explorer_root();
                }
            },
        );
        imp.signal_handlers
            .borrow_mut()
            .push((self.clone().upcast(), handler_id));

        // Initial explorer root
        self.update_explorer_root();

        // Connect file activation in explorer
        let handler_id = explorer.connect_file_activated(clone! {
            #[weak(rename_to = window)] self,
            move |_, file| {
                let file = file.clone();
                gtk::glib::MainContext::default().spawn_local(async move {
                    window.open_file_from_explorer(&file).await;
                });
            }
        });
        imp.signal_handlers
            .borrow_mut()
            .push((explorer.clone().upcast(), handler_id));

        // Connect open folder request in explorer
        let handler_id = explorer.connect_open_folder_requested(clone! {
            #[weak(rename_to = window)] self,
            move |_| {
                gtk::glib::MainContext::default().spawn_local(async move {
                    if let Some(folder) = window.prompt_select_project_folder().await {
                        window.set_project_root(folder);
                        let editor = window.imp().editor_container.get();
                        let text = editor.contents();
                        window.imp().preview_container.update_preview(&text);
                    }
                });
            }
        });
        imp.signal_handlers
            .borrow_mut()
            .push((explorer.clone().upcast(), handler_id));
    }

    fn update_explorer_root(&self) {
        let root = self.project_root().or_else(|| {
            self.current_file_path().and_then(|p| {
                if p.is_dir() {
                    Some(p)
                } else {
                    p.parent().map(|p| p.to_path_buf())
                }
            })
        });
        self.imp().explorer_container.set_root_path(root.as_deref());
    }

    async fn confirm_unsaved_changes(&self) -> bool {
        if !self.current_file_is_unsaved() {
            return true;
        }

        let autosave_enabled = self
            .imp()
            .settings
            .get()
            .map(|settings| settings.boolean("autosave"))
            .unwrap_or(false);

        if let Some(path) = self.current_file_path()
            && autosave_enabled
        {
            match self
                .save_file(
                    self.imp().editor_container.buffer(),
                    Some(gio::File::for_path(&path)),
                )
                .await
            {
                Ok(Some(saved_file)) => {
                    let project_root = self.project_root().map(|p| gio::File::for_path(&p));
                    file_io::recent::add_recent_file(&saved_file, project_root.as_ref());
                    return true;
                }
                Ok(None) => return false,
                Err(_) => {
                    self.show_toast(&gettext("Failed to save file."), None);
                    return false;
                }
            }
        }

        let dialog = adw::AlertDialog::new(
            Some(&gettext("Unsaved Changes")),
            Some(&gettext(
                "The document has been modified. Do you want to save your changes before opening another file?",
            )),
        );

        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("discard", &gettext("Discard"));
        dialog.add_response("save", &gettext("Save"));
        dialog.set_close_response("cancel");
        dialog.set_default_response(Some("save"));
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);

        let response = dialog.choose_future(Some(self)).await;
        match response.as_str() {
            "save" => {
                match self
                    .save_file(
                        self.imp().editor_container.buffer(),
                        self.current_file_path().map(|p| gio::File::for_path(&p)),
                    )
                    .await
                {
                    Ok(Some(saved_file)) => {
                        let project_root = self.project_root().map(|p| gio::File::for_path(&p));
                        file_io::recent::add_recent_file(&saved_file, project_root.as_ref());
                        true
                    }
                    _ => false,
                }
            }
            "discard" => true,
            _ => false,
        }
    }

    async fn open_file_from_explorer(&self, file: &gio::File) {
        let t_start = std::time::Instant::now();
        let Some(open_path) = file.path() else {
            return;
        };

        let is_typ = open_path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("typ"))
            .unwrap_or(false);

        if !is_typ {
            let uri = file.uri();
            let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
            return;
        }

        if let Some(current) = self.current_file_path()
            && current == open_path
        {
            self.imp().editor_container.view().grab_focus();
            return;
        }

        if let Some(app) = self.application()
            && let Some(other_window) = file_io::open::find_window_with_file(&app, file)
        {
            other_window.present();
            return;
        }

        if !self.confirm_unsaved_changes().await {
            return;
        }

        let buffer = self.imp().editor_container.buffer();
        match file_io::open::load_file(buffer, file).await {
            Ok(()) => {
                // Cancel pending debounce timer triggered by buffer changed signal during load_file
                self.imp().editor_container.cancel_debounce();

                // Clear previous document preview pages and textures so stale pages are never shown
                self.imp().preview_container.reset();

                let start_iter = buffer.start_iter();
                buffer.place_cursor(&start_iter);
                self.imp().editor_container.view().grab_focus();

                self.set_file_state(&open_path, false);

                if self.project_root().is_none()
                    && let Some(detected) = Self::detect_project_root(&open_path)
                {
                    self.set_project_root(detected);
                }

                let t_reset = std::time::Instant::now();
                if let Ok(compiler) = self.compiler_system().await {
                    compiler.reset_world();
                }
                let reset_duration = t_reset.elapsed();

                let text = self.imp().editor_container.contents();
                self.imp().preview_container.update_preview(&text);
                self.imp().editor_container.lint_full_document();

                let project_root = self.project_root().map(|p| gio::File::for_path(&p));
                file_io::recent::add_recent_file(file, project_root.as_ref());

                #[cfg(debug_assertions)]
                println!(
                    "[open_file_from_explorer] File switch completed in {:?} (world reset: {:?})",
                    t_start.elapsed(),
                    reset_duration
                );
            }
            Err(err) => {
                self.show_toast(
                    &format!("{}{}", gettext("Unable to open file: "), err),
                    None,
                );
            }
        }
    }

    pub fn new_file(&self) {
        if let Some(app) = self.application() {
            Self::new(&app, None).present();
        }
    }

    /// Update the preview button badge based on error/warning count
    /// Shows a red dot for errors, yellow dot for warnings, hidden if none
    pub fn update_preview_error_badge(&self, error_count: u32, warning_count: u32) {
        let imp = self.imp();
        let badge = &*imp.preview_error_badge;

        if error_count > 0 {
            badge.remove_css_class("warning");
            badge.add_css_class("error");
            badge.set_visible(true);
        } else if warning_count > 0 {
            badge.remove_css_class("error");
            badge.add_css_class("warning");
            badge.set_visible(true);
        } else {
            badge.set_visible(false);
        }
    }

    async fn export_html_file(
        &self,
        content: String,
        source_path: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Result<Option<PathBuf>, glib::Error> {
        if let Some(export_path) =
            file_io::export::prompt_export_html_path(self, source_path.clone()).await
        {
            self.start_progress_pulse();
            let result = file_io::export::export_html(
                self,
                content,
                source_path,
                project_root,
                export_path.clone(),
            )
            .await;
            self.stop_progress_pulse();
            result.map(|_| Some(export_path))
        } else {
            Ok(None)
        }
    }

    async fn export_markdown_file(
        &self,
        content: String,
        source_path: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Result<Option<PathBuf>, glib::Error> {
        if let Some(export_path) =
            file_io::export::prompt_export_markdown_path(self, source_path.clone()).await
        {
            self.start_progress_pulse();
            let result = file_io::export::export_markdown(
                self,
                content,
                source_path,
                project_root,
                export_path.clone(),
            )
            .await;
            self.stop_progress_pulse();
            result.map(|_| Some(export_path))
        } else {
            Ok(None)
        }
    }

    async fn export_pdf_file(
        &self,
        content: String,
        source_path: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Result<Option<PathBuf>, glib::Error> {
        if let Some(export_path) =
            file_io::export::prompt_export_pdf_path(self, source_path.clone()).await
        {
            let export_standard = self.settings().string("pdf-standard").to_string();
            self.start_progress_pulse();
            let result = file_io::export::export_pdf(
                self,
                content,
                source_path,
                project_root,
                export_path.clone(),
                export_standard,
            )
            .await;
            self.stop_progress_pulse();
            result.map(|_| Some(export_path))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn open_file(
        &self,
        buffer: &sourceview::Buffer,
        file: Option<gio::File>,
    ) -> Result<Option<gio::File>, glib::Error> {
        let open_file = match file {
            Some(f) => f,
            None => match crate::file_io::open::prompt_open_file(self).await {
                Some(f) => f,
                None => return Ok(None),
            },
        };

        let open_path = open_file.path();

        if let (Some(app), Some(_path)) = (self.application(), &open_path)
            && let Some(existing_window) =
                crate::file_io::open::find_window_with_file(&app, &open_file)
        {
            existing_window.present();
            return Ok(None);
        }

        let is_unsaved: bool = self.property("current-file-is-unsaved");
        let current_path: Option<PathBuf> = self.property("current-file-path");

        if !is_unsaved && current_path.is_none() {
            crate::file_io::open::load_file(buffer, &open_file).await?;

            let start_iter = buffer.start_iter();
            buffer.place_cursor(&start_iter);
            self.imp().editor_container.view().grab_focus();

            // Detect and set project root
            if let Some(ref path) = open_path {
                if let Some(project_root) = Self::detect_project_root(path) {
                    self.set_project_root(project_root);
                }
                self.set_file_state(path, false);
            }

            self.imp().editor_container.lint_full_document();

            Ok(Some(open_file))
        } else if let Some(app) = self.application() {
            Self::new(&app, Some(open_file)).present();
            Ok(None)
        } else {
            Ok(None)
        }
    }

    async fn save_file(
        &self,
        buffer: &sourceview::Buffer,
        file: Option<gio::File>,
    ) -> Result<Option<gio::File>, glib::Error> {
        let save_file = match file {
            Some(f) => f,
            None => match file_io::save::prompt_save_file(self, self.current_file_path()).await {
                Some(f) => f,
                None => return Ok(None),
            },
        };

        let saved_file = file_io::save::write_file(buffer, save_file.clone()).await?;

        if let Some(saved_path) = saved_file.path() {
            self.set_file_state(&saved_path, false);
        }

        Ok(Some(save_file))
    }

    fn handle_close_request(&self) -> gtk::glib::Propagation {
        let imp = self.imp();

        if let Some(source_id) = imp.divider_debounce_id.borrow_mut().take() {
            source_id.remove();
            let p = imp.saved_divider_position.get();
            if p > 0 {
                let _ = self.settings().set_int("preview-divider-position", p);
            }
        }

        if imp.force_close.get() || !self.current_file_is_unsaved() {
            if let Some(compiler) = self.imp().compiler_system.get() {
                compiler.cancel_all();
            }
            imp.preview_container.cleanup();
            return gtk::glib::Propagation::Proceed;
        }

        let autosave_enabled = imp
            .settings
            .get()
            .map(|settings| settings.boolean("autosave"))
            .unwrap_or(false);

        let window_weak = self.downgrade();
        gtk::glib::MainContext::default().spawn_local(async move {
            let Some(window) = window_weak.upgrade() else {
                return;
            };

            if let Some(path) = window.current_file_path()
                && autosave_enabled
            {
                match window
                    .save_file(
                        window.imp().editor_container.buffer(),
                        Some(gio::File::for_path(&path)),
                    )
                    .await
                {
                    Ok(Some(saved_file)) => {
                        let project_root = window.project_root().map(|p| gio::File::for_path(&p));
                        file_io::recent::add_recent_file(&saved_file, project_root.as_ref());
                        window.imp().force_close.set(true);
                        window.close();
                    }
                    Ok(None) => {}
                    Err(_) => {
                        window.show_toast(&gettext("Failed to save file."), None);
                    }
                }
                return;
            }

            window.show_unsaved_changes_dialog().await;
        });

        gtk::glib::Propagation::Stop
    }

    async fn show_unsaved_changes_dialog(&self) {
        let dialog = adw::AlertDialog::new(
            Some(&gettext("Unsaved Changes")),
            Some(&gettext(
                "The document has been modified. Do you want to save your changes before closing?",
            )),
        );

        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("discard", &gettext("Discard"));
        dialog.add_response("save", &gettext("Save"));
        dialog.set_close_response("cancel");
        dialog.set_default_response(Some("save"));
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);

        let response = dialog.choose_future(Some(self)).await;
        match response.as_str() {
            "save" => {
                match self
                    .save_file(
                        self.imp().editor_container.buffer(),
                        self.current_file_path().map(|p| gio::File::for_path(&p)),
                    )
                    .await
                {
                    Ok(Some(saved_file)) => {
                        let project_root = self.project_root().map(|p| gio::File::for_path(&p));
                        file_io::recent::add_recent_file(&saved_file, project_root.as_ref());
                        self.imp().force_close.set(true);
                        self.close();
                    }
                    Ok(None) => {}
                    Err(_) => {
                        self.show_toast(&gettext("Failed to save file."), None);
                    }
                }
            }
            "discard" => {
                self.imp().force_close.set(true);
                self.close();
            }
            _ => {}
        }
    }

    /// Show a toast notification with a message and an optional button to open a file by URI.
    pub fn show_toast(&self, message: &str, open_path: Option<&PathBuf>) {
        let toast = adw::Toast::new(message);
        toast.set_timeout(TOAST_TIMEOUT);
        if let Some(path) = open_path {
            toast.set_button_label(Some(&gettext("Open")));
            let file = gio::File::for_path(path);
            let window = self.clone();
            toast.connect_button_clicked(move |_| {
                let launcher = gtk::FileLauncher::new(Some(&file));
                launcher.launch(
                    Some(window.upcast_ref::<gtk::Window>()),
                    None::<&gio::Cancellable>,
                    |result| {
                        if let Err(err) = result {
                            eprintln!("Failed to open file: {err}");
                        }
                    },
                );
            });
        }
        self.imp().toast_overlay.add_toast(toast);
    }
}

/// Calculate a clamped divider position ensuring both panes adhere to minimum widths.
pub fn calculate_clamped_divider_position(
    pos: i32,
    available_width: i32,
    min_editor: i32,
    min_preview: i32,
) -> i32 {
    let max_pos = (available_width - min_preview).max(min_editor);
    let min_pos = min_editor;

    if max_pos <= min_pos {
        min_pos
    } else if pos <= 0 {
        (available_width / 2).clamp(min_pos, max_pos)
    } else {
        pos.clamp(min_pos, max_pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_divider_default_split() {
        // When pos is -1 (uninitialized/default), should split 50/50
        let pos = calculate_clamped_divider_position(-1, 1024, 250, 270);
        assert_eq!(pos, 512);

        // When pos is 0, should split 50/50
        let pos = calculate_clamped_divider_position(0, 1024, 250, 270);
        assert_eq!(pos, 512);
    }

    #[test]
    fn test_divider_clamp_within_bounds() {
        // Valid position within bounds should be preserved
        let pos = calculate_clamped_divider_position(600, 1024, 250, 270);
        assert_eq!(pos, 600);
    }

    #[test]
    fn test_divider_clamp_minimum_editor() {
        // Position below min_editor (250) must clamp to 250
        let pos = calculate_clamped_divider_position(100, 1024, 250, 270);
        assert_eq!(pos, 250);
    }

    #[test]
    fn test_divider_clamp_minimum_preview() {
        // Position exceeding total_width - min_preview (1024 - 270 = 754) must clamp to 754
        let pos = calculate_clamped_divider_position(900, 1024, 250, 270);
        assert_eq!(pos, 754);
    }

    #[test]
    fn test_divider_narrow_window_clamping() {
        // When window is narrow (e.g. 500), max_pos = (500 - 270).max(250) = 250
        let pos = calculate_clamped_divider_position(400, 500, 250, 270);
        assert_eq!(pos, 250);

        let pos_default = calculate_clamped_divider_position(-1, 500, 250, 270);
        assert_eq!(pos_default, 250);
    }
}
