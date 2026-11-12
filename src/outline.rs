// Copyright 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::{prelude::*, subclass::prelude::*};
use gtk::{CompositeTemplate, gdk, glib};

use crate::typst_system::DocumentHeading;

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(CompositeTemplate, Debug, Default)]
    #[template(resource = "/dev/whetstone/editor/gtk/outline.ui")]
    pub struct TypesetterOutline {
        #[template_child]
        pub search_bar: TemplateChild<gtk::SearchBar>,
        #[template_child]
        pub search_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub scroll_window: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub headings_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub empty_state: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub no_results: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub error_overlay: TemplateChild<adw::StatusPage>,
        /// Full unfiltered list of headings, kept for filtering
        pub headings: RefCell<Vec<DocumentHeading>>,
        /// Currently visible (filtered) headings, indexed by row position.
        /// The single row_activated handler reads from this.
        pub filtered_headings: RefCell<Vec<DocumentHeading>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterOutline {
        const NAME: &'static str = "TypesetterOutline";
        type Type = super::TypesetterOutline;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TypesetterOutline {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();
            obj.setup_signals();
            obj.load_outline();
        }
    }

    impl WidgetImpl for TypesetterOutline {}
    impl AdwDialogImpl for TypesetterOutline {}
}

glib::wrapper! {
    pub struct TypesetterOutline(ObjectSubclass<imp::TypesetterOutline>)
        @extends gtk::Widget, adw::Dialog,
        @implements gtk::Buildable,
                    gtk::ConstraintTarget,
                    gtk::Accessible;
}

impl Default for TypesetterOutline {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl TypesetterOutline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Connect all signals at construction time
    fn setup_signals(&self) {
        let imp = imp::TypesetterOutline::from_obj(self);

        // Search entry, filter the list on every keystroke
        let dialog_weak = self.downgrade();
        imp.search_entry.connect_search_changed(move |entry| {
            let Some(dialog) = dialog_weak.upgrade() else {
                return;
            };
            dialog.apply_filter(&entry.text());
        });

        // Row activation, reads from filtered_headings by index
        let dialog_weak = self.downgrade();
        imp.headings_list.connect_row_activated(move |_, row| {
            let Some(dialog) = dialog_weak.upgrade() else {
                return;
            };
            let imp = imp::TypesetterOutline::from_obj(&dialog);
            let index = row.index() as usize;
            if let Some(heading) = imp.filtered_headings.borrow().get(index) {
                dialog.jump_to(heading.source_offset);
            }
        });

        // Up/Down move the highlighted row while keeping keyboard focus in
        // the search entry, so the user can keep typing and navigate at once
        let dialog_weak = self.downgrade();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_controller, keyval, _keycode, _state| {
            let Some(dialog) = dialog_weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match keyval {
                gdk::Key::Down => {
                    dialog.move_selection(1);
                    glib::Propagation::Stop
                }
                gdk::Key::Up => {
                    dialog.move_selection(-1);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        imp.search_entry.add_controller(key_controller);

        // Return activates the currently highlighted row, if any (nothing
        // is highlighted until the user starts typing a query)
        let dialog_weak = self.downgrade();
        imp.search_entry.connect_activate(move |_entry| {
            let Some(dialog) = dialog_weak.upgrade() else {
                return;
            };
            dialog.activate_selected();
        });
    }

    /// Load the outline from the window's compiler system
    fn load_outline(&self) {
        let dialog_weak = self.downgrade();

        glib::spawn_future_local(async move {
            let Some(dialog) = dialog_weak.upgrade() else {
                return;
            };

            let Some(root) = dialog.root() else {
                dialog.show_error();
                return;
            };

            let Some(window) = root.downcast_ref::<crate::window::TypesetterWindow>() else {
                dialog.show_error();
                return;
            };

            let compiler_system = match window.compiler_system().await {
                Ok(system) => system,
                Err(_) => {
                    dialog.show_error();
                    return;
                }
            };

            if !compiler_system.has_document() {
                let text = window.imp().editor_container.contents();
                let _ = compiler_system
                    .request_preview(text, window.current_file_path(), window.project_root())
                    .await;
            }

            if !compiler_system.arena_last_compile_succeeded() {
                dialog.show_error();
                return;
            }

            match compiler_system.request_document_outline().await {
                Ok(headings) => dialog.populate_headings(headings),
                Err(_) => dialog.show_error(),
            }
        });
    }

    /// Store the full heading list and do an initial unfiltered render
    fn populate_headings(&self, headings: Vec<DocumentHeading>) {
        let imp = imp::TypesetterOutline::from_obj(self);

        while let Some(child) = imp.headings_list.first_child() {
            imp.headings_list.remove(&child);
        }
        for heading in &headings {
            imp.headings_list.append(&self.make_heading_row(heading));
        }

        *imp.headings.borrow_mut() = headings.clone();
        *imp.filtered_headings.borrow_mut() = headings;

        self.update_visibility("");
    }

    /// Filter the heading list and update which panel is visible
    fn apply_filter(&self, query: &str) {
        let imp = imp::TypesetterOutline::from_obj(self);
        let query_lower = query.to_lowercase();

        let filtered: Vec<DocumentHeading> = imp
            .headings
            .borrow()
            .iter()
            .filter(|h| query_lower.is_empty() || h.label.to_lowercase().contains(&query_lower))
            .cloned()
            .collect();

        while let Some(child) = imp.headings_list.first_child() {
            imp.headings_list.remove(&child);
        }
        for heading in &filtered {
            imp.headings_list.append(&self.make_heading_row(heading));
        }

        let filtered_len = filtered.len();
        *imp.filtered_headings.borrow_mut() = filtered;

        if filtered_len == 0 && !query_lower.is_empty() {
            imp.search_entry.add_css_class("error");
        } else {
            imp.search_entry.remove_css_class("error");
        }

        self.update_visibility(query);

        if query_lower.is_empty() {
            imp.headings_list.select_row(gtk::ListBoxRow::NONE);
        } else {
            self.select_first_row();
        }
    }

    /// Enable or disable search bar and button, managing key capture accordingly
    fn set_search_enabled(&self, enabled: bool) {
        let imp = imp::TypesetterOutline::from_obj(self);
        if enabled {
            imp.search_bar.set_key_capture_widget(Some(self));
        } else {
            imp.search_bar.set_key_capture_widget(gtk::Widget::NONE);
            imp.search_bar.set_search_mode(false);
        }
        imp.search_button.set_sensitive(enabled);
    }

    /// Show the correct panel based on current state
    fn update_visibility(&self, query: &str) {
        let imp = imp::TypesetterOutline::from_obj(self);
        let query_lower = query.to_lowercase();

        let total = imp.headings.borrow().len();
        let matches = imp.filtered_headings.borrow().len();

        self.set_search_enabled(total > 0);

        imp.scroll_window.set_visible(false);
        imp.empty_state.set_visible(false);
        imp.no_results.set_visible(false);
        imp.error_overlay.set_visible(false);

        match (total, matches, query_lower.is_empty()) {
            (0, _, _) => imp.empty_state.set_visible(true),
            (_, 0, false) => imp.no_results.set_visible(true),
            _ => imp.scroll_window.set_visible(true),
        }
    }

    /// Highlight the first row of the (possibly filtered) list, if any,
    /// so it's clear what pressing Return will do
    fn select_first_row(&self) {
        let imp = imp::TypesetterOutline::from_obj(self);
        match imp.headings_list.row_at_index(0) {
            Some(row) => imp.headings_list.select_row(Some(&row)),
            None => imp.headings_list.select_row(gtk::ListBoxRow::NONE),
        }
    }

    /// Move the highlighted row up or down by `delta`, clamped to the list
    /// bounds, without moving keyboard focus away from the search entry
    fn move_selection(&self, delta: i32) {
        let imp = imp::TypesetterOutline::from_obj(self);
        let len = imp.filtered_headings.borrow().len();
        if len == 0 {
            return;
        }

        let current = imp
            .headings_list
            .selected_row()
            .map(|row| row.index())
            .unwrap_or(-1);
        let new_index = (current + delta).clamp(0, len as i32 - 1);

        if let Some(row) = imp.headings_list.row_at_index(new_index) {
            imp.headings_list.select_row(Some(&row));
            self.scroll_row_into_view(&row);
        }
    }

    /// Scroll the list just enough to bring `row` fully into view, without
    /// touching keyboard focus
    fn scroll_row_into_view(&self, row: &gtk::ListBoxRow) {
        let imp = imp::TypesetterOutline::from_obj(self);
        let adjustment = imp.scroll_window.vadjustment();

        let point = gtk::graphene::Point::new(0.0, 0.0);
        let Some(viewport_point) = row.compute_point(&imp.scroll_window.get(), &point) else {
            return;
        };
        let viewport_y = viewport_point.y() as f64;

        let row_top = adjustment.value() + viewport_y;
        let row_bottom = row_top + row.height() as f64;
        let page_top = adjustment.value();
        let page_bottom = page_top + adjustment.page_size();

        if row_top < page_top {
            adjustment.set_value(row_top);
        } else if row_bottom > page_bottom {
            adjustment.set_value((row_bottom - adjustment.page_size()).max(0.0));
        }
    }

    /// Activate the currently highlighted row, if any, reusing the
    /// existing row-activated handler. Does nothing if no row is
    /// highlighted (e.g. the search box is empty)
    fn activate_selected(&self) {
        let imp = imp::TypesetterOutline::from_obj(self);

        if let Some(row) = imp.headings_list.selected_row() {
            imp.headings_list
                .emit_by_name::<()>("row-activated", &[&row]);
        }
    }

    /// Build a list row for a heading
    fn make_heading_row(&self, heading: &DocumentHeading) -> gtk::ListBoxRow {
        let indent = (heading.level.saturating_sub(1) as i32) * 16;

        let label = gtk::Label::new(Some(&heading.label));
        label.set_halign(gtk::Align::Start);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_margin_start(12 + indent);
        label.set_margin_top(12);
        label.set_margin_bottom(12);
        label.set_margin_end(12);

        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&label));
        row
    }

    /// Jump the editor to the given source offset and close the dialog
    fn jump_to(&self, source_offset: usize) {
        let dialog_weak = self.downgrade();

        glib::spawn_future_local(async move {
            let Some(dialog) = dialog_weak.upgrade() else {
                return;
            };
            let Some(root) = dialog.root() else { return };
            let Some(window) = root.downcast_ref::<crate::window::TypesetterWindow>() else {
                return;
            };

            let editor = window.imp().editor_container.get();
            let buffer = editor.buffer();
            let view = editor.view();

            let iter = editor.byte_offset_to_iter(source_offset);
            buffer.place_cursor(&iter);

            let mut scroll_iter = iter;
            let fraction = view.scroll_fraction();
            view.scroll_to_iter(&mut scroll_iter, 0.0, true, 0.0, fraction);
            view.grab_focus();

            dialog.close();

            view.grab_focus();
        });
    }

    fn show_error(&self) {
        let imp = imp::TypesetterOutline::from_obj(self);
        self.set_search_enabled(false);
        imp.scroll_window.set_visible(false);
        imp.empty_state.set_visible(false);
        imp.no_results.set_visible(false);
        imp.error_overlay.set_visible(true);
    }
}
