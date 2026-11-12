// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

mod search_entry;

use adw::subclass::prelude::*;
use gtk::glib::{Properties, clone};
use gtk::{CompositeTemplate, TemplateChild, gdk, glib, prelude::*};
use sourceview::{Buffer, SearchContext, SearchSettings, prelude::SearchSettingsExt};
use std::{
    cell::{Cell, RefCell},
    sync::OnceLock,
};

use super::view::TypesetterEditorView;
use crate::editor::search_toolbar::search_entry::SearchEntry;

mod imp {
    use super::*;

    #[derive(CompositeTemplate, Debug, Default, Properties)]
    #[properties(wrapper_type = super::SearchToolbar)]
    #[template(resource = "/dev/whetstone/editor/gtk/search-toolbar.ui")]
    pub struct SearchToolbar {
        #[property(get, set, default = false)]
        pub(crate) case_sensitive_mode: Cell<bool>,
        #[property(get, set, default = false)]
        pub(crate) regex_mode: Cell<bool>,
        #[property(get, set, default = false)]
        pub(crate) replace_mode: Cell<bool>,
        #[property(get, set, default = -1)]
        pub(crate) total_matches: Cell<i32>,

        #[template_child(id = "search_toolbar_revealer")]
        pub toolbar_revealer: TemplateChild<gtk::Revealer>,
        #[template_child(id = "search_find_entry")]
        pub find_entry: TemplateChild<SearchEntry>,
        #[template_child(id = "search_replace_entry")]
        pub replace_entry: TemplateChild<gtk::Entry>,
        #[template_child(id = "find_previous")]
        pub find_previous_button: TemplateChild<gtk::Button>,
        #[template_child(id = "find_next")]
        pub find_next_button: TemplateChild<gtk::Button>,
        #[template_child(id = "search_case_sensitive_button")]
        pub case_sensitive_button: TemplateChild<gtk::ToggleButton>,
        #[template_child(id = "search_regex_button")]
        pub regex_button: TemplateChild<gtk::ToggleButton>,
        #[template_child(id = "search_replace_button")]
        pub replace_toggle_button: TemplateChild<gtk::ToggleButton>,
        #[template_child(id = "replace_one")]
        pub replace_one_button: TemplateChild<gtk::Button>,
        #[template_child(id = "replace_all")]
        pub replace_all_button: TemplateChild<gtk::Button>,
        #[template_child(id = "replace_entry_revealer")]
        pub replace_entry_revealer: TemplateChild<gtk::Revealer>,
        #[template_child(id = "replace_one_revealer")]
        pub replace_one_revealer: TemplateChild<gtk::Revealer>,
        #[template_child(id = "replace_all_revealer")]
        pub replace_all_revealer: TemplateChild<gtk::Revealer>,

        pub buffer: OnceLock<sourceview::Buffer>,
        /// Persistent search context.
        pub context: OnceLock<sourceview::SearchContext>,
        /// Signal handler for context::occurrences-count-notify.
        pub context_signal_handler: RefCell<Option<glib::SignalHandlerId>>,
        /// Signal handler for buffer::mark-set (cursor tracking).
        /// Connected only while the toolbar is visible.
        pub buffer_mark_set_handler: RefCell<Option<glib::SignalHandlerId>>,
        /// Pending idle source for coalescing cursor position updates.
        pub cursor_update_source: RefCell<Option<glib::SourceId>>,
        /// Cursor offset recorded when the toolbar is revealed, so the first
        /// search after opening starts from the cursor rather than buffer start.
        pub offset_when_shown: Cell<i32>,
        pub settings: RefCell<Option<sourceview::SearchSettings>>,
        pub signal_handlers: RefCell<Vec<(glib::Object, glib::SignalHandlerId)>>,
        pub bindings: RefCell<Vec<glib::Binding>>,
        pub settings_bindings: RefCell<Vec<glib::Binding>>,
        pub view: OnceLock<TypesetterEditorView>,
        pub event_controller: RefCell<Option<gtk::EventControllerKey>>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for SearchToolbar {
        const NAME: &'static str = "SearchToolbar";
        type Type = super::SearchToolbar;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            SearchEntry::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[gtk::glib::derived_properties]
    impl ObjectImpl for SearchToolbar {
        fn constructed(&self) {
            let obj = self.obj();

            let binding = obj
                .bind_property(
                    "case-sensitive-mode",
                    &*self.case_sensitive_button,
                    "active",
                )
                .bidirectional()
                .sync_create()
                .build();
            self.bindings.borrow_mut().push(binding);

            let binding = obj
                .bind_property("regex-mode", &*self.regex_button, "active")
                .bidirectional()
                .sync_create()
                .build();
            self.bindings.borrow_mut().push(binding);

            let binding = obj
                .bind_property("replace-mode", &*self.replace_toggle_button, "active")
                .bidirectional()
                .sync_create()
                .build();
            self.bindings.borrow_mut().push(binding);

            let binding = obj
                .bind_property(
                    "replace-mode",
                    &*self.replace_entry_revealer,
                    "reveal-child",
                )
                .sync_create()
                .build();
            self.bindings.borrow_mut().push(binding);

            let binding = obj
                .bind_property("replace-mode", &*self.replace_one_revealer, "reveal-child")
                .sync_create()
                .build();
            self.bindings.borrow_mut().push(binding);

            let binding = obj
                .bind_property("replace-mode", &*self.replace_all_revealer, "reveal-child")
                .sync_create()
                .build();
            self.bindings.borrow_mut().push(binding);

            let binding = obj
                .bind_property("total-matches", &*self.find_next_button, "sensitive")
                .transform_to(|_, count: &gtk::glib::Value| {
                    Some(count.get::<i32>().unwrap_or(0) > 0)
                })
                .sync_create()
                .build();
            self.bindings.borrow_mut().push(binding);

            let binding = obj
                .bind_property("total-matches", &*self.find_previous_button, "sensitive")
                .transform_to(|_, count: &gtk::glib::Value| {
                    Some(count.get::<i32>().unwrap_or(0) > 0)
                })
                .sync_create()
                .build();
            self.bindings.borrow_mut().push(binding);

            let binding = obj
                .bind_property("total-matches", &*self.replace_all_button, "sensitive")
                .transform_to(|_, count: &gtk::glib::Value| {
                    Some(count.get::<i32>().unwrap_or(0) > 0)
                })
                .sync_create()
                .build();
            self.bindings.borrow_mut().push(binding);

            let binding = obj
                .bind_property("total-matches", &*self.replace_one_button, "sensitive")
                .transform_to(|_, count: &gtk::glib::Value| {
                    Some(count.get::<i32>().unwrap_or(0) > 0)
                })
                .sync_create()
                .build();
            self.bindings.borrow_mut().push(binding);

            obj.setup_find_entry_shortcuts();

            // Update error styling whenever total-matches changes.
            obj.connect_notify_local(Some("total-matches"), move |obj, _| {
                let imp = obj.imp();
                let count = obj.property::<i32>("total-matches");
                let is_empty = imp.find_entry.text().is_empty();

                if count == 0 && !is_empty {
                    imp.find_entry.add_css_class("error");
                } else {
                    imp.find_entry.remove_css_class("error");
                }
            });

            let handler_id = self.find_entry.connect_activate(clone! {
                #[weak] obj,
                move || {
                    if let Err(err) = obj.activate_action("window.find-next", None) {
                        eprintln!("Failed to activate action: {}", err);
                    }
                }
            });
            self.signal_handlers
                .borrow_mut()
                .push((self.find_entry.clone().upcast::<glib::Object>(), handler_id));

            let handler_id = self.replace_entry.connect_activate(clone! {
                #[weak] obj,
                move |_| {
                    if let Err(err) = obj.activate_action("window.replace-one", None) {
                        eprintln!("Failed to activate action: {}", err);
                    }
                }
            });
            self.signal_handlers.borrow_mut().push((
                self.replace_entry.clone().upcast::<glib::Object>(),
                handler_id,
            ));

            let handler_id = self.toolbar_revealer.connect_notify_local(
                Some("reveal-child"),
                clone! {
                    #[weak] obj,
                    move |revealer, _| {
                        if revealer.reveals_child() {
                            obj.on_toolbar_revealed();
                        } else {
                            obj.on_toolbar_hidden();
                        }
                    }
                },
            );
            self.signal_handlers.borrow_mut().push((
                self.toolbar_revealer.clone().upcast::<glib::Object>(),
                handler_id,
            ));

            self.parent_constructed();
        }

        fn dispose(&self) {
            // Cancel any pending cursor update idle before anything else
            if let Some(id) = self.cursor_update_source.borrow_mut().take() {
                id.remove();
            }

            // Unbind all property bindings
            for binding in self.bindings.borrow_mut().drain(..) {
                binding.unbind();
            }

            // Unbind settings bindings
            for binding in self.settings_bindings.borrow_mut().drain(..) {
                binding.unbind();
            }

            // Disconnect the persistent occurrences-count handler
            if let Some(handler_id) = self.context_signal_handler.borrow_mut().take()
                && let Some(context) = self.context.get()
            {
                context.disconnect(handler_id);
            }

            // Disconnect the cursor mark-set handler if still connected
            if let Some(handler_id) = self.buffer_mark_set_handler.borrow_mut().take()
                && let Some(buffer) = self.buffer.get()
            {
                buffer.disconnect(handler_id);
            }

            // Clean up all other signal handlers
            for (object, handler_id) in self.signal_handlers.borrow_mut().drain(..) {
                object.disconnect(handler_id);
            }

            // Remove event controller
            if let Some(controller) = self.event_controller.borrow_mut().take() {
                self.find_entry.remove_controller(&controller);
            }
        }
    }

    impl WidgetImpl for SearchToolbar {}
    impl BinImpl for SearchToolbar {}
}

gtk::glib::wrapper! {
    pub struct SearchToolbar(ObjectSubclass<imp::SearchToolbar>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Buildable, gtk::ConstraintTarget, gtk::Accessible,
                    gtk::Orientable, gtk::Actionable, gtk::ShortcutManager;
}

#[allow(clippy::new_without_default)]
impl SearchToolbar {
    pub fn new() -> Self {
        gtk::glib::Object::new()
    }

    pub fn set_context(&self, buffer: &Buffer, view: &TypesetterEditorView) {
        let imp = self.imp();

        let settings = imp
            .settings
            .borrow_mut()
            .get_or_insert_with(|| SearchSettings::builder().wrap_around(true).build())
            .clone();

        let binding = settings
            .bind_property("case-sensitive", self, "case-sensitive-mode")
            .bidirectional()
            .sync_create()
            .build();
        imp.settings_bindings.borrow_mut().push(binding);

        let binding = settings
            .bind_property("regex-enabled", self, "regex-mode")
            .bidirectional()
            .sync_create()
            .build();
        imp.settings_bindings.borrow_mut().push(binding);

        // Settings → entry: when search text changes externally (e.g. "find
        // selected text"), push it into our custom entry.
        let handler_id = settings.connect_notify_local(
            Some("search-text"),
            clone! {
                #[weak(rename_to = entry)] *imp.find_entry,
                move |settings, _| {
                    let text = settings.search_text().unwrap_or_default();
                    if entry.text().as_str() != text.as_str() {
                        entry.set_text(&text);
                    }
                }
            },
        );
        imp.signal_handlers
            .borrow_mut()
            .push((settings.clone().upcast::<glib::Object>(), handler_id));

        // Entry → settings: when the user types, push text back into settings.
        // The persistent context picks up the change and fires
        // occurrences-count-notify when the search is complete.
        let handler_id = imp.find_entry.connect_changed(clone! {
            #[weak(rename_to = obj)] self,
            #[weak] settings,
            move |text| {
                let text = text.text();
                if settings.search_text().unwrap_or_default() != text {
                    settings.set_search_text(Some(&text));
                }

                let imp = obj.imp();
                if let Some(context) = imp.context.get()
                    && let Some(view) = imp.view.get()
                {
                    let buffer = context.buffer();
                    let iter = buffer.iter_at_offset(imp.offset_when_shown.get());
                    if let Some((mut match_start, match_end, _)) = context.forward(&iter) {
                        buffer.select_range(&match_start, &match_end);
                        let fraction = view.scroll_fraction();
                        view.scroll_to_iter(&mut match_start, 0.0, true, 0.0, fraction);
                    }
                }
            }
        });
        imp.signal_handlers
            .borrow_mut()
            .push((imp.find_entry.clone().upcast::<glib::Object>(), handler_id));

        // Initial sync
        let initial = settings.search_text().unwrap_or_default();
        imp.find_entry.set_text(&initial);

        imp.buffer.set(buffer.clone()).ok();
        imp.view.set(view.clone()).ok();

        // Create the context immediately and keep it for the lifetime of this
        // buffer attachment. The occurrences-count handler stays connected
        // permanently — the index stays warm even while the toolbar is hidden.
        self.create_search_context();
    }

    /// Creates the search context and connects the permanent
    /// occurrences-count-notify handler. Called once from set_context.
    fn create_search_context(&self) {
        let imp = self.imp();

        let (Some(buffer), Some(settings)) =
            (imp.buffer.get(), imp.settings.borrow().as_ref().cloned())
        else {
            return;
        };

        let context = SearchContext::builder()
            .buffer(buffer)
            .settings(&settings)
            .highlight(false)
            .build();

        // occurrences-count-notify fires when the context finishes a search.
        // With a persistent context this only fires when the search text or
        // settings actually change — never spuriously on toolbar open/close.
        let handler_id = context.connect_occurrences_count_notify(clone! {
            #[weak(rename_to = obj)] self,
            move |context| {
                let count = context.occurrences_count();
                obj.set_total_matches(count);
                obj.update_occurrence_position();
            }
        });
        imp.context_signal_handler.replace(Some(handler_id));
        imp.context.set(context).ok();
    }

    /// Connects the buffer mark-set handler for cursor tracking.
    /// Called each time the toolbar is revealed; disconnected on hide.
    fn connect_mark_set_handler(&self) {
        let imp = self.imp();

        // Guard against double-connecting if called twice
        if imp.buffer_mark_set_handler.borrow().is_some() {
            return;
        }

        if let Some(buffer) = imp.buffer.get() {
            let handler_id = buffer.connect_mark_set(clone! {
                #[weak(rename_to = obj)] self,
                move |buffer, _, mark| {
                    if mark == &buffer.get_insert() {
                        if let Some(id) = obj.imp().cursor_update_source.borrow_mut().take() {
                            id.remove();
                        }
                        let source_id = glib::idle_add_local_once(clone! {
                            #[weak] obj,
                            move || {
                                obj.update_occurrence_position();
                                obj.imp().cursor_update_source.borrow_mut().take();
                            }
                        });
                        obj.imp().cursor_update_source.replace(Some(source_id));
                    }
                }
            });
            imp.buffer_mark_set_handler.replace(Some(handler_id));
        }
    }

    fn update_occurrence_position(&self) {
        let imp = self.imp();
        let Some(context) = imp.context.get() else {
            imp.find_entry.set_occurrence(-1, 0);
            return;
        };

        let count = context.occurrences_count().max(0) as u32;
        let buffer = context.buffer();
        let position = if let Some((sel_start, sel_end)) = buffer.selection_bounds() {
            context.occurrence_position(&sel_start, &sel_end)
        } else {
            -1
        };
        imp.find_entry.set_occurrence(position, count);
    }

    fn ensure_toolbar_visible_and_focused(&self, enable_replace_mode: bool) {
        let imp = self.imp();

        if enable_replace_mode && !self.replace_mode() {
            self.set_replace_mode(true);
        } else if !enable_replace_mode && self.replace_mode() {
            self.set_replace_mode(false);
        }

        if !imp.toolbar_revealer.reveals_child() {
            imp.toolbar_revealer.set_reveal_child(true);
        }

        imp.replace_entry.select_region(0, 0);
        imp.find_entry.select_region(0, -1);
        imp.find_entry.grab_focus();
    }

    fn on_toolbar_revealed(&self) {
        let imp = self.imp();

        // Record cursor position so the first search starts from here.
        if let Some(buffer) = imp.buffer.get() {
            let iter = buffer.iter_at_mark(&buffer.get_insert());
            imp.offset_when_shown.set(iter.offset());
        }

        // Re-enable highlighting now that the toolbar is visible.
        if let Some(context) = imp.context.get() {
            context.set_highlight(true);
        }

        // Connect cursor tracking — only active while toolbar is visible.
        self.connect_mark_set_handler();

        // Update the counter immediately using the warm context — no need to
        // wait for occurrences-count-notify since the context is persistent.
        self.update_occurrence_position();

        if !imp.find_entry.has_focus() && !imp.replace_entry.has_focus() {
            imp.replace_entry.select_region(0, 0);
            imp.find_entry.select_region(0, -1);
            imp.find_entry.grab_focus();
        }
    }

    fn on_toolbar_hidden(&self) {
        let imp = self.imp();

        // Cancel any pending cursor update
        if let Some(id) = imp.cursor_update_source.borrow_mut().take() {
            id.remove();
        }

        // Disconnect cursor tracking — no point tracking while hidden
        if let Some(handler_id) = imp.buffer_mark_set_handler.borrow_mut().take()
            && let Some(buffer) = imp.buffer.get()
        {
            buffer.disconnect(handler_id);
        }

        // Hide highlights — the index stays warm, they'll reappear instantly
        // when the toolbar is shown again.
        if let Some(context) = imp.context.get() {
            context.set_highlight(false);
        }

        // Reset the label — context and its index stay alive
        imp.find_entry.set_occurrence(-1, 0);

        if self.replace_mode() {
            self.set_replace_mode(false);
        }
    }

    pub fn revealer(&self) -> &gtk::Revealer {
        &self.imp().toolbar_revealer
    }

    pub fn find_next(&self) {
        let imp = self.imp();
        if let (Some(context), Some(view)) = (imp.context.get(), imp.view.get()) {
            let buffer = context.buffer();
            let insert_mark = buffer.get_insert();
            let mut iter = buffer.iter_at_mark(&insert_mark);

            if !iter.is_end() {
                iter.forward_char();
            }

            if let Some((match_start, match_end, _wrapped)) = context.forward(&iter) {
                self.select_and_scroll_to_match(&buffer, view, match_start, match_end);
            }
        }
        self.update_occurrence_position();
    }

    pub fn find_previous(&self) {
        let imp = self.imp();
        if let (Some(context), Some(view)) = (imp.context.get(), imp.view.get()) {
            let buffer = context.buffer();
            let insert_mark = buffer.get_insert();
            let iter = buffer.iter_at_mark(&insert_mark);

            if let Some((match_start, match_end, _wrapped)) = context.backward(&iter) {
                self.select_and_scroll_to_match(&buffer, view, match_start, match_end);
            }
        }
        self.update_occurrence_position();
    }

    fn prepare_replacement(&self) -> Option<(sourceview::SearchContext, String)> {
        let imp = self.imp();
        let context = imp.context.get()?.clone();
        let text = imp.replace_entry.text().to_string();
        Some((context, text))
    }

    pub fn replace_all(&self) {
        if let Some((context, text)) = self.prepare_replacement()
            && let Err(err) = context.replace_all(&text)
        {
            eprintln!("Replace all failed: {}", err);
        }
        self.update_occurrence_position();
    }

    pub fn replace_next(&self) {
        if let Some((context, text)) = self.prepare_replacement() {
            let buffer = context.buffer();

            if let Some((sel_start, sel_end)) = buffer.selection_bounds()
                && let Some((mut match_start, mut match_end, _)) = context.forward(&sel_start)
                && sel_start == match_start
                && sel_end == match_end
                && let Err(err) = context.replace(&mut match_start, &mut match_end, &text)
            {
                eprintln!("Error replacing text: {:?}", err);
            }

            self.find_next();
        }
    }

    pub fn search_cancel(&self) {
        let imp = self.imp();
        if imp.toolbar_revealer.reveals_child() {
            imp.toolbar_revealer.set_reveal_child(false);
        }
    }

    pub fn search_find(&self) {
        self.ensure_toolbar_visible_and_focused(false);
    }

    pub fn search_replace(&self) {
        self.ensure_toolbar_visible_and_focused(true);
    }

    fn select_and_scroll_to_match(
        &self,
        buffer: &Buffer,
        view: &TypesetterEditorView,
        mut match_start: gtk::TextIter,
        match_end: gtk::TextIter,
    ) {
        buffer.select_range(&match_start, &match_end);
        let fraction = view.scroll_fraction();
        view.scroll_to_iter(&mut match_start, 0.0, true, 0.0, fraction);
    }

    pub fn setup_find_entry_shortcuts(&self) {
        let imp = self.imp();
        let entry = imp.find_entry.get();
        let find_entry = imp.find_entry.clone();
        let replace_entry = imp.replace_entry.clone();
        let controller = gtk::EventControllerKey::new();
        let weak_self = self.downgrade();

        controller.connect_key_pressed(move |_, keyval, _, state| {
            if let Some(obj) = weak_self.upgrade() {
                if keyval == gdk::Key::Return && state.contains(gdk::ModifierType::SHIFT_MASK) {
                    if let Err(err) = obj.activate_action("window.find-previous", None) {
                        eprintln!("Failed to activate action: {}", err);
                    }
                    return true.into();
                }
                if keyval == gdk::Key::Escape {
                    if let Err(err) = obj.activate_action("window.search-cancel", None) {
                        eprintln!("Failed to activate action: {}", err);
                    }
                    return true.into();
                }
                if keyval == gdk::Key::h && state.contains(gdk::ModifierType::CONTROL_MASK) {
                    obj.search_replace();
                    find_entry.select_region(0, 0);
                    replace_entry.select_region(0, -1);
                    replace_entry.grab_focus();
                    return true.into();
                }
            }
            false.into()
        });

        imp.event_controller.replace(Some(controller.clone()));
        entry.add_controller(controller);
    }
}
