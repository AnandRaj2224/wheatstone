// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::file_io::packages::TemplateInfo;
use adw::subclass::prelude::*;
use gtk::{CompositeTemplate, gdk, gio, glib, prelude::*};

use super::thumbnail_image::ThumbnailImage;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/dev/whetstone/editor/gtk/template-row.ui")]
    pub struct TemplateRow {
        #[template_child]
        pub row: TemplateChild<gtk::Box>,
        #[template_child]
        pub title: TemplateChild<gtk::Label>,
        #[template_child]
        pub subtitle: TemplateChild<gtk::Label>,
        #[template_child]
        pub thumbnail_container: TemplateChild<gtk::CenterBox>,
        pub thumbnail: std::cell::RefCell<Option<ThumbnailImage>>,
        pub info: std::cell::RefCell<Option<TemplateInfo>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TemplateRow {
        const NAME: &'static str = "TemplateRow";
        type Type = super::TemplateRow;
        type ParentType = gtk::ListBoxRow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TemplateRow {}
    impl WidgetImpl for TemplateRow {}
    impl ListBoxRowImpl for TemplateRow {}
}

glib::wrapper! {
    pub struct TemplateRow(ObjectSubclass<imp::TemplateRow>)
        @extends gtk::Widget, gtk::ListBoxRow,
        @implements gtk::Buildable, gtk::ConstraintTarget, gtk::Accessible, gtk::Orientable, gtk::Actionable, gtk::ShortcutManager;
}

impl TemplateRow {
    pub fn new(info: TemplateInfo) -> Self {
        let obj: Self = glib::Object::new();
        {
            let imp = obj.imp();

            imp.title.set_label(info.display_name_or_name());

            let subtitle = info.package.description.clone().unwrap_or_default();
            imp.subtitle.set_label(&subtitle);

            // Hide the thumbnail container by default; shown only if a
            // texture is successfully loaded asynchronously.
            imp.thumbnail_container.set_visible(false);

            if let Some(thumb_path) = info.thumbnail_path.clone() {
                let obj_weak = obj.downgrade();
                glib::spawn_future_local(async move {
                    // Read the file bytes off the main thread
                    let bytes = gio::spawn_blocking(move || std::fs::read(&thumb_path)).await;

                    let Ok(Ok(bytes)) = bytes else { return };

                    // Construct the texture on the main thread
                    let bytes = glib::Bytes::from_owned(bytes);
                    let Ok(texture) = gdk::Texture::from_bytes(&bytes) else {
                        return;
                    };

                    // Apply to the widget if it still exists
                    let Some(obj) = obj_weak.upgrade() else {
                        return;
                    };
                    let imp = obj.imp();
                    let thumb = ThumbnailImage::new();
                    thumb.set_from_texture(texture);
                    thumb.set_halign(gtk::Align::Center);
                    thumb.set_valign(gtk::Align::Center);
                    imp.thumbnail_container.set_center_widget(Some(&thumb));
                    imp.thumbnail_container.set_visible(true);
                    imp.thumbnail.replace(Some(thumb));
                });
            }

            imp.info.replace(Some(info));
        }
        obj
    }

    pub fn get_info(&self) -> Option<TemplateInfo> {
        self.imp().info.borrow().clone()
    }

    pub fn matches_filter(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let query = query.to_lowercase();
        if let Some(info) = self.imp().info.borrow().as_ref() {
            if info.package.name.to_lowercase().contains(&query) {
                return true;
            }
            if let Some(desc) = &info.package.description
                && desc.to_lowercase().contains(&query)
            {
                return true;
            }
        }
        false
    }
}
