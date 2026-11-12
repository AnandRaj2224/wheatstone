// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::sync::OnceLock;
use tokio::runtime::Runtime;

use crate::config::{APP_BASE_PATH, APP_ID, VERSION};
use crate::{TypesetterWindow, package_manager, preferences, system_info};

static TOKIO_RUNTIME: OnceLock<Runtime> = OnceLock::new();

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct TypesetterApplication {}

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterApplication {
        const NAME: &'static str = "TypesetterApplication";
        type Type = super::TypesetterApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for TypesetterApplication {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_gactions();
            obj.set_accels_for_action("app.package-manager", &["<primary>slash"]);
            obj.set_accels_for_action("app.preferences", &["<primary>comma"]);
            obj.set_accels_for_action("app.quit", &["<primary>q"]);
        }
    }

    impl ApplicationImpl for TypesetterApplication {
        fn startup(&self) {
            self.parent_startup();
            crate::migration::migrate_from_typesetter_if_needed();
            sourceview::StyleSchemeManager::default()
                .append_search_path("resource:///dev/whetstone/editor/style-schemes/");
        }

        fn activate(&self) {
            let application = self.obj();
            let window = application.active_window().unwrap_or_else(|| {
                let window = TypesetterWindow::new(&*application, None);
                window.upcast()
            });
            window.present();
        }

        fn open(&self, files: &[gio::File], _hint: &str) {
            let application = self.obj();
            for file in files {
                if let Some(path) = file.path() {
                    let gio_file = gio::File::for_path(&path);
                    if let Some(existing_window) = crate::file_io::open::find_window_with_file(
                        &application.clone().upcast::<gtk::Application>(),
                        &gio_file,
                    ) {
                        existing_window.present();
                    } else {
                        let window = TypesetterWindow::new(&*application, Some(gio_file));
                        window.present();
                    }
                }
            }
        }
    }

    impl GtkApplicationImpl for TypesetterApplication {}
    impl AdwApplicationImpl for TypesetterApplication {}
}

glib::wrapper! {
    pub struct TypesetterApplication(ObjectSubclass<imp::TypesetterApplication>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl TypesetterApplication {
    pub fn new(application_id: &str, flags: &gio::ApplicationFlags) -> Self {
        glib::Object::builder()
            .property("application-id", application_id)
            .property("flags", flags)
            .property("resource-base-path", APP_BASE_PATH)
            .build()
    }

    pub fn tokio_runtime() -> &'static Runtime {
        TOKIO_RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create Tokio runtime")
        })
    }

    fn setup_gactions(&self) {
        let quit_action = gio::ActionEntry::builder("quit")
            .activate(move |app: &Self, _, _| app.quit())
            .build();
        let about_action = gio::ActionEntry::builder("about")
            .activate(move |app: &Self, _, _| app.show_about())
            .build();
        let preferences_action = gio::ActionEntry::builder("preferences")
            .activate(move |app: &Self, _, _| app.show_preferences())
            .build();
        let manage_packages_action = gio::ActionEntry::builder("package-manager")
            .activate(move |app: &Self, _, _| app.show_manage_packages())
            .build();
        self.add_action_entries([
            quit_action,
            about_action,
            preferences_action,
            manage_packages_action,
        ]);
    }

    fn show_about(&self) {
        let window = self.active_window().unwrap();
        let about = adw::AboutDialog::builder()
            .application_name("Whetstone")
            .application_icon(APP_ID)
            .developers(vec!["Haydn Trowell"])
            .license_type(gtk::License::Gpl30)
            .version(VERSION)
            .debug_info(system_info::build_debug_info())
            .website("https://typesetter.trowell.net/")
            .issue_url("https://codeberg.org/haydn/typesetter/issues")
            .translator_credits(gettext("translator-credits"))
            .build();
        about.present(Some(&window));
    }

    fn show_preferences(&self) {
        let window = self.active_window().unwrap();
        let preferences = preferences::TypesetterPreferences::default();
        preferences.present(Some(&window));
    }

    fn show_manage_packages(&self) {
        let window = self.active_window().unwrap();
        let dialog = package_manager::TypesetterManageTemplatesDialog::new();
        dialog.present(Some(&window));
    }
}
