// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::{gdk, glib, glib::subclass::prelude::*, prelude::*, subclass::prelude::*};
use std::cell::{Cell, RefCell};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ThumbnailImage {
        pub texture: RefCell<Option<gdk::Texture>>,
        pub aspect_ratio: Cell<f32>, // height / width
        pub max_size: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ThumbnailImage {
        const NAME: &'static str = "ThumbnailImage";
        type Type = super::ThumbnailImage;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ThumbnailImage {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("template-thumbnail");
            obj.set_halign(gtk::Align::Center);
            obj.set_valign(gtk::Align::Center);
            self.max_size.set(80);
        }
    }

    impl WidgetImpl for ThumbnailImage {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let aspect_ratio = self.aspect_ratio.get();
            let max_size = self.max_size.get();

            if aspect_ratio <= 0.0 {
                return (0, 0, -1, -1);
            }

            match orientation {
                gtk::Orientation::Horizontal => {
                    let natural_w = if aspect_ratio > 1.0 {
                        ((max_size as f32) / aspect_ratio).round() as i32
                    } else {
                        max_size
                    };
                    (0, natural_w, -1, -1)
                }
                gtk::Orientation::Vertical => {
                    let width = if for_size > 0 { for_size } else { max_size };
                    let natural_h = (width as f32 * aspect_ratio).round() as i32;
                    let natural_h = natural_h.min(max_size);
                    (0, natural_h, -1, -1)
                }
                _ => unreachable!(),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let texture = self.texture.borrow();
            let Some(texture) = texture.as_ref() else {
                return;
            };

            let w = obj.width() as f64;
            let h = obj.height() as f64;

            texture.snapshot(snapshot, w, h);
        }
    }
}

glib::wrapper! {
    pub struct ThumbnailImage(ObjectSubclass<imp::ThumbnailImage>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ThumbnailImage {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_from_texture(&self, texture: gdk::Texture) {
        let imp = self.imp();
        let w = texture.width() as f32;
        let h = texture.height() as f32;
        if w > 0.0 && h > 0.0 {
            imp.aspect_ratio.set(h / w);
        }
        imp.texture.replace(Some(texture));
        self.queue_resize();
        self.queue_draw();
    }

    pub fn set_max_size(&self, size: i32) {
        self.imp().max_size.set(size);
        self.queue_resize();
    }
}

impl Default for ThumbnailImage {
    fn default() -> Self {
        Self::new()
    }
}
