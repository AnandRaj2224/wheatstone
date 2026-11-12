// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::{gio, glib, prelude::*, subclass::prelude::*};
use std::cell::{OnceCell, RefCell};

mod imp {
    use super::*;
    use gtk::TemplateChild;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/dev/whetstone/editor/gtk/theme-selector.ui")]
    pub struct ThemeSelector {
        #[template_child(id = "_auto")]
        auto_button: TemplateChild<gtk::CheckButton>,
        #[template_child(id = "_light")]
        light_button: TemplateChild<gtk::CheckButton>,
        #[template_child(id = "_dark")]
        dark_button: TemplateChild<gtk::CheckButton>,
        pub settings: OnceCell<gio::Settings>,
        suppress_callback: RefCell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ThemeSelector {
        const NAME: &'static str = "ThemeSelector";
        type Type = super::ThemeSelector;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ThemeSelector {
        fn constructed(&self) {
            self.parent_constructed();
        }
    }

    impl WidgetImpl for ThemeSelector {}
    impl BoxImpl for ThemeSelector {}

    impl ThemeSelector {
        pub(crate) fn sync_from_settings(&self, settings: &gio::Settings) {
            *self.suppress_callback.borrow_mut() = true;

            let theme = settings.string("theme");

            match theme.as_str() {
                "light" => self.light_button.set_active(true),
                "dark" => self.dark_button.set_active(true),
                _ => self.auto_button.set_active(true),
            }

            *self.suppress_callback.borrow_mut() = false;
        }
    }

    #[gtk::template_callbacks]
    impl ThemeSelector {
        #[template_callback]
        fn on_option_selected(&self, _trigger: &gtk::CheckButton) {
            let selected_theme = match (
                self.auto_button.is_active(),
                self.light_button.is_active(),
                self.dark_button.is_active(),
            ) {
                (true, _, _) => Some("auto"),
                (_, true, _) => Some("light"),
                (_, _, true) => Some("dark"),
                _ => None,
            };

            if *self.suppress_callback.borrow() {
                return;
            }

            if let (Some(settings), Some(theme)) = (self.settings.get(), selected_theme) {
                let current = settings.string("theme").to_string();
                if current != theme
                    && let Err(err) = settings.set_string("theme", theme)
                {
                    eprintln!("Failed to set theme: {}", err);
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct ThemeSelector(ObjectSubclass<imp::ThemeSelector>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Buildable, gtk::ConstraintTarget, gtk::Accessible, gtk::Orientable, gtk::Actionable, gtk::ShortcutManager;
}

impl ThemeSelector {
    pub fn new(settings: gio::Settings) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp()
            .settings
            .set(settings.clone())
            .expect("Settings already set");
        obj
    }

    pub fn setup(&self) {
        let imp = self.imp();

        let settings = match imp.settings.get() {
            Some(s) => s,
            None => {
                eprintln!("ThemeSelector: gio::Settings not initialized in setup()");
                return;
            }
        };

        imp.sync_from_settings(settings);

        let obj = self.clone();
        let settings_clone = settings.clone();
        settings_clone.connect_changed(Some("theme"), {
            let settings = settings_clone.clone();
            move |_, _| {
                obj.imp().sync_from_settings(&settings);
            }
        });
    }
}
