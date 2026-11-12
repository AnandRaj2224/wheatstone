// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::{prelude::*, subclass::prelude::*};
use gtk::{
    CompositeTemplate,
    gio::{self},
    glib,
};

use crate::typst_system::PdfStandardType;

// Array of PDF standards matching the order in preferences.ui
const PDF_STANDARDS: [PdfStandardType; 17] = [
    PdfStandardType::V14,
    PdfStandardType::V15,
    PdfStandardType::V16,
    PdfStandardType::V17,
    PdfStandardType::V20,
    PdfStandardType::A1b,
    PdfStandardType::A1a,
    PdfStandardType::A2b,
    PdfStandardType::A2u,
    PdfStandardType::A2a,
    PdfStandardType::A3b,
    PdfStandardType::A3a,
    PdfStandardType::A3u,
    PdfStandardType::A4,
    PdfStandardType::A4f,
    PdfStandardType::A4e,
    PdfStandardType::Ua1,
];

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(CompositeTemplate, Debug, Default)]
    #[template(resource = "/dev/whetstone/editor/gtk/preferences.ui")]
    pub struct TypesetterPreferences {
        #[template_child]
        pub autosave_toggle: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub highlight_current_line_toggle: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub pdf_standard_combo: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub pdf_standard_reset: TemplateChild<gtk::Button>,
        #[template_child]
        pub scroll_on_type_toggle: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub centered_scrolling_toggle: TemplateChild<adw::SwitchRow>,
        pub settings: RefCell<Option<gio::Settings>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterPreferences {
        const NAME: &'static str = "TypesetterPreferences";
        type Type = super::TypesetterPreferences;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TypesetterPreferences {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            obj.bind_settings(&gio::Settings::new(crate::config::APP_ID));
            obj.setup_pdf_standard_reset();
        }
    }

    impl WidgetImpl for TypesetterPreferences {}

    impl AdwDialogImpl for TypesetterPreferences {}

    impl PreferencesDialogImpl for TypesetterPreferences {}
}

glib::wrapper! {
    pub struct TypesetterPreferences(ObjectSubclass<imp::TypesetterPreferences>)
        @extends gtk::Widget, adw::Dialog, adw::PreferencesDialog,
        @implements gtk::Buildable,
                    gtk::ConstraintTarget,
                    gtk::Accessible,
                    gtk::Orientable,
                    gtk::Actionable,
                    gtk::ShortcutManager;
}

impl Default for TypesetterPreferences {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl TypesetterPreferences {
    /// Enable automatic syncing between UI state and persistent settings.
    pub(crate) fn bind_settings(&self, settings: &gio::Settings) {
        let imp = imp::TypesetterPreferences::from_obj(self);

        // Store settings for PDF standard handling
        imp.settings.replace(Some(settings.clone()));

        // Toggle: Automatically save changes to files.
        settings
            .bind("autosave", &imp.autosave_toggle.get(), "active")
            .flags(gio::SettingsBindFlags::DEFAULT)
            .build();

        // Toggle: Center the caret position vertically while typing.
        settings
            .bind(
                "centered-scrolling",
                &imp.centered_scrolling_toggle.get(),
                "active",
            )
            .flags(gio::SettingsBindFlags::DEFAULT)
            .build();

        // Toggle: Highlight the current line in the editor.
        settings
            .bind(
                "highlight-current-line",
                &imp.highlight_current_line_toggle.get(),
                "active",
            )
            .flags(gio::SettingsBindFlags::DEFAULT)
            .build();

        // ComboBox: Select which PDF standard to use for export.
        // We need custom handling here since we're converting between string values and combo indices
        self.setup_pdf_standard_binding(settings);

        // Toggle: Automatically scroll the preview to the current position in the editor.
        settings
            .bind(
                "scroll-preview-on-type",
                &imp.scroll_on_type_toggle.get(),
                "active",
            )
            .flags(gio::SettingsBindFlags::DEFAULT)
            .build();
    }

    /// Set up custom binding for PDF standard combo box
    fn setup_pdf_standard_binding(&self, settings: &gio::Settings) {
        let imp = imp::TypesetterPreferences::from_obj(self);

        // Load initial value from settings
        let current_standard = settings.string("pdf-standard");
        let index = PDF_STANDARDS
            .iter()
            .position(|s| s.as_str() == current_standard.as_str())
            .unwrap_or(3); // Default to V17 (index 3) if not found

        imp.pdf_standard_combo.set_selected(index as u32);

        // Update reset button sensitivity based on initial value
        imp.pdf_standard_reset
            .set_sensitive(current_standard.as_str() != PdfStandardType::V17.as_str());

        // Connect signal to save changes
        let settings_clone = settings.clone();
        let reset_button_weak = imp.pdf_standard_reset.downgrade();
        imp.pdf_standard_combo
            .connect_selected_notify(move |combo| {
                let selected_index = combo.selected() as usize;
                if let Some(standard) = PDF_STANDARDS.get(selected_index) {
                    let _ = settings_clone.set_string("pdf-standard", standard.as_str());

                    // Update reset button sensitivity
                    if let Some(reset_button) = reset_button_weak.upgrade() {
                        reset_button.set_sensitive(*standard != PdfStandardType::V17);
                    }
                }
            });

        // Listen for external changes to the setting
        let combo_weak = imp.pdf_standard_combo.downgrade();
        let reset_button_weak = imp.pdf_standard_reset.downgrade();
        settings.connect_changed(Some("pdf-standard"), move |settings, _| {
            if let Some(combo) = combo_weak.upgrade() {
                let current_standard = settings.string("pdf-standard");
                if let Some(index) = PDF_STANDARDS
                    .iter()
                    .position(|s| s.as_str() == current_standard.as_str())
                {
                    combo.set_selected(index as u32);

                    // Update reset button sensitivity
                    if let Some(reset_button) = reset_button_weak.upgrade() {
                        reset_button.set_sensitive(
                            current_standard.as_str() != PdfStandardType::V17.as_str(),
                        );
                    }
                }
            }
        });
    }

    /// Get the currently selected PDF standard
    pub fn get_pdf_standard(&self) -> PdfStandardType {
        let imp = imp::TypesetterPreferences::from_obj(self);
        if let Some(settings) = imp.settings.borrow().as_ref() {
            let standard_str = settings.string("pdf-standard");
            PdfStandardType::from_str(&standard_str).unwrap_or(PdfStandardType::V17)
        } else {
            PdfStandardType::V17
        }
    }

    /// Set up PDF standard reset button
    fn setup_pdf_standard_reset(&self) {
        let imp = imp::TypesetterPreferences::from_obj(self);

        // Connect reset button click - use weak reference to avoid potential cycles
        if let Some(settings) = imp.settings.borrow().as_ref() {
            let settings = settings.clone();
            imp.pdf_standard_reset.connect_clicked(move |_| {
                // Reset to V17 (default)
                let _ = settings.set_string("pdf-standard", PdfStandardType::V17.as_str());
            });
        }
    }
}
