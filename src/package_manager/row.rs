// Copyright 2026 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib, glib::clone};

use crate::file_io::packages::PackageInfo;
use crate::package_manager::TypesetterManageTemplatesDialog;

/// Whether a package row represents a user-installed package, a cached
/// (compiler-downloaded) one, or a bundled (application-shipped) one.
/// Used to determine which confirmation message to show, which group to
/// refresh after deletion, and whether to show a delete button at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PackageKind {
    #[default]
    Installed,
    Cached,
    Bundled,
}

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Debug, Default)]
    pub struct TypesetterPackageRow {
        pub info: RefCell<Option<PackageInfo>>,
        pub kind: Cell<PackageKind>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterPackageRow {
        const NAME: &'static str = "TypesetterPackageRow";
        type Type = super::TypesetterPackageRow;
        type ParentType = adw::ActionRow;
    }

    impl ObjectImpl for TypesetterPackageRow {
        fn constructed(&self) {
            self.parent_constructed();
        }
    }

    impl WidgetImpl for TypesetterPackageRow {}
    impl ListBoxRowImpl for TypesetterPackageRow {}
    impl PreferencesRowImpl for TypesetterPackageRow {}
    impl ActionRowImpl for TypesetterPackageRow {}
}

glib::wrapper! {
    pub struct TypesetterPackageRow(ObjectSubclass<imp::TypesetterPackageRow>)
        @extends adw::ActionRow, adw::PreferencesRow, gtk::ListBoxRow, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Actionable;
}

impl TypesetterPackageRow {
    pub fn new(info: PackageInfo, kind: PackageKind) -> Self {
        let obj: Self = glib::Object::new();
        obj.set_title(info.display_name_or_name());
        obj.set_subtitle(&info.specifier());

        // Repository button, only present when the package declares a URL
        if let Some(url) = info.repository.clone() {
            let repo_button = gtk::Button::builder()
                .icon_name("info-outline-symbolic")
                .valign(gtk::Align::Center)
                .tooltip_text(&url)
                .css_classes(["flat"])
                .build();

            repo_button.connect_clicked(clone! {
                #[weak] obj,
                move |_| {
                    gtk::UriLauncher::new(&url).launch(
                        gtk::Widget::root(obj.upcast_ref::<gtk::Widget>())
                            .and_downcast::<gtk::Window>()
                            .as_ref(),
                        gio::Cancellable::NONE,
                        |_| {},
                    );
                }
            });

            obj.add_suffix(&repo_button);
        }

        // Delete button, only for packages the user can actually remove
        if kind != PackageKind::Bundled {
            let delete_button = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Delete")
                .valign(gtk::Align::Center)
                .css_classes(["destructive-action"])
                .build();

            delete_button.connect_clicked(clone! {
                #[weak] obj,
                move |_| obj.show_delete_dialog(),
            });

            obj.add_suffix(&delete_button);
        }

        obj.imp().info.replace(Some(info));
        obj.imp().kind.set(kind);
        obj
    }

    pub fn info(&self) -> Option<PackageInfo> {
        self.imp().info.borrow().clone()
    }

    fn show_delete_dialog(&self) {
        let Some(info) = self.info() else { return };
        let kind = self.imp().kind.get();

        let heading = "Remove Package?";
        let body = match kind {
            PackageKind::Installed => format!(
                "Are you sure you want to remove {}? This cannot be undone.",
                info.display_name_or_name()
            ),
            PackageKind::Cached => format!(
                "Are you sure you want to remove {}? It will be re-downloaded automatically if needed.",
                info.display_name_or_name()
            ),
            PackageKind::Bundled => return,
        };

        let dialog = adw::AlertDialog::new(Some(heading), Some(&body));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Remove");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        dialog.connect_response(
            None,
            clone! {
                #[weak(rename_to = this)] self,
                move |_, response| {
                    if response == "delete" {
                        this.delete_package();
                    }
                }
            },
        );

        dialog.present(Some(self));
    }

    fn delete_package(&self) {
        let Some(info) = self.info() else { return };
        let kind = self.imp().kind.get();

        match std::fs::remove_dir_all(&info.package_dir) {
            Ok(()) => {
                // Remove the package name directory if it is now empty
                // (i.e. no other versions remain). Fails silently if not empty.
                if let Some(package_name_dir) = info.package_dir.parent() {
                    let _ = std::fs::remove_dir(package_name_dir);
                }

                if let Some(dialog) = self
                    .ancestor(TypesetterManageTemplatesDialog::static_type())
                    .and_downcast::<TypesetterManageTemplatesDialog>()
                {
                    match kind {
                        PackageKind::Installed => dialog.refresh_installed(),
                        PackageKind::Cached => dialog.refresh_cached(),
                        PackageKind::Bundled => {}
                    }
                }

                // Manually clear the templates popover cache since the file
                // monitor can be unreliable on Flatpak paths.
                if let Some(app) = gio::Application::default().and_downcast::<gtk::Application>()
                    && let Some(window) = app
                        .active_window()
                        .and_downcast::<crate::TypesetterWindow>()
                    && let Some(popover) = window.imp().templates_popover.borrow().as_ref()
                {
                    popover.clear_cache();
                }
            }
            Err(e) => {
                eprintln!(
                    "package-row: failed to delete {}: {}",
                    info.package_dir.display(),
                    e
                );
                if let Some(overlay) = self
                    .ancestor(adw::ToastOverlay::static_type())
                    .and_downcast::<adw::ToastOverlay>()
                {
                    let toast = adw::Toast::new(&format!(
                        "Failed to remove {}: {}",
                        info.display_name_or_name(),
                        e
                    ));
                    overlay.add_toast(toast);
                }
            }
        }
    }
}
