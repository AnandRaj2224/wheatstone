// Copyright 2026 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

mod import;
mod row;

use adw::prelude::{AdwDialogExt, PreferencesGroupExt};
use adw::subclass::prelude::*;
use gtk::{CompositeTemplate, gio, glib, prelude::*};

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/dev/whetstone/editor/gtk/package-manager.ui")]
    pub struct TypesetterManageTemplatesDialog {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub import_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub installed_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub installed_placeholder: TemplateChild<adw::ActionRow>,
        pub installed_rows: std::cell::RefCell<Vec<row::TypesetterPackageRow>>,
        #[template_child]
        pub cached_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub cached_placeholder: TemplateChild<adw::ActionRow>,
        pub cached_rows: std::cell::RefCell<Vec<row::TypesetterPackageRow>>,
        #[template_child]
        pub bundled_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub bundled_placeholder: TemplateChild<adw::ActionRow>,
        pub bundled_rows: std::cell::RefCell<Vec<row::TypesetterPackageRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterManageTemplatesDialog {
        const NAME: &'static str = "TypesetterManageTemplatesDialog";
        type Type = super::TypesetterManageTemplatesDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TypesetterManageTemplatesDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // "dialog.import-folder" action
            let action_folder = gio::SimpleAction::new("import-folder", None);
            action_folder.connect_activate(glib::clone! {
                #[weak] obj,
                move |_, _| {
                    glib::spawn_future_local(glib::clone! {
                        #[weak] obj,
                        async move {
                            let Some(parent) = obj.upcast_ref::<gtk::Widget>()
                                .root()
                                .and_downcast::<gtk::Window>() else { return };
                            match import::prompt_and_import_folder(&parent).await {
                                None => {}
                                Some(Ok(_)) => obj.on_import_success(),
                                Some(Err(e)) => obj.on_import_error(&e),
                            }
                        }
                    });
                }
            });

            // "dialog.import-archive" action
            let action_archive = gio::SimpleAction::new("import-archive", None);
            action_archive.connect_activate(glib::clone! {
                #[weak] obj,
                move |_, _| {
                    glib::spawn_future_local(glib::clone! {
                        #[weak] obj,
                        async move {
                            let Some(parent) = obj.upcast_ref::<gtk::Widget>()
                                .root()
                                .and_downcast::<gtk::Window>() else { return };
                            match import::prompt_and_import_archive(&parent).await {
                                None => {}
                                Some(Ok(_)) => obj.on_import_success(),
                                Some(Err(e)) => obj.on_import_error(&e),
                            }
                        }
                    });
                }
            });

            let action_group = gio::SimpleActionGroup::new();
            action_group.add_action(&action_folder);
            action_group.add_action(&action_archive);
            obj.insert_action_group("dialog", Some(&action_group));

            // TODO: Re-enable the toggle switch here after more testing and UI consideration
            // Wire the show-bundled-templates GSettings key to a toggle switch
            // let settings = gio::Settings::new(crate::config::APP_ID);

            // let bundled_switch = gtk::Switch::builder()
            //     .valign(gtk::Align::Center)
            //     .tooltip_text("Show Built-in Templates in the Template Picker")
            //     .build();

            // settings.bind("show-bundled-templates", &bundled_switch, "active")
            //     .build();

            // When the switch changes, refresh the bundled group and the popover
            // bundled_switch.connect_active_notify(glib::clone! {
            //     #[weak] obj,
            //     move |_| {
            //         obj.on_bundled_setting_changed();
            //     }
            // });

            // self.bundled_group.set_header_suffix(Some(&bundled_switch));

            obj.refresh_installed();
            obj.refresh_cached();
            obj.refresh_bundled();
        }
    }

    impl WidgetImpl for TypesetterManageTemplatesDialog {}
    impl AdwDialogImpl for TypesetterManageTemplatesDialog {}
}

glib::wrapper! {
    pub struct TypesetterManageTemplatesDialog(ObjectSubclass<imp::TypesetterManageTemplatesDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TypesetterManageTemplatesDialog {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Called after any successful import to refresh the UI and clear the
    /// templates popover cache.
    fn on_import_success(&self) {
        self.refresh_installed();
        self.clear_templates_popover_cache();
    }

    /// Shows a toast for a failed import.
    fn on_import_error(&self, e: &import::ImportError) {
        self.imp()
            .toast_overlay
            .add_toast(adw::Toast::new(&e.to_string()));
    }

    // TODO: Re-enable this after more testing
    /// Clears the templates popover cache
    // fn on_bundled_setting_changed(&self) {
    //     self.clear_templates_popover_cache();
    // }

    /// Clears the templates popover cache in the main window
    fn clear_templates_popover_cache(&self) {
        if let Some(app) = gio::Application::default().and_downcast::<gtk::Application>()
            && let Some(window) = app
                .active_window()
                .and_downcast::<crate::TypesetterWindow>()
            && let Some(popover) = window.imp().templates_popover.borrow().as_ref()
        {
            popover.clear_cache();
        }
    }

    /// Clears and repopulates the installed packages group.
    pub fn refresh_installed(&self) {
        let imp = self.imp();
        self.refresh_group(
            &imp.installed_group.get(),
            &imp.installed_placeholder.get(),
            &imp.installed_rows,
            crate::file_io::packages::load_packages,
            row::PackageKind::Installed,
        );
    }

    /// Clears and repopulates the cached packages group.
    pub fn refresh_cached(&self) {
        let imp = self.imp();
        self.refresh_group(
            &imp.cached_group.get(),
            &imp.cached_placeholder.get(),
            &imp.cached_rows,
            crate::file_io::packages::load_cached_packages,
            row::PackageKind::Cached,
        );
    }

    /// Clears and repopulates the bundled packages group.
    pub fn refresh_bundled(&self) {
        let imp = self.imp();
        self.refresh_group(
            &imp.bundled_group.get(),
            &imp.bundled_placeholder.get(),
            &imp.bundled_rows,
            crate::file_io::packages::load_bundled_packages,
            row::PackageKind::Bundled,
        );
    }

    /// Shared implementation for refreshing a package group.
    fn refresh_group(
        &self,
        group: &adw::PreferencesGroup,
        placeholder: &adw::ActionRow,
        rows: &std::cell::RefCell<Vec<row::TypesetterPackageRow>>,
        load: fn() -> Vec<crate::file_io::packages::PackageInfo>,
        kind: row::PackageKind,
    ) {
        for row in rows.borrow().iter() {
            group.remove(row);
        }
        rows.borrow_mut().clear();

        let packages = load();
        let has_packages = !packages.is_empty();

        for pkg in packages {
            let row = row::TypesetterPackageRow::new(pkg, kind);
            group.add(&row);
            rows.borrow_mut().push(row);
        }

        placeholder.set_visible(!has_packages);
        self.set_content_height(-1);
    }
}

impl Default for TypesetterManageTemplatesDialog {
    fn default() -> Self {
        Self::new()
    }
}
