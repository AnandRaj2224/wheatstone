// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

mod row;
mod thumbnail_image;

use adw::{
    prelude::{PopoverExt, StaticType},
    subclass::prelude::*,
};
use gio::prelude::{FileExt, FileMonitorExt, SettingsExt};
use gtk::{
    CompositeTemplate, gio, glib,
    glib::clone,
    glib::prelude::Cast,
    prelude::{EditableExt, ObjectExt, WidgetExt},
};

use crate::file_io::packages::TemplateInfo;
use row::TemplateRow;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/dev/whetstone/editor/gtk/templates-popover.ui")]
    pub struct TypesetterTemplatesPopover {
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub templates: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub list_box: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub empty: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub no_results: TemplateChild<adw::StatusPage>,
        /// Cached template list, populated on first popover open and reused
        /// thereafter. Invalidate by calling `clear_cache()` when the
        /// packages directory changes.
        pub cache: std::cell::RefCell<Option<Vec<TemplateInfo>>>,
        /// Persistent settings instance, shared across calls to avoid
        /// constructing a new one on every popover open.
        pub settings: std::cell::OnceCell<gio::Settings>,
        /// File monitor for the user packages directory.
        pub user_monitor: std::cell::RefCell<Option<gio::FileMonitor>>,
        /// File monitor for the bundled templates directory.
        pub bundled_monitor: std::cell::RefCell<Option<gio::FileMonitor>>,
        /// Pending cache invalidation timeout source. Debounces bursts of
        /// file system events (e.g. a package installed file-by-file).
        pub invalidate_timeout: std::cell::RefCell<Option<glib::SourceId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterTemplatesPopover {
        const NAME: &'static str = "TypesetterTemplatesPopover";
        type Type = super::TypesetterTemplatesPopover;
        type ParentType = gtk::Popover;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TypesetterTemplatesPopover {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            // Initialise settings once and store for reuse
            let settings = gio::Settings::new(crate::config::APP_ID);
            let _ = self.settings.set(settings.clone());

            let list_box = self.list_box.get();
            let search_entry = self.search_entry.get();

            let search_text = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
            let search_text_clone = search_text.clone();

            list_box.set_filter_func(move |row| {
                let query = search_text.borrow().to_lowercase();
                if let Ok(template_row) = row.clone().downcast::<TemplateRow>() {
                    template_row.matches_filter(&query)
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
                    if let Some(template_row) = row.downcast_ref::<TemplateRow>() {
                        let Some(info) = template_row.get_info() else { return };
                        let path = info.entrypoint_path.to_string_lossy().to_string();

                        obj.popdown();

                        obj.emit_by_name::<()>("template-activated", &[&path]);
                    }
                }
            });

            obj.connect_show(clone! {
                #[weak] obj,
                move |_| {
                    obj.refresh_list();
                }
            });

            // Invalidate the cache when the show-bundled-templates setting
            // changes so the next popover open reflects the new preference.
            settings.connect_changed(
                Some("show-bundled-templates"),
                clone! {
                    #[weak] obj,
                    move |_, _| {
                        obj.clear_cache();
                    }
                },
            );

            // Watch both package directories for changes and invalidate the
            // cache when anything is added, removed, or modified. Changes are
            // debounced so that a burst of events (e.g. a package installed
            // file-by-file) only triggers a single invalidation.
            let watch = |path: std::path::PathBuf| -> Option<gio::FileMonitor> {
                let file = gio::File::for_path(&path);
                file.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
                    .ok()
            };

            let user_monitor = watch(crate::utils::xdg_data_dir().join("packages"));
            let bundled_monitor = watch(crate::utils::bundled_templates_dir());

            for monitor in [&user_monitor, &bundled_monitor].into_iter().flatten() {
                monitor.connect_changed(clone! {
                    #[weak] obj,
                    move |_, _, _, _| {
                        // Cancel any pending invalidation
                        if let Some(id) = obj.imp().invalidate_timeout.borrow_mut().take() {
                            id.remove();
                        }
                        // Schedule a fresh one 300ms from now
                        let source_id = glib::timeout_add_local_once(
                            std::time::Duration::from_millis(300),
                            clone! {
                                #[weak] obj,
                                move || {
                                    obj.imp().invalidate_timeout.borrow_mut().take();
                                    obj.clear_cache();
                                }
                            },
                        );
                        *obj.imp().invalidate_timeout.borrow_mut() = Some(source_id);
                    }
                });
            }

            *self.user_monitor.borrow_mut() = user_monitor;
            *self.bundled_monitor.borrow_mut() = bundled_monitor;
        }

        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: std::sync::OnceLock<Vec<glib::subclass::Signal>> =
                std::sync::OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("template-activated")
                        .param_types([str::static_type()])
                        .build(),
                ]
            })
        }
    }

    impl WidgetImpl for TypesetterTemplatesPopover {}
    impl PopoverImpl for TypesetterTemplatesPopover {}
}

glib::wrapper! {
    pub struct TypesetterTemplatesPopover(ObjectSubclass<imp::TypesetterTemplatesPopover>)
        @extends gtk::Widget, gtk::Popover,
        @implements gtk::Buildable, gtk::ConstraintTarget, gtk::Accessible, gtk::Orientable, gtk::Actionable, gtk::ShortcutManager, gtk::Native;
}

impl TypesetterTemplatesPopover {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn clear_cache(&self) {
        self.imp().cache.borrow_mut().take();
    }

    pub fn refresh_list(&self) {
        let imp = self.imp();
        let list_box = imp.list_box.get();

        // Populate the cache on first open, reuse it thereafter
        let mut cache = imp.cache.borrow_mut();
        let templates = cache.get_or_insert_with(crate::file_io::packages::load_templates);

        let show_bundled = imp
            .settings
            .get()
            .map(|s| s.boolean("show-bundled-templates"))
            .unwrap_or(true);

        while let Some(row) = list_box.first_child() {
            list_box.remove(&row);
        }

        for info in templates.iter().cloned() {
            if !show_bundled && info.is_bundled {
                continue;
            }
            let row = TemplateRow::new(info);
            list_box.append(&row);
        }

        drop(cache);
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
            if let Ok(r) = row.downcast::<TemplateRow>() {
                total += 1;
                if r.matches_filter(&query) {
                    matches += 1;
                }
            }
        }

        imp.templates.get().set_visible(false);
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
                imp.templates.get().set_visible(true);
                imp.search_entry.remove_css_class("error");
                imp.search_entry.set_sensitive(true);
            }
        }
    }
}

impl Default for TypesetterTemplatesPopover {
    fn default() -> Self {
        Self::new()
    }
}
