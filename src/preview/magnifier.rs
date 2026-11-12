// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::{Snapshot, gdk, graphene, gsk, prelude::*, subclass::prelude::*};
use std::cell::{Cell, RefCell};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PreviewMagnifier {
        pub center_x: Cell<f64>,
        pub center_y: Cell<f64>,
        pub radius: Cell<f64>,
        pub texture: RefCell<Option<gdk::Texture>>,
        pub source_x: Cell<f64>,
        pub source_y: Cell<f64>,
        pub zoom_factor: Cell<f64>,
        pub invert_lightness: Cell<bool>,
        // Colorblind-simulation mode, applied at paint time via a GPU
        // color matrix - see snapshot(). Mirrors PreviewImage's cb_mode;
        // there's no shared base widget to hang this off of, so it's
        // duplicated rather than factored out.
        pub cb_mode: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreviewMagnifier {
        const NAME: &'static str = "PreviewMagnifier";
        type Type = super::PreviewMagnifier;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for PreviewMagnifier {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // Set default radius (about 200 pixels)
            self.radius.set(200.0);

            // Widget should not receive input events
            obj.set_can_target(false);
        }

        fn dispose(&self) {
            // Take ownership and explicitly drop the texture
            if let Some(texture) = self.texture.borrow_mut().take() {
                drop(texture);
            }

            // Reset all state
            self.center_x.set(0.0);
            self.center_y.set(0.0);
            self.radius.set(0.0);
            self.source_x.set(0.0);
            self.source_y.set(0.0);
            self.zoom_factor.set(1.0);
            self.invert_lightness.set(false);
            self.cb_mode.replace(String::new());
        }
    }

    /// Converts one of crate::utils's 3x3 RGB color matrices (plus an
    /// [R,G,B] offset) into the (color_matrix, color_offset) pair
    /// gtk_snapshot_push_color_matrix() expects. See image.rs's copy of
    /// this same helper for the derivation - duplicated here since there's
    /// no shared base widget between PreviewImage and PreviewMagnifier to
    /// hang it off of.
    fn graphene_color_matrix(
        m: crate::utils::Matrix,
        offset: crate::utils::Offset,
    ) -> (graphene::Matrix, graphene::Vec4) {
        let mut values = [0f32; 16];
        for row in 0..3 {
            for col in 0..3 {
                values[row * 4 + col] = m[col][row];
            }
        }
        values[3 * 4 + 3] = 1.0; // alpha passes through unchanged

        let matrix = graphene::Matrix::from_float(values);
        let offset = graphene::Vec4::new(offset[0], offset[1], offset[2], 0.0);
        (matrix, offset)
    }

    impl WidgetImpl for PreviewMagnifier {
        fn snapshot(&self, snapshot: &Snapshot) {
            let obj = self.obj();

            // Find GtkScrolledWindow and clip to it
            let clip_bounds = {
                let mut parent = obj.parent();
                let mut result = None;

                while let Some(widget) = parent {
                    if widget.type_().name() == "GtkOverlay" {
                        let mut child = widget.first_child();
                        while let Some(c) = child {
                            if c.type_().name() == "GtkScrolledWindow" {
                                let top_left = graphene::Point::new(0.0, 0.0);
                                let bottom_right =
                                    graphene::Point::new(c.width() as f32, c.height() as f32);

                                if let (Some(tl), Some(br)) = (
                                    c.compute_point(obj.as_ref(), &top_left),
                                    c.compute_point(obj.as_ref(), &bottom_right),
                                ) {
                                    result = Some(graphene::Rect::new(
                                        tl.x(),
                                        tl.y(),
                                        br.x() - tl.x(),
                                        br.y() - tl.y(),
                                    ));
                                }
                                break;
                            }
                            child = c.next_sibling();
                        }
                        break;
                    }
                    parent = widget.parent();
                }
                result
            };

            if let Some(clip_rect) = clip_bounds {
                snapshot.push_clip(&clip_rect);
            }

            let cx = self.center_x.get() as f32;
            let cy = self.center_y.get() as f32;
            let radius = self.radius.get() as f32;

            if radius <= 0.0 {
                if clip_bounds.is_some() {
                    snapshot.pop();
                }
                return;
            }

            // Get the style manager to check dark mode
            let style_manager = adw::StyleManager::default();
            let is_dark = style_manager.is_dark();

            // Background colour: black if dark mode + invert lightness, otherwise white
            let bg_color = if self.invert_lightness.get() && is_dark {
                gdk::RGBA::new(0.0, 0.0, 0.0, 1.0)
            } else {
                gdk::RGBA::new(1.0, 1.0, 1.0, 1.0)
            };

            // Border color based on theme
            let border_color = if is_dark {
                // Dark mode: #2e2e32 (https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1-latest/css-variables.html#sidebar-colors)
                gdk::RGBA::new(
                    0x2e as f32 / 255.0,
                    0x2e as f32 / 255.0,
                    0x32 as f32 / 255.0,
                    1.0,
                )
            } else {
                // Light mode: #ebebed (https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1-latest/css-variables.html#sidebar-colors)
                gdk::RGBA::new(
                    0xeb as f32 / 255.0,
                    0xeb as f32 / 255.0,
                    0xed as f32 / 255.0,
                    1.0,
                )
            };

            // Shadow color and blur size
            let shadow_color = gdk::RGBA::new(0.0, 0.0, 0.0, 0.25);
            let shadow_blur = 10.0;

            let rect = graphene::Rect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0);

            let rounded = gsk::RoundedRect::new(
                rect,
                graphene::Size::new(radius, radius),
                graphene::Size::new(radius, radius),
                graphene::Size::new(radius, radius),
                graphene::Size::new(radius, radius),
            );

            // Push 1: shadow
            snapshot.push_shadow(&[gsk::Shadow::new(shadow_color, 0.0, 0.0, shadow_blur)]);

            // Magnifier border
            let outer_radius = radius;
            let border_rect = graphene::Rect::new(
                cx - outer_radius,
                cy - outer_radius,
                outer_radius * 2.0,
                outer_radius * 2.0,
            );
            let border_rounded = gsk::RoundedRect::new(
                border_rect,
                graphene::Size::new(outer_radius, outer_radius),
                graphene::Size::new(outer_radius, outer_radius),
                graphene::Size::new(outer_radius, outer_radius),
                graphene::Size::new(outer_radius, outer_radius),
            );

            // Push 2: border rounded clip
            snapshot.push_rounded_clip(&border_rounded);
            snapshot.append_color(&border_color, &border_rect);
            // Pop 2: border rounded clip
            snapshot.pop();

            // Push 3: content rounded clip
            snapshot.push_rounded_clip(&rounded);

            if let Some(texture) = self.texture.borrow().as_ref() {
                // Snap to the nearest whole physical pixel before use.
                // source_x/source_y come from a normalized cursor position
                // and are essentially continuous. Left as-is, they
                // would place the texture at a fractional physical-pixel
                // offset on almost every frame, which forces the
                // compositor to blend between texels rather than sample
                // one exactly, causing softness.
                let src_x = self.source_x.get().round() as f32;
                let src_y = self.source_y.get().round() as f32;
                let zoom = self.zoom_factor.get() as f32;

                snapshot.save();
                snapshot.translate(&graphene::Point::new(cx - radius, cy - radius));

                let page_shadow_color = gdk::RGBA::new(0.0, 0.0, 0.0, 0.25);
                let page_shadow_blur = 10.0 * zoom;

                snapshot.save();

                // Get scale factor to convert physical pixels to logical pixels
                let scale_factor = obj.scale_factor() as f32;

                // Texture dimensions are in physical pixels
                let texture_width_physical = texture.width() as f32;
                let texture_height_physical = texture.height() as f32;

                // Convert to logical pixels for rendering
                let logical_width = texture_width_physical / scale_factor;
                let logical_height = texture_height_physical / scale_factor;

                // Source positions are in physical pixels, convert to logical
                let src_x_logical = src_x / scale_factor;
                let src_y_logical = src_y / scale_factor;

                snapshot.translate(&graphene::Point::new(
                    radius - src_x_logical,
                    radius - src_y_logical,
                ));

                let shadow_expansion = page_shadow_blur / 2.0;

                let page_shadow_rect = graphene::Rect::new(
                    -shadow_expansion,
                    -shadow_expansion,
                    logical_width + shadow_expansion * 2.0,
                    logical_height + shadow_expansion * 2.0,
                );

                // Push 4: blur
                snapshot.push_blur(page_shadow_blur as f64);
                snapshot.append_color(&page_shadow_color, &page_shadow_rect);
                // Pop 4: blur
                snapshot.pop();

                // Colorblind simulation and lightness-invert, applied the
                // same way as the main preview (see PreviewImage::snapshot())
                // - at paint time via a single GPU color matrix, instead of
                // baking either into the texture when the lens re-renders.
                let transform = crate::utils::combined_transform(
                    self.invert_lightness.get(),
                    &self.cb_mode.borrow(),
                );
                if let Some((m, offset)) = transform {
                    let (color_matrix, color_offset) = graphene_color_matrix(m, offset);
                    snapshot.push_color_matrix(&color_matrix, &color_offset);
                }

                // Render texture at logical size (GTK will handle the scaling)
                snapshot.append_texture(
                    texture,
                    &graphene::Rect::new(0.0, 0.0, logical_width, logical_height),
                );

                if transform.is_some() {
                    snapshot.pop();
                }

                snapshot.restore();
                snapshot.restore();
            } else {
                snapshot.append_color(&bg_color, &rect);
            }

            // Pop 3: content rounded clip
            snapshot.pop();
            // Pop 1: shadow
            snapshot.pop();

            // Pop outer clip if we pushed it
            if clip_bounds.is_some() {
                snapshot.pop();
            }
        }

        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }
    }
}

glib::wrapper! {
    pub struct PreviewMagnifier(ObjectSubclass<imp::PreviewMagnifier>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PreviewMagnifier {
    pub fn new(invert_lightness: bool) -> Self {
        let obj: Self = gtk::glib::Object::new();
        obj.imp().invert_lightness.set(invert_lightness);
        obj
    }

    pub fn set_position(&self, x: f64, y: f64) {
        let imp = self.imp();
        imp.center_x.set(x);
        imp.center_y.set(y);
        self.queue_draw();
    }

    pub fn set_radius(&self, radius: f64) {
        self.imp().radius.set(radius);
        self.queue_draw();
    }

    pub fn radius(&self) -> f64 {
        self.imp().radius.get()
    }

    pub fn set_texture(&self, texture: Option<gdk::Texture>) {
        self.imp().texture.replace(texture);
        self.queue_draw();
    }

    pub fn set_source_position(&self, x: f64, y: f64) {
        let imp = self.imp();

        // Source positions are in physical pixels (from texture coordinates)
        // They will be converted to logical pixels in snapshot()
        imp.source_x.set(x);
        imp.source_y.set(y);
        self.queue_draw();
    }

    pub fn set_zoom_factor(&self, factor: f64) {
        self.imp().zoom_factor.set(factor);
        self.queue_draw();
    }

    /// Sets the colorblind-simulation mode applied at paint time.
    pub fn set_cb_mode(&self, mode: &str) {
        self.imp().cb_mode.replace(mode.to_string());
        self.queue_draw();
    }

    /// Updates whether the lens texture is lightness-inverted. Unlike the
    /// constructor's invert_lightness parameter (only ever read once, at
    /// activation), this lets an already-active magnifier stay in sync if
    /// the setting changes while it's open.
    pub fn set_invert_lightness(&self, invert: bool) {
        self.imp().invert_lightness.set(invert);
        self.queue_draw();
    }
}

impl Default for PreviewMagnifier {
    fn default() -> Self {
        Self::new(false)
    }
}
