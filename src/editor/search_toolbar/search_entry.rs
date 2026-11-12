// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{CompositeTemplate, TemplateChild, glib, prelude::*};
use std::cell::Cell;

mod imp {
    use super::*;

    #[derive(CompositeTemplate, Debug, Default)]
    #[template(resource = "/dev/whetstone/editor/gtk/search-entry.ui")]
    pub struct SearchEntry {
        #[template_child]
        pub text: TemplateChild<gtk::Text>,
        #[template_child]
        pub info: TemplateChild<gtk::Label>,
        #[template_child]
        pub clear: TemplateChild<gtk::Image>,
        pub occurrence_count: Cell<u32>,
        pub occurrence_position: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SearchEntry {
        const NAME: &'static str = "TypesetterSearchEntry";
        type Type = super::SearchEntry;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.set_layout_manager_type::<gtk::BoxLayout>();
            klass.set_css_name("entry");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SearchEntry {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // Show/hide the clear button based on text content
            self.text.connect_changed(glib::clone!(
                #[weak]
                obj,
                move |text| {
                    obj.imp().clear.set_visible(!text.text().is_empty());
                }
            ));

            // Wire up the clear gesture
            let gesture = gtk::GestureClick::new();
            gesture.connect_pressed(glib::clone!(
                #[weak]
                obj,
                move |_, _, _, _| {
                    obj.imp().text.set_text("");
                }
            ));
            self.clear.add_controller(gesture);

            // Accessibility label
            self.text
                .update_property(&[gtk::accessible::Property::Label("Search")]);
        }

        fn dispose(&self) {
            // Unparent all children
            let widget = self.obj();
            let mut child = widget.first_child();
            while let Some(c) = child {
                child = c.next_sibling();
                c.unparent();
            }
        }
    }

    impl WidgetImpl for SearchEntry {
        fn grab_focus(&self) -> bool {
            self.text.grab_focus()
        }
    }
}

glib::wrapper! {
    pub struct SearchEntry(ObjectSubclass<imp::SearchEntry>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for SearchEntry {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl SearchEntry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Select a region in the inner GtkText
    pub fn select_region(&self, start: i32, end: i32) {
        self.imp().text.select_region(start, end);
    }

    /// Connect to text changes
    pub fn connect_changed<F: Fn(&gtk::Text) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.imp().text.connect_changed(move |text| f(text))
    }

    pub fn connect_activate<F: Fn() + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.imp().text.connect_activate(move |_| f())
    }

    pub fn text(&self) -> glib::GString {
        self.imp().text.text()
    }

    pub fn set_text(&self, text: &str) {
        self.imp().text.set_text(text);
    }

    pub fn set_occurrence(&self, position: i32, count: u32) {
        let imp = self.imp();
        imp.occurrence_count.set(count);
        imp.occurrence_position.set(position);
        self.update_position();
    }

    fn update_position(&self) {
        let imp = self.imp();
        let count = imp.occurrence_count.get();
        let position = imp.occurrence_position.get();

        if count == 0 {
            imp.info.set_label("");
        } else {
            let pos = position.max(0) as u32;
            // Translators: {0} is the current match index, {1} is the total
            // number of matches. You may reorder these for your language.
            let label = gettext("{0} of {1}")
                .replace("{0}", &pos.to_string())
                .replace("{1}", &count.to_string());
            imp.info.set_label(&label);
        }
    }
}
