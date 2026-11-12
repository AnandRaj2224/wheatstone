// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::ObjectExt;
use gtk::glib::{
    self,
    subclass::{Signal, prelude::*},
};
use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

use crate::typst_system::PageMetadata;

mod imp {
    use super::*;

    pub struct PreviewObject {
        pub generation: Cell<u64>,
        pub page_number: Cell<usize>,
        pub dimensions: Cell<(f32, f32)>, // (width_pt, height_pt)
        pub texture: RefCell<Option<gtk::gdk::MemoryTexture>>,
        pub is_rendering: Cell<bool>,
        pub needs_update: Cell<bool>,
    }

    impl Default for PreviewObject {
        fn default() -> Self {
            Self {
                generation: Cell::new(0),
                page_number: Cell::new(0),
                dimensions: Cell::new((0.0, 0.0)),
                texture: RefCell::new(None),
                is_rendering: Cell::new(false),
                needs_update: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreviewObject {
        const NAME: &'static str = "PreviewObject";
        type Type = super::PreviewObject;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for PreviewObject {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("texture-ready").build(),
                    Signal::builder("texture-cleanup").build(),
                ]
            })
        }

        fn dispose(&self) {
            // Explicitly drop texture reference
            let _texture = self.texture.borrow_mut().take();
            drop(_texture);

            self.generation.set(0);
            self.page_number.set(0);
            self.dimensions.set((0.0, 0.0));
            self.needs_update.set(false);
        }
    }
}

glib::wrapper! {
    pub struct PreviewObject(ObjectSubclass<imp::PreviewObject>);
}

impl PreviewObject {
    pub fn new(metadata: &PageMetadata, generation: u64) -> Self {
        let obj: Self = glib::Object::new();
        let imp = obj.imp();
        imp.generation.set(generation);
        imp.page_number.set(metadata.page_number);
        imp.dimensions.set(metadata.dimensions);
        imp.needs_update.set(false);

        obj
    }

    pub fn generation(&self) -> u64 {
        self.imp().generation.get()
    }

    pub fn set_generation(&self, generation: u64) {
        self.imp().generation.set(generation);
    }

    pub fn has_texture(&self) -> bool {
        self.imp().texture.borrow().is_some()
    }

    pub fn get_texture(&self) -> Option<gtk::gdk::MemoryTexture> {
        self.imp().texture.borrow().clone()
    }

    pub fn set_texture(&self, texture: gtk::gdk::MemoryTexture) {
        let imp = self.imp();

        // Take old texture out and drop it explicitly
        let old_texture = imp.texture.borrow_mut().replace(texture);

        // Explicitly drop old texture
        if let Some(old) = old_texture {
            drop(old);
        }

        // Clear needs_update flag when new texture is set
        imp.needs_update.set(false);

        self.emit_by_name::<()>("texture-ready", &[]);
    }

    pub fn clear_texture(&self) {
        // Take and explicitly drop old reference
        let old = self.imp().texture.borrow_mut().take();
        if let Some(texture) = old {
            drop(texture);
        }

        self.emit_by_name::<()>("texture-cleanup", &[]);
    }

    pub fn is_rendering(&self) -> bool {
        self.imp().is_rendering.get()
    }

    pub fn set_rendering(&self, rendering: bool) {
        self.imp().is_rendering.set(rendering);
    }

    pub fn cleanup(&self) {
        let imp = self.imp();

        // Take and explicitly drop texture
        let texture = imp.texture.borrow_mut().take();
        if let Some(tex) = texture {
            drop(tex);
        }

        imp.generation.set(0);
        imp.page_number.set(0);
        imp.dimensions.set((0.0, 0.0));
        imp.needs_update.set(false);
    }

    pub fn cleanup_texture(&self) {
        self.clear_texture();
    }

    pub fn page_number(&self) -> usize {
        self.imp().page_number.get()
    }

    pub fn dimensions(&self) -> (f32, f32) {
        self.imp().dimensions.get()
    }

    pub fn needs_update(&self) -> bool {
        self.imp().needs_update.get()
    }

    pub fn set_needs_update(&self, needs_update: bool) {
        self.imp().needs_update.set(needs_update);
    }
}
