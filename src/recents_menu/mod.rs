// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

mod row;

use adw::{prelude::PopoverExt, subclass::prelude::*};
use gettextrs::gettext;
use gtk::{
    CompositeTemplate, glib,
    glib::clone,
    glib::prelude::Cast,
    prelude::{EditableExt, FileExt, GtkWindowExt, WidgetExt},
};

use row::RecentFileRow;

mod imp {
    use super::*;
    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/dev/whetstone/editor/gtk/recent-files.ui")]
    pub struct TypesetterRecentFilesPopover {
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub recents: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub list_box: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub empty: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub no_results: TemplateChild<adw::StatusPage>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterRecentFilesPopover {
        const NAME: &'static str = "TypesetterRecentFilesPopover";
        type Type = super::TypesetterRecentFilesPopover;
        type ParentType = gtk::Popover;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TypesetterRecentFilesPopover {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            let list_box = self.list_box.get();
            let search_entry = self.search_entry.get();

            let search_text = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
            let search_text_clone = search_text.clone();

            list_box.set_filter_func(move |row| {
                let query = search_text.borrow().to_lowercase();

                if let Ok(custom_row) = row.clone().downcast::<RecentFileRow>() {
                    custom_row
                        .get_file()
                        .display_name
                        .to_lowercase()
                        .contains(&query)
                } else {
                    true
                }
            });

            search_entry.connect_changed(clone! {
                #[weak] obj,
                #[weak] list_box,
                move |entry| {
                    let query = entry.text().to_string();
                    *search_text_clone.borrow_mut() = query;
                    list_box.invalidate_filter();
                    obj.update_visibility();
                }
            });

            list_box.connect_row_activated(clone! {
                #[weak] obj,
                move |_, row| {
                    if let Some(custom_row) = row.downcast_ref::<row::RecentFileRow>() {
                        let file = custom_row.get_file();
                        let Some(file_path) = file.path_buf() else { return; };
                        let project_root = file.project_root_path();

                        let gio_file = gtk::gio::File::for_path(&file_path);

                        // Get the application to check all windows
                        if let Some(app) = obj.clone()
                            .upcast::<gtk::Widget>()
                            .root()
                            .and_then(|r| r.downcast::<gtk::ApplicationWindow>().ok())
                            .and_then(|w| w.application())
                            && let Some(existing_window) = crate::file_io::open::find_window_with_file(&app, &gio_file) {
                                existing_window.present();
                                obj.popdown();
                                return;
                            }

                        // File is not open, proceed with opening
                        if let Some(window) = obj.clone()
                            .upcast::<gtk::Widget>()
                            .root()
                            .and_then(|r| r.downcast::<gtk::ApplicationWindow>().ok())
                            .and_then(|w| w.downcast::<crate::window::TypesetterWindow>().ok())
                        {
                            gtk::glib::MainContext::default().spawn_local(async move {
                                match window.open_file(window.imp().editor_container.buffer(), Some(gtk::gio::File::for_path(&file_path))).await {
                                    Ok(Some(opened_file)) => {
                                        // Set the project root from the recent file
                                        window.imp().project_root.replace(project_root.clone());

                                        // Save to recent files with the project root
                                        let project_root_file = project_root.as_deref()
                                            .map(gtk::gio::File::for_path);
                                        crate::file_io::recent::add_recent_file(
                                            &opened_file,
                                            project_root_file.as_ref(),
                                        );
                                        if let Some(opened_path) = opened_file.path() {
                                            window.set_current_file_path(opened_path);
                                        }
                                        window.set_current_file_is_unsaved(false);
                                    }
                                    Ok(None) => {}
                                    Err(err) => {
                                        window.show_toast(&format!("{}{}", &gettext("Unable to open file: "), err), None);
                                    }
                                }
                            });

                            obj.popdown();
                        }
                    }
                }
            });

            obj.connect_show(clone! {
                #[weak] obj,
                move |_| {
                    obj.refresh_list();
                }
            });
        }
    }

    impl WidgetImpl for TypesetterRecentFilesPopover {}
    impl PopoverImpl for TypesetterRecentFilesPopover {}
}

glib::wrapper! {
    pub struct TypesetterRecentFilesPopover(ObjectSubclass<imp::TypesetterRecentFilesPopover>)
        @extends gtk::Widget, gtk::Popover,
        @implements gtk::Buildable, gtk::ConstraintTarget, gtk::Accessible, gtk::Orientable, gtk::Actionable, gtk::ShortcutManager, gtk::Native;
}

impl TypesetterRecentFilesPopover {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn refresh_list(&self) {
        let list_box = self.imp().list_box.get();

        while let Some(row) = list_box.first_child() {
            list_box.remove(&row);
        }

        let recent_files: Vec<_> = crate::file_io::recent::load_recent_files()
            .into_iter()
            .filter(|recent| recent.path_buf().is_some_and(|p| p.exists()))
            .collect();

        for recent in recent_files {
            let row = RecentFileRow::new(recent, self);
            list_box.append(&row);
        }

        self.update_visibility();
    }

    pub fn update_visibility(&self) {
        let imp = self.imp();
        let query = imp.search_entry.get().text().to_lowercase();

        let mut total = 0;
        let mut matches = 0;
        let mut child = imp.list_box.get().first_child();

        while let Some(row) = child {
            child = row.next_sibling();
            if let Ok(r) = row.downcast::<RecentFileRow>() {
                total += 1;
                if r.matches_filter(&query) {
                    matches += 1;
                }
            }
        }

        imp.recents.get().set_visible(false);
        imp.no_results.get().set_visible(false);
        imp.empty.get().set_visible(false);

        match (total, matches) {
            (0, _) => {
                imp.empty.get().set_visible(true);
                imp.search_entry.remove_css_class("error");
                imp.search_entry.set_sensitive(false);
            }
            (_, 0) => {
                imp.no_results.get().set_visible(true);
                imp.search_entry.add_css_class("error");
                imp.search_entry.set_sensitive(true);
            }
            _ => {
                imp.recents.get().set_visible(true);
                imp.search_entry.remove_css_class("error");
                imp.search_entry.set_sensitive(true);
            }
        }
    }
}

impl Default for TypesetterRecentFilesPopover {
    fn default() -> Self {
        Self::new()
    }
}
