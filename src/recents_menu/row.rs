// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use super::TypesetterRecentFilesPopover;
use crate::file_io::recent::{RecentFile, load_recent_files, save_recent_files};
use adw::subclass::prelude::*;
use gtk::{CompositeTemplate, glib, glib::clone, prelude::*};

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/dev/whetstone/editor/gtk/recent-file-row.ui")]
    pub struct RecentFileRow {
        #[template_child]
        pub row: TemplateChild<gtk::Box>,
        #[template_child]
        pub title: TemplateChild<gtk::Label>,
        #[template_child]
        pub subtitle: TemplateChild<gtk::Label>,
        #[template_child]
        pub close_button: TemplateChild<gtk::Button>,
        pub file: std::cell::RefCell<RecentFile>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RecentFileRow {
        const NAME: &'static str = "RecentFileRow";
        type Type = super::RecentFileRow;
        type ParentType = gtk::ListBoxRow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for RecentFileRow {}
    impl WidgetImpl for RecentFileRow {}
    impl ListBoxRowImpl for RecentFileRow {}
}

glib::wrapper! {
    pub struct RecentFileRow(ObjectSubclass<imp::RecentFileRow>)
        @extends gtk::Widget, gtk::ListBoxRow,
        @implements gtk::Buildable, gtk::ConstraintTarget, gtk::Accessible, gtk::Orientable, gtk::Actionable, gtk::ShortcutManager;
}

impl RecentFileRow {
    pub fn new(file: RecentFile, parent: &TypesetterRecentFilesPopover) -> Self {
        let obj: Self = glib::Object::new();
        let popover_weak = parent.downgrade();
        {
            let imp = obj.imp();
            imp.file.replace(file.clone());
            imp.title.set_label(file.display_name());
            imp.subtitle.set_label(&file.display_directory());
            imp.row.set_tooltip_text(Some(&format!(
                "{}{}{}",
                file.display_directory(),
                if file.display_directory().ends_with('/') {
                    ""
                } else {
                    "/"
                },
                file.display_name()
            )));
            imp.close_button.connect_clicked(clone! {
                #[weak] obj,
                move |_| {
                    let file = obj.imp().file.borrow().clone();
                    let recent_files = load_recent_files();
                    let filtered_files: Vec<_> = recent_files
                        .into_iter()
                        .filter(|f| f.uri != file.uri)
                        .collect();
                    save_recent_files(&filtered_files);
                    if let Some(list_box) = obj.parent().and_then(|w| w.downcast::<gtk::ListBox>().ok()) {
                        list_box.remove(&obj);
                    }
                    if let Some(popover) = popover_weak.upgrade() {
                        popover.update_visibility();
                    }
                }
            });
        }
        obj
    }

    pub fn get_file(&self) -> RecentFile {
        self.imp().file.borrow().clone()
    }

    pub fn matches_filter(&self, query: &str) -> bool {
        self.get_file()
            .display_name()
            .to_lowercase()
            .contains(&query.to_lowercase())
    }
}
