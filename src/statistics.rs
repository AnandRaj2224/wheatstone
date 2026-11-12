// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::{prelude::*, subclass::prelude::*};
use gtk::{CompositeTemplate, glib};

use crate::typst_system::DocumentStats;

mod imp {
    use super::*;

    #[derive(CompositeTemplate, Debug, Default)]
    #[template(resource = "/dev/whetstone/editor/gtk/statistics.ui")]
    pub struct TypesetterStatistics {
        #[template_child]
        pub words_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub all_chars_row: TemplateChild<adw::ExpanderRow>,
        #[template_child]
        pub cjk_chars_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub non_cjk_chars_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub pages_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub headings_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub paragraphs_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub equations_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub error_overlay: TemplateChild<adw::StatusPage>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterStatistics {
        const NAME: &'static str = "TypesetterStatistics";
        type Type = super::TypesetterStatistics;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TypesetterStatistics {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();
            obj.load_statistics();
        }
    }

    impl WidgetImpl for TypesetterStatistics {}

    impl AdwDialogImpl for TypesetterStatistics {}
}

glib::wrapper! {
    pub struct TypesetterStatistics(ObjectSubclass<imp::TypesetterStatistics>)
        @extends gtk::Widget, adw::Dialog,
        @implements gtk::Buildable,
                    gtk::ConstraintTarget,
                    gtk::Accessible;
}

impl Default for TypesetterStatistics {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl TypesetterStatistics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load statistics from the window's compiler system
    fn load_statistics(&self) {
        let dialog_weak = self.downgrade();

        glib::spawn_future_local(async move {
            let Some(dialog) = dialog_weak.upgrade() else {
                return;
            };

            // Get the window that owns this dialog
            let Some(root) = dialog.root() else {
                dialog.show_error();
                return;
            };

            let Some(window) = root.downcast_ref::<crate::window::TypesetterWindow>() else {
                dialog.show_error();
                return;
            };

            // Get the compiler system
            let compiler_system = match window.compiler_system().await {
                Ok(system) => system,
                Err(_) => {
                    dialog.show_error();
                    return;
                }
            };

            // If no document has been compiled yet, trigger one now
            if !compiler_system.has_document() {
                let text = window.imp().editor_container.contents();
                let _ = compiler_system
                    .request_preview(text, window.current_file_path(), window.project_root())
                    .await;
            }

            // Request document statistics
            match compiler_system.request_document_stats().await {
                Ok(stats) => {
                    dialog.update_ui_with_stats(stats);
                }
                Err(_) => {
                    dialog.show_error();
                }
            }
        });
    }

    fn update_ui_with_stats(&self, stats: DocumentStats) {
        let imp = imp::TypesetterStatistics::from_obj(self);

        // Ensure the error overlay is hidden
        imp.error_overlay.set_visible(false);

        // Calculate derived metrics
        let space_chars = stats.spaces;
        let non_space_chars = stats.chars.saturating_sub(space_chars);
        let non_cjk_chars = non_space_chars.saturating_sub(stats.cjk_chars);
        let non_cjk_words = stats.words.saturating_sub(stats.cjk_chars);

        // Format and display all numbers with thousands separators
        imp.words_row
            .set_subtitle(&Self::format_number(non_cjk_words));
        // Show characters excluding spaces as the main count
        imp.all_chars_row
            .set_subtitle(&Self::format_number(non_space_chars));
        imp.cjk_chars_row
            .set_subtitle(&Self::format_number(stats.cjk_chars));
        imp.non_cjk_chars_row
            .set_subtitle(&Self::format_number(non_cjk_chars));
        imp.pages_row
            .set_subtitle(&Self::format_number(stats.pages));
        imp.headings_row
            .set_subtitle(&Self::format_number(stats.headings));
        imp.paragraphs_row
            .set_subtitle(&Self::format_number(stats.paragraphs));
        imp.equations_row
            .set_subtitle(&Self::format_number(stats.equations));
    }

    fn show_error(&self) {
        let imp = imp::TypesetterStatistics::from_obj(self);
        imp.error_overlay.set_visible(true);
    }

    // TODO: i18n requires different separators for different languages...
    fn format_number(n: usize) -> String {
        // Format with thousands separators
        let s = n.to_string();
        let mut result = String::new();

        for (count, c) in s.chars().rev().enumerate() {
            if count > 0 && count % 3 == 0 {
                result.push(',');
            }
            result.push(c);
        }

        result.chars().rev().collect()
    }
}
