// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::AnimationExt;
use gtk::{Snapshot, gdk, glib, glib::subclass::prelude::*, prelude::*, subclass::prelude::*};
use std::cell::{Cell, RefCell};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PreviewImage {
        // Page dimensions in points so pages can be sized correctly relative to each other.
        pub width_pt: Cell<f32>,
        pub height_pt: Cell<f32>,
        pub page_number: Cell<usize>,
        pub ppi: Cell<u32>,
        pub invert_bg: Cell<bool>,
        // Colorblind-simulation mode ("none" / "deuteranopia" / etc.),
        // applied at paint time via a GPU color matrix - see snapshot().
        // Unlike invert_bg (background placeholder color only), this
        // drives the actual texture's colors, replacing what used to be
        // baked in on the CPU by crate::utils::apply_transforms.
        pub cb_mode: RefCell<String>,
        // Lightness-invert, also applied at paint time via a GPU color
        // matrix now (a luma-invert approximation, not bit-identical to
        // crate::utils::apply_transforms's exact HSL version - see
        // crate::utils::combined_transform's doc comment). Separate from
        // invert_bg, which only colors the no-texture-yet placeholder.
        pub invert: Cell<bool>,
        pub texture_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub cleanup_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub current_page: RefCell<Option<glib::WeakRef<crate::preview::PreviewObject>>>,
        pub ripple_x: Cell<f64>,
        pub ripple_y: Cell<f64>,
        pub ripple_progress: Cell<f64>,
        pub ripple_animation: RefCell<Option<adw::TimedAnimation>>,
        pub magnifier_activated: Cell<bool>,
        pub resize_timeout: RefCell<Option<glib::SourceId>>,
        pub last_allocated_width: Cell<i32>,
        pub last_scale_factor: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreviewImage {
        const NAME: &'static str = "PreviewImage";
        type Type = super::PreviewImage;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for PreviewImage {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            obj.add_css_class("preview_picture");
            // Deliberately NOT hexpand/Fill: the widget's natural size (from
            // measure(), below) is the page's true size at the shared
            // document scale, so it should sit at that size and be centered
            // in whatever width the row gives it, not stretched to fill it.
            obj.set_hexpand(false);
            obj.set_vexpand(false);
            obj.set_halign(gtk::Align::Center);
            obj.set_valign(gtk::Align::Start);

            // Unified click gesture: hold for magnifier, quick click for jump to source
            let click = gtk::GestureClick::builder().button(1).build();

            // Allow more mouse movement during click (default is too restrictive, this lets users click even if their hand isn't perfectly steady)
            click.set_touch_only(false);

            // Track press start
            click.connect_pressed(glib::clone! {
                #[weak] obj,
                move |_gesture, _n_press, _x, _y| {
                    let imp = obj.imp();
                    imp.magnifier_activated.set(false);
                }
            });

            click.connect_released(glib::clone! {
                #[weak] obj,
                move |_gesture, _n_press, x, y| {
                    let imp = obj.imp();

                    // Restore cursor if we had hidden it
                    if imp.magnifier_activated.get() {
                        obj.set_cursor_from_name(None);
                    }

                    if let Some(preview) = obj.find_preview_parent() {
                        // If magnifier was activated, just deactivate it
                        if imp.magnifier_activated.get() {
                            preview.deactivate_magnifier();
                            imp.magnifier_activated.set(false);
                        }
                        // Otherwise, this was a quick click, so do jump to source
                        else {
                            obj.handle_jump_to_source(x, y);
                        }
                    }
                }
            });

            // Cancel only happens if GTK invalidates the gesture (e.g., another gesture takes priority)
            // We still restore cursor and deactivate magnifier if it was active
            click.connect_cancel(glib::clone! {
                #[weak] obj,
                move |_gesture, _sequence| {
                    let imp = obj.imp();

                    // Restore cursor if we hid it
                    if imp.magnifier_activated.get() {
                        obj.set_cursor_from_name(None);
                    }

                    if let Some(preview) = obj.find_preview_parent()
                        && imp.magnifier_activated.get() {
                            preview.deactivate_magnifier();
                            imp.magnifier_activated.set(false);
                        }
                }
            });

            obj.add_controller(click);

            // Long press gesture for magnifier activation
            let long_press = gtk::GestureLongPress::builder().button(1).build();

            long_press.connect_pressed(glib::clone! {
                #[weak] obj,
                move |_gesture, x, y| {
                    let imp = obj.imp();

                    // Hide cursor for magnifier mode
                    obj.set_cursor_from_name(Some("none"));
                    imp.magnifier_activated.set(true);

                    if let Some(preview) = obj.find_preview_parent() {
                        let page_num = imp.page_number.get();

                        // Convert to normalized coordinates
                        let (norm_x, norm_y) = obj.widget_coords_to_normalized(x, y);

                        // Convert to preview list coordinates
                        if let Some((list_x, list_y)) = obj.widget_coords_to_preview_list(x, y) {
                            preview.activate_magnifier(
                                page_num,
                                list_x,
                                list_y,
                                norm_x,
                                norm_y,
                            );
                        }
                    }
                }
            });

            obj.add_controller(long_press);

            // Motion controller: update magnifier position when active
            let motion = gtk::EventControllerMotion::new();

            motion.connect_motion(glib::clone! {
                #[weak] obj,
                move |_ctrl, x, y| {
                    let imp = obj.imp();

                    // Only update if magnifier is active
                    if !imp.magnifier_activated.get() {
                        return;
                    }

                    if let Some(preview) = obj.find_preview_parent() {
                        // Check if magnifier exists
                        if preview.imp().magnifier.borrow().is_none() {
                            return;
                        }

                        // Convert to normalized coordinates
                        let (norm_x, norm_y) = obj.widget_coords_to_normalized(x, y);

                        // Convert to preview list coordinates
                        if let Some((list_x, list_y)) = obj.widget_coords_to_preview_list(x, y) {
                            preview.update_magnifier_position(
                                list_x,
                                list_y,
                                norm_x,
                                norm_y,
                            );
                        }
                    }
                }
            });
            obj.add_controller(motion);

            // Scroll: deactivate magnifier
            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::VERTICAL
                    | gtk::EventControllerScrollFlags::HORIZONTAL,
            );

            scroll.connect_scroll(glib::clone! {
                #[weak] obj,
                #[upgrade_or] glib::Propagation::Proceed,
                move |_ctrl, _dx, _dy| {
                    let imp = obj.imp();

                    // Deactivate magnifier and restore cursor if active
                    if imp.magnifier_activated.get() {
                        obj.set_cursor_from_name(None);
                        imp.magnifier_activated.set(false);

                        if let Some(preview) = obj.find_preview_parent() {
                            preview.deactivate_magnifier();
                        }
                    }

                    glib::Propagation::Proceed
                }
            });
            obj.add_controller(scroll);

            // Re-render when scale factor changes (moving between displays, etc.)
            obj.connect_notify_local(Some("scale-factor"), |preview_image, _| {
                let imp = preview_image.imp();
                let new_scale_factor = preview_image.scale_factor();

                // Store new scale factor
                imp.last_scale_factor.set(new_scale_factor);

                // Only re-render if we have a current page
                if let Some(weak_page) = imp.current_page.borrow().as_ref()
                    && let Some(page) = weak_page.upgrade()
                    && page.has_texture()
                {
                    // Clear texture and request re-render from parent
                    page.clear_texture();
                    preview_image.request_parent_visibility_check();
                }
            });
        }

        fn dispose(&self) {
            // Clean up signal handlers during disposal
            if let Some(handler_id) = self.texture_handler.take()
                && let Some(weak_page) = self.current_page.borrow().as_ref()
                && let Some(page) = weak_page.upgrade()
            {
                page.disconnect(handler_id);
            }
            if let Some(handler_id) = self.cleanup_handler.take()
                && let Some(weak_page) = self.current_page.borrow().as_ref()
                && let Some(page) = weak_page.upgrade()
            {
                page.disconnect(handler_id);
            }
            self.current_page.take();

            // Stop ripple animation
            if let Some(anim) = self.ripple_animation.take() {
                anim.pause();
                drop(anim);
            }
        }
    }

    /// Converts one of crate::utils's 3x3 RGB color matrices (plus an
    /// [R,G,B] offset - zero for pure colorblind simulation, nonzero for
    /// the luma-invert transform, see crate::utils::combined_transform)
    /// into the (color_matrix, color_offset) pair
    /// gtk_snapshot_push_color_matrix() expects.
    fn graphene_color_matrix(
        m: crate::utils::Matrix,
        offset: crate::utils::Offset,
    ) -> (gtk::graphene::Matrix, gtk::graphene::Vec4) {
        let mut values = [0f32; 16];
        for row in 0..3 {
            for col in 0..3 {
                values[row * 4 + col] = m[col][row];
            }
        }
        values[3 * 4 + 3] = 1.0; // alpha passes through unchanged

        let matrix = gtk::graphene::Matrix::from_float(values);
        let offset = gtk::graphene::Vec4::new(offset[0], offset[1], offset[2], 0.0);
        (matrix, offset)
    }

    impl WidgetImpl for PreviewImage {
        /// The page's size is computed directly from its point
        /// dimensions and the shared document scale rather than
        /// derived from whatever width the parent happens to allocate.
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        /// Called when GTK allocates size to the widget
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let obj = self.obj();

            // Call parent implementation first
            self.parent_size_allocate(width, height, baseline);

            let last_width = self.last_allocated_width.get();
            let scale_factor = obj.scale_factor();
            let last_scale = self.last_scale_factor.get();

            self.last_allocated_width.set(width);
            self.last_scale_factor.set(scale_factor);

            // If width or scale hasn't actually changed, skip.
            if width == last_width && scale_factor == last_scale {
                return;
            }

            // last_scale_factor's default (0) is never a real GTK scale
            // factor (always >= 1), so it doubles as a "never allocated
            // before" sentinel - no separate flag needed. Without this,
            // every widget's first-ever layout looked identical to a real
            // resize (since 0 != any real width/scale), which scheduled a
            // full cache-clear + rerender 200ms after every window open,
            // duplicating the render settle_render_scale() already
            // performed correctly.
            if last_scale == 0 {
                return;
            }

            // Find preview parent and use its shared timeout
            if let Some(preview) = obj.find_preview_parent() {
                let preview_imp = preview.imp();

                // Cancel any pending resize timeout (shared across all widgets)
                if let Some(timeout_id) = preview_imp.width_change_timeout.take() {
                    timeout_id.remove();
                }

                // Schedule re-render after resize settles (200ms delay)
                let page_number = self.page_number.get();
                let timeout_id = glib::timeout_add_local_once(
                    std::time::Duration::from_millis(200),
                    glib::clone!(
                        #[weak]
                        preview,
                        move || {
                            preview.imp().width_change_timeout.take();
                            let _ = page_number;

                            // Clear cache so old-sized textures can't be reused
                            preview.texture_cache().clear();

                            // Mark all pages as needing update so they'll re-render
                            let page_store = &preview.imp().page_store;
                            for i in 0..page_store.n_items() {
                                if let Some(obj) = page_store.item(i)
                                    && let Ok(page) =
                                        obj.downcast::<crate::preview::PreviewObject>()
                                {
                                    page.set_needs_update(true);
                                }
                            }

                            // Trigger re-render of visible pages
                            preview.check_visible_pages("image.rs:320");
                        }
                    ),
                );

                preview_imp.width_change_timeout.replace(Some(timeout_id));
            }
        }

        /// Called when GTK draws the widget (snapshot phase)
        fn snapshot(&self, snapshot: &Snapshot) {
            let obj = self.obj();

            let allocated_width = obj.width();
            let allocated_height = obj.height();

            if allocated_width <= 0 || allocated_height <= 0 {
                return;
            }

            // Always draw background first to prevent flicker while texture is loading
            let bg_color = if self.invert_bg.get() {
                gdk::RGBA::new(0.0, 0.0, 0.0, 1.0)
            } else {
                gdk::RGBA::new(1.0, 1.0, 1.0, 1.0)
            };
            snapshot.append_color(
                &bg_color,
                &gtk::graphene::Rect::new(
                    0.0,
                    0.0,
                    allocated_width as f32,
                    allocated_height as f32,
                ),
            );

            // Get texture from PreviewObject on demand; no storage in this widget
            let texture = if let Some(weak_page) = self.current_page.borrow().as_ref() {
                weak_page.upgrade().and_then(|page| page.get_texture())
            } else {
                None
            };

            let Some(texture) = texture else {
                return;
            };

            // Colorblind simulation and lightness-invert are both applied
            // here, at paint time, via a single GPU color matrix rather
            // than baked into the texture's pixels. This means
            // toggling either mode is a plain queue_draw(), no recompile,
            // no cache invalidation.
            let transform =
                crate::utils::combined_transform(self.invert.get(), &self.cb_mode.borrow());

            if let Some((m, offset)) = transform {
                let (color_matrix, color_offset) = graphene_color_matrix(m, offset);
                snapshot.push_color_matrix(&color_matrix, &color_offset);
            }

            // Draw at exact integer dimensions
            // GTK will automatically handle the scale factor for us
            texture.snapshot(snapshot, allocated_width as f64, allocated_height as f64);

            if transform.is_some() {
                snapshot.pop();
            }

            // Draw ripple effect on top
            let progress = self.ripple_progress.get();
            if progress > 0.0 && progress < 1.0 {
                let ripple_x = self.ripple_x.get() as f32;
                let ripple_y = self.ripple_y.get() as f32;

                // Calculate ripple size with ease-out cubic
                let eased_progress = {
                    let t = progress as f32;
                    let t = t - 1.0;
                    t * t * t + 1.0
                };

                // Small ripple: ~1cm at 96 DPI = ~38 pixels
                let max_radius = 38.0;
                let current_radius = max_radius * eased_progress;

                // Fade out quickly as it expands
                let alpha = (1.0 - eased_progress).powf(2.0) * 0.4;

                // Get system accent color using libadwaita
                let style_manager = adw::StyleManager::default();
                let accent_color = style_manager.accent_color_rgba();

                // Draw ripple circle with accent color and fade
                let color = gdk::RGBA::new(
                    accent_color.red(),
                    accent_color.green(),
                    accent_color.blue(),
                    alpha,
                );
                let rect = gtk::graphene::Rect::new(
                    ripple_x - current_radius,
                    ripple_y - current_radius,
                    current_radius * 2.0,
                    current_radius * 2.0,
                );

                let rounded = gtk::gsk::RoundedRect::new(
                    rect,
                    gtk::graphene::Size::new(current_radius, current_radius),
                    gtk::graphene::Size::new(current_radius, current_radius),
                    gtk::graphene::Size::new(current_radius, current_radius),
                    gtk::graphene::Size::new(current_radius, current_radius),
                );

                snapshot.push_rounded_clip(&rounded);
                snapshot.append_color(&color, &rect);
                snapshot.pop();
            }
        }

        /// Provides size measurements for layout.
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let width_pt = self.width_pt.get();
            let height_pt = self.height_pt.get();

            // If no valid dimensions yet, widget takes no space
            if width_pt <= 0.0 || height_pt <= 0.0 {
                return (0, 0, -1, -1);
            }

            let obj = self.obj();
            let ppp = obj.effective_pixels_per_point();

            match orientation {
                gtk::Orientation::Horizontal => {
                    let width_px = ((width_pt as f64 * ppp).round().max(1.0)) as i32;
                    (width_px, width_px, -1, -1)
                }
                gtk::Orientation::Vertical => {
                    let height_px = ((height_pt as f64 * ppp).round().max(1.0)) as i32;
                    (height_px, height_px, -1, -1)
                }
                _ => unreachable!(),
            }
        }
    }
}

glib::wrapper! {
    pub struct PreviewImage(ObjectSubclass<imp::PreviewImage>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PreviewImage {
    /// Create a new instance of PreviewImage widget
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Connect to a PreviewObject's signals
    pub fn connect_to_page(&self, page: &crate::preview::PreviewObject) {
        let imp = self.imp();

        // Disconnect any existing handlers first to prevent accumulation
        self.disconnect_from_page();

        // Store weak reference to page
        imp.current_page.replace(Some(page.downgrade()));

        // Connect texture-ready handler (triggers redraw when texture becomes available)
        let texture_handler_id = page.connect_closure(
            "texture-ready",
            false,
            glib::closure_local!(
                #[weak(rename_to = preview_image)]
                self,
                move |_page: &crate::preview::PreviewObject| {
                    // Just queue a redraw, we'll fetch texture in snapshot()
                    preview_image.queue_draw();
                }
            ),
        );

        // Connect texture-cleanup handler (triggers redraw when texture is removed)
        let cleanup_handler_id = page.connect_closure(
            "texture-cleanup",
            false,
            glib::closure_local!(
                #[weak(rename_to = preview_image)]
                self,
                move |_page: &crate::preview::PreviewObject| {
                    // Just queue a redraw - we'll show white background in snapshot()
                    preview_image.queue_draw();
                }
            ),
        );

        #[cfg(debug_assertions)]
        {
            let page_num = imp.page_number.get();
            println!(
                "PreviewImage: Connected handlers for page {} (handler_ids: {:?}, {:?})",
                page_num, texture_handler_id, cleanup_handler_id
            );
        }

        // Store both handlers (moved after debug print)
        imp.texture_handler.replace(Some(texture_handler_id));
        imp.cleanup_handler.replace(Some(cleanup_handler_id));

        // If texture already exists, queue initial draw
        if page.has_texture() {
            self.queue_draw();
        }
    }

    /// Disconnect from the PreviewObject's signals
    pub fn disconnect_from_page(&self) {
        let imp = self.imp();

        #[cfg(debug_assertions)]
        let page_num = imp.page_number.get();

        // Take the current page reference
        if let Some(weak_page) = imp.current_page.take() {
            if let Some(page) = weak_page.upgrade() {
                // Disconnect texture handler
                if let Some(handler_id) = imp.texture_handler.take() {
                    #[cfg(debug_assertions)]
                    println!(
                        "PreviewImage: Disconnecting texture_handler {:?} for page {}",
                        handler_id, page_num
                    );

                    page.disconnect(handler_id);
                }

                // Disconnect cleanup handler
                if let Some(handler_id) = imp.cleanup_handler.take() {
                    #[cfg(debug_assertions)]
                    println!(
                        "PreviewImage: Disconnecting cleanup_handler {:?} for page {}",
                        handler_id, page_num
                    );

                    page.disconnect(handler_id);
                }

                // Explicitly drop the page reference
                drop(page);
            } else {
                // Page was already dropped, just clear our handlers
                imp.texture_handler.take();
                imp.cleanup_handler.take();
            }
        } else {
            // No current page, just clear handlers
            imp.texture_handler.take();
            imp.cleanup_handler.take();
        }

        // Ensure weak reference is fully dropped
        imp.current_page.replace(None);
    }

    /// Clean up widget state (called on unbind/teardown)
    pub fn cleanup(&self) {
        let imp = self.imp();

        // Disconnect and clear handlers
        if let Some(handler_id) = imp.texture_handler.take()
            && let Some(weak_page) = imp.current_page.take()
            && let Some(page) = weak_page.upgrade()
        {
            page.disconnect(handler_id);
        }
        if let Some(handler_id) = imp.cleanup_handler.take()
            && let Some(weak_page) = imp.current_page.borrow().as_ref()
            && let Some(page) = weak_page.upgrade()
        {
            page.disconnect(handler_id);
        }

        // Stop any active ripple animation
        if let Some(anim) = imp.ripple_animation.take() {
            anim.pause();
        }

        // Clear current page reference
        imp.current_page.take();

        // Reset dimensions
        imp.width_pt.set(0.0);
        imp.height_pt.set(0.0);
    }

    /// Find the parent TypesetterPreview widget
    fn find_preview_parent(&self) -> Option<crate::preview::TypesetterPreview> {
        let mut parent = self.parent();
        while let Some(widget) = parent {
            if let Ok(preview) = widget
                .clone()
                .downcast::<crate::preview::TypesetterPreview>()
            {
                return Some(preview);
            }
            parent = widget.parent();
        }
        None
    }

    /// Set dimensions from page dimensions in points.
    pub fn set_dimensions(&self, width_pt: f32, height_pt: f32) {
        let imp = self.imp();

        if width_pt <= 0.0 || height_pt <= 0.0 {
            return;
        }

        let changed = (imp.width_pt.get() - width_pt).abs() > 0.001
            || (imp.height_pt.get() - height_pt).abs() > 0.001;

        // Only update if different (with small epsilon for floating point comparison)
        if changed {
            imp.width_pt.set(width_pt);
            imp.height_pt.set(height_pt);

            // Queue resize to update layout
            if self.is_realized() {
                self.queue_resize();
            }
        }
    }

    pub fn set_page_number(&self, page_number: usize) {
        self.imp().page_number.set(page_number);
    }

    pub fn set_ppi(&self, ppi: u32) {
        self.imp().ppi.set(ppi);
    }

    pub fn set_invert_bg(&self, invert: bool) {
        self.imp().invert_bg.set(invert);
    }

    /// Sets the colorblind-simulation mode applied at paint time. Does not
    /// itself trigger a redraw. Callers should queue_draw() the widget
    /// (this mirrors set_invert_bg, which has the same expectation).
    pub fn set_cb_mode(&self, mode: &str) {
        self.imp().cb_mode.replace(mode.to_string());
    }

    /// Sets whether the texture's lightness is inverted, applied at paint
    /// time (see the `invert` field's doc comment for the caveat about this
    /// being an approximation, not the exact CPU HSL version). Same
    /// no-auto-redraw expectation as set_cb_mode.
    pub fn set_invert(&self, invert: bool) {
        self.imp().invert.set(invert);
    }

    pub fn page_number(&self) -> usize {
        self.imp().page_number.get()
    }

    pub fn dimensions_pt(&self) -> (f32, f32) {
        let imp = self.imp();
        (imp.width_pt.get(), imp.height_pt.get())
    }

    pub fn ppi(&self) -> u32 {
        self.imp().ppi.get()
    }

    /// Trigger a ripple effect at the given widget coordinates
    fn trigger_ripple(&self, x: f64, y: f64) {
        let imp = self.imp();

        // Store ripple position
        imp.ripple_x.set(x);
        imp.ripple_y.set(y);
        imp.ripple_progress.set(0.0);

        // Stop any existing animation
        if let Some(anim) = imp.ripple_animation.take() {
            anim.pause();
        }

        // Create animation
        let target = adw::CallbackAnimationTarget::new(glib::clone!(
            #[weak(rename_to = preview_image)]
            self,
            move |value| {
                preview_image.imp().ripple_progress.set(value);
                preview_image.queue_draw();
            }
        ));

        let animation = adw::TimedAnimation::new(self, 0.0, 1.0, 1000, target);

        animation.set_easing(adw::Easing::EaseOutCubic);

        animation.connect_done(glib::clone!(
            #[weak(rename_to = preview_image)]
            self,
            move |_| {
                preview_image.imp().ripple_animation.take();
                preview_image.imp().ripple_progress.set(0.0);
            }
        ));

        imp.ripple_animation.replace(Some(animation.clone()));
        animation.play();
    }

    /// Look up the shared document scale (base fit-to-viewport scale times
    /// current zoom level) from the parent `TypesetterPreview`. This is what
    /// `measure()` uses to turn this page's point dimensions into a pixel
    /// size, so that all pages sit at the same physical scale rather than
    /// each being stretched to its row's width independently.
    fn effective_pixels_per_point(&self) -> f64 {
        self.find_preview_parent()
            .map(|preview| preview.effective_pixels_per_point())
            .unwrap_or(1.0)
    }

    /// Convert widget coordinates to normalized coordinates (0.0 to 1.0).
    fn widget_coords_to_normalized(&self, x: f64, y: f64) -> (f64, f64) {
        let widget_width = self.width() as f64;
        let widget_height = self.height() as f64;

        if widget_width <= 0.0 || widget_height <= 0.0 {
            return (0.0, 0.0);
        }

        // Convert to normalized coordinates (0.0 to 1.0)
        let norm_x = (x / widget_width).clamp(0.0, 1.0);
        let norm_y = (y / widget_height).clamp(0.0, 1.0);

        (norm_x, norm_y)
    }

    /// Convert widget coordinates to preview list coordinates (returns None if conversion fails)
    fn widget_coords_to_preview_list(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let widget_width = self.width() as f64;
        let widget_height = self.height() as f64;

        if widget_width <= 0.0 || widget_height <= 0.0 {
            return None;
        }

        // Clamp to valid widget bounds first
        let clamped_x = x.clamp(0.0, widget_width);
        let clamped_y = y.clamp(0.0, widget_height);

        // Find preview list parent
        let preview = self.find_preview_parent()?;

        // Transform to preview list coordinates
        let point = gtk::graphene::Point::new(clamped_x as f32, clamped_y as f32);
        let translated = self.compute_point(preview.preview_list(), &point)?;

        Some((translated.x() as f64, translated.y() as f64))
    }

    /// Handle click to jump to source
    fn handle_jump_to_source(&self, x: f64, y: f64) {
        let imp = self.imp();
        let page_number = imp.page_number.get();

        // Convert to normalized coordinates
        let (relative_x, relative_y) = self.widget_coords_to_normalized(x, y);

        // Store click position for potential ripple
        let click_x = x;
        let click_y = y;

        // Get the TypesetterPreview widget
        if let Some(preview) = self.find_preview_parent() {
            // Clone self for the async callback
            let self_clone = self.clone();

            // Request jump and trigger ripple only on success
            preview.jump_from_preview_with_callback(
                page_number,
                relative_x as f32,
                relative_y as f32,
                move |jump_result| {
                    // Trigger ripple only if we actually jumped to source
                    if matches!(jump_result, crate::typst_system::JumpResult::Source(_)) {
                        self_clone.trigger_ripple(click_x, click_y);
                    }
                },
            );
        }
    }

    fn request_parent_visibility_check(&self) {
        // Find the TypesetterPreview parent and trigger a visibility check
        let mut parent = self.parent();
        while let Some(widget) = parent {
            if let Ok(preview) = widget
                .clone()
                .downcast::<crate::preview::TypesetterPreview>()
            {
                glib::idle_add_local_once(glib::clone!(
                    #[weak]
                    preview,
                    move || {
                        preview.check_visible_pages("image.rs:806");
                    }
                ));
                return;
            }
            parent = widget.parent();
        }
    }
}

impl Default for PreviewImage {
    fn default() -> Self {
        Self::new()
    }
}
