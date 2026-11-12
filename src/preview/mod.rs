// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

mod cache;
mod compilation;
pub mod image;
mod magnifier;
pub mod object;

use adw::{prelude::*, subclass::prelude::*};
use gettextrs::gettext;
use gtk::{CompositeTemplate, glib, glib::Properties};
use std::cell::{Cell, OnceCell, RefCell};

pub use image::PreviewImage;
pub use magnifier::PreviewMagnifier;
pub use object::PreviewObject;

use cache::TextureCache;

const ZOOM_STEP: f64 = 1.25;
const REFERENCE_PIXELS_PER_POINT: f64 = 96.0 / 72.0;
const ZOOM_MIN_FACTOR: f64 = 0.25;
const ZOOM_MAX_FACTOR: f64 = 2.0;

/// Direct Rust port of `child_size_from_clamp()` in libadwaita's
/// adw-clamp-layout.c - the ease-out-cubic curve `AdwClampLayout` itself
/// uses to smoothly ease a child's allocated size between
/// `tightening_threshold` and `maximum_size`, rather than a hard cutoff.
///
/// We compute this ourselves instead of reading `preview_clamp`'s or
/// `preview_list`'s allocated width back out of the widget tree, because
/// `PreviewImage::measure()` reports a fixed (min == natural) width derived
/// from `base_pixels_per_point` - the very value this function feeds into.
/// Reading a widget's size back out of that tree would just echo last
/// frame's already-applied value, not an honest "how much room do you
/// actually have" signal.
///
/// `for_size` is the raw available width (`preview_scroller.width()`) -
/// this is what `AdwClamp` itself always receives, since it fills whatever
/// width its parent (a plain `GtkScrolledWindow`) gives it and only narrows
/// the box it hands to *its own* child.
fn clamp_eased_width(for_size: f64, maximum_size: f64, tightening_threshold: f64) -> f64 {
    // ADW_EASE_OUT_TAN_CUBIC in adw-clamp-layout.c.
    const EASE_OUT_TAN_CUBIC: f64 = 3.0;

    let lower = tightening_threshold.min(maximum_size).max(0.0);
    let max = lower.max(maximum_size);
    let upper = lower + EASE_OUT_TAN_CUBIC * (max - lower);

    if for_size <= lower {
        return for_size;
    }
    if for_size >= upper {
        return max.ceil();
    }

    let progress = (for_size - lower) / (upper - lower);
    let eased = 1.0 - (1.0 - progress).powi(3); // ease-out-cubic
    (lower + (max - lower) * eased).floor()
}

/// Anchor captured right when a zoom is requested (Ctrl+Scroll, toolbar
/// buttons, or the zoom percentage menu), so the document point under the
/// anchor can be kept fixed on screen once the resulting layout change has
/// actually settled.
pub(crate) struct ZoomAnchor {
    // The page number under the anchor point at capture time.
    page_number: usize,
    // Fractional position within that page's own allocation (0.0-1.0
    // across its width/height respectively), which stays meaningful
    // whatever size the page ends up at after the zoom.
    fx: f64,
    fy: f64,
    // Cursor/anchor position relative to the viewport (preview_scroller),
    // which doesn't change across the zoom and is what we want to keep
    // that fractional point of the page aligned with.
    viewport_x: f64,
    viewport_y: f64,
}

mod imp {
    use super::*;

    #[derive(CompositeTemplate, Properties)]
    #[properties(wrapper_type = super::TypesetterPreview)]
    #[template(resource = "/dev/whetstone/editor/gtk/preview.ui")]
    pub struct TypesetterPreview {
        pub(crate) action_group: OnceCell<gtk::gio::SimpleActionGroup>,
        #[template_child]
        pub(crate) diagnostics_list: TemplateChild<gtk::ListBox>,
        pub(crate) diagnostics_rows: RefCell<Vec<gtk::Widget>>,
        #[template_child]
        pub(crate) bottom_bar_label: TemplateChild<gtk::Box>,
        #[template_child]
        pub(crate) warnings_number: TemplateChild<gtk::Label>,
        #[template_child]
        pub(crate) preview_pane: TemplateChild<adw::BottomSheet>,
        pub(crate) parent_window: OnceCell<crate::window::TypesetterWindow>,
        pub(crate) editor_buffer: OnceCell<sourceview::Buffer>,
        pub(crate) view: OnceCell<crate::editor::view::TypesetterEditorView>,
        #[property(get, set)]
        pub(crate) error_count: Cell<u32>,
        #[property(get, set)]
        pub(crate) warning_count: Cell<u32>,
        #[template_child]
        pub(crate) preview_clamp: TemplateChild<adw::ClampScrollable>,
        #[template_child]
        pub(crate) preview_scroller: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub(crate) preview_list: TemplateChild<gtk::ListView>,
        #[template_child]
        pub(crate) zoom_in_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(crate) zoom_out_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(crate) reset_zoom_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(crate) zoom_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(crate) pane_overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub(crate) loading_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub(crate) loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub(crate) loading_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(crate) zoom_controls_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub(crate) zoom_info_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub(crate) zoom_level_button: TemplateChild<gtk::MenuButton>,
        // Document generation tracking
        pub(crate) current_generation: Cell<u64>,
        pub(crate) loading_timeout_id: RefCell<Option<glib::SourceId>>,
        // The zoom toolbar should stay visible if either of these is
        // true: just hovering, or an open popover launched from within it
        // (e.g. the zoom-level menu). See update_toolbar_visibility().
        pub(crate) is_hovering_content: Cell<bool>,
        pub(crate) zoom_menu_open: Cell<bool>,
        pub(crate) page_store: gtk::gio::ListStore,
        #[property(get, set, default = false)]
        pub(crate) is_visible: Cell<bool>,
        // Document-wide render scale. Every page is sized in pixels as
        // `point_dimension * effective_pixels_per_point()`. There are two
        // distinct modes (like "Fit Width" vs. a locked zoom % in most
        // document viewers): Automatic (`is_auto_fit == true`, the default)
        // and Manual (`is_auto_fit == false`, entered by zooming in/out).
        pub(crate) base_pixels_per_point: Cell<f64>,
        pub(crate) manual_pixels_per_point: Cell<f64>,
        pub(crate) is_auto_fit: Cell<bool>,
        // The clamp's configured "optimal reading width" cap (from
        // preview.ui's maximum-size), captured at startup. Automatic
        // mode uses this as the fit-to-width target regardless of what the
        // clamp's maximum-size is currently set to. See
        // `update_clamp_for_mode` and `recompute_base_scale`.
        pub(crate) auto_fit_clamp_max: Cell<i32>,
        // Weak refs to the currently-bound PreviewImage widgets, so that a
        // zoom or base-scale change can explicitly queue_resize() them.
        // GTK has no way to know on its own that measure() would now return
        // something different, since nothing here is a GObject property.
        pub(crate) bound_images: RefCell<Vec<glib::WeakRef<PreviewImage>>>,
        // Viewport tracking for lazy rendering
        pub(crate) visible_pages: RefCell<std::collections::HashSet<usize>>,
        pub(crate) scroll_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub(crate) resize_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub(crate) last_viewport_check: Cell<i64>,
        // When a check_visible_pages() call gets throttled below, this
        // holds the follow-up timer that guarantees it still runs once the
        // throttle window closes, instead of silently dropping it. See
        // check_visible_pages(). Only ever holds one pending timer at a
        // time: further throttled calls in the same window see it's
        // already Some and don't schedule a second one.
        pub(crate) pending_viewport_recheck: RefCell<Option<glib::SourceId>>,
        // The vadjustment/hadjustment instance we last attached
        // value-changed/page-size/upper listeners to, so that a repeat
        // `notify::vadjustment` / `notify::hadjustment` firing for the same
        // underlying object (which GTK does not guarantee to suppress) is a
        // no-op instead of stacking a duplicate set of listeners. See
        // `bind_vadjustment_listeners` / `bind_hadjustment_listeners`.
        pub(crate) bound_vadjustment: RefCell<Option<glib::WeakRef<gtk::Adjustment>>>,
        pub(crate) bound_hadjustment: RefCell<Option<glib::WeakRef<gtk::Adjustment>>>,
        // Texture cache for memory management
        pub(crate) texture_cache: TextureCache,
        // Magnifier inspection tool
        pub(crate) magnifier: RefCell<Option<PreviewMagnifier>>,
        // Fade in/out animation for the magnifier. See activate_magnifier
        // and deactivate_magnifier. Kept separate from `magnifier` itself.
        // During fade-out, `magnifier` deliberately stays Some (the widget
        // is still visually present, just fading) until the animation's
        // `connect_done` tears it down.
        pub(crate) magnifier_fade_animation: RefCell<Option<adw::TimedAnimation>>,
        // Kinetic scrolling state
        pub(crate) drag_ongoing: Cell<bool>,
        pub(crate) drag_buffer: RefCell<[(f64, f64); 10]>,
        pub(crate) drag_buffer_index: Cell<usize>,
        pub(crate) drag_start_adj: Cell<(f64, f64)>,
        pub(crate) drag_start_pos: Cell<(f64, f64)>,
        pub(crate) drag_momentum: Cell<(f64, f64)>,
        pub(crate) drag_buffer_update_signal: RefCell<Option<glib::SourceId>>,
        pub(crate) drag_released_motion_signal: RefCell<Option<glib::SourceId>>,
        // Width change monitoring
        pub(crate) width_change_timeout: RefCell<Option<glib::SourceId>>,
        // Ctrl+Scroll zoom: last known pointer position within
        // preview_scroller (viewport-local coordinates). GtkEventControllerScroll
        // doesn't report a position itself, so a separate motion controller
        // keeps this up to date for the scroll handler to read.
        pub(crate) last_pointer_pos: Cell<(f64, f64)>,
        // Set by `zoom_at_point` when a Ctrl+Scroll zoom needs to correct
        // the scroll position once its resulting (idle-deferred) layout
        // change has settled. See `apply_pending_zoom_anchor`.
        pub(crate) pending_zoom_anchor: RefCell<Option<ZoomAnchor>>,
        // Whether the frame-clock "layout" hook that applies
        // pending_zoom_anchor has been connected yet (done lazily on first
        // use, from `zoom_at_point`, since the widget's frame clock isn't
        // available until it's realized). See `zoom_at_point`.
        pub(crate) zoom_anchor_hook_connected: Cell<bool>,
    }

    impl Default for TypesetterPreview {
        fn default() -> Self {
            Self {
                action_group: OnceCell::new(),
                diagnostics_list: TemplateChild::default(),
                diagnostics_rows: RefCell::new(Vec::new()),
                bottom_bar_label: TemplateChild::default(),
                warnings_number: TemplateChild::default(),
                preview_pane: TemplateChild::default(),
                parent_window: OnceCell::new(),
                editor_buffer: OnceCell::new(),
                view: OnceCell::new(),
                error_count: Cell::new(0),
                warning_count: Cell::new(0),
                preview_clamp: TemplateChild::default(),
                preview_scroller: TemplateChild::default(),
                preview_list: TemplateChild::default(),
                zoom_in_button: TemplateChild::default(),
                zoom_out_button: TemplateChild::default(),
                reset_zoom_button: TemplateChild::default(),
                zoom_label: TemplateChild::default(),
                pane_overlay: TemplateChild::default(),
                loading_revealer: TemplateChild::default(),
                loading_spinner: TemplateChild::default(),
                loading_label: TemplateChild::default(),
                zoom_controls_revealer: TemplateChild::default(),
                zoom_info_revealer: TemplateChild::default(),
                zoom_level_button: TemplateChild::default(),
                current_generation: Cell::new(0),
                loading_timeout_id: RefCell::new(None),
                is_hovering_content: Cell::new(false),
                zoom_menu_open: Cell::new(false),
                page_store: gtk::gio::ListStore::new::<PreviewObject>(),
                is_visible: Cell::new(false),
                base_pixels_per_point: Cell::new(1.0),
                manual_pixels_per_point: Cell::new(REFERENCE_PIXELS_PER_POINT),
                is_auto_fit: Cell::new(true),
                auto_fit_clamp_max: Cell::new(890),
                bound_images: RefCell::new(Vec::new()),
                visible_pages: RefCell::new(std::collections::HashSet::new()),
                scroll_handler: RefCell::new(None),
                resize_handler: RefCell::new(None),
                last_viewport_check: Cell::new(0),
                pending_viewport_recheck: RefCell::new(None),
                bound_vadjustment: RefCell::new(None),
                bound_hadjustment: RefCell::new(None),
                texture_cache: TextureCache::new(32),
                magnifier: RefCell::new(None),
                magnifier_fade_animation: RefCell::new(None),
                drag_ongoing: Cell::new(false),
                drag_buffer: RefCell::new([(0.0, 0.0); 10]),
                drag_buffer_index: Cell::new(0),
                drag_start_adj: Cell::new((0.0, 0.0)),
                drag_start_pos: Cell::new((0.0, 0.0)),
                drag_momentum: Cell::new((0.0, 0.0)),
                drag_buffer_update_signal: RefCell::new(None),
                drag_released_motion_signal: RefCell::new(None),
                width_change_timeout: RefCell::new(None),
                last_pointer_pos: Cell::new((0.0, 0.0)),
                pending_zoom_anchor: RefCell::new(None),
                zoom_anchor_hook_connected: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterPreview {
        const NAME: &'static str = "TypesetterPreview";
        type Type = super::TypesetterPreview;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for TypesetterPreview {
        fn constructed(&self) {
            self.parent_constructed();

            let imp = self;

            // Remember the "optimal reading width" cap preview.ui configured
            // on the clamp as automatic mode uses this as its fit-to-width
            // target (see recompute_base_scale), independent of whatever the
            // clamp's maximum-size is currently set to (it gets lifted while
            // manually zoomed, see update_clamp_for_mode).
            imp.auto_fit_clamp_max.set(imp.preview_clamp.maximum_size());

            let factory = gtk::SignalListItemFactory::new();

            factory.connect_setup(|_, list_item| {
                if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
                    let preview_image = PreviewImage::new();
                    list_item.set_child(Some(&preview_image));
                    list_item.set_activatable(false);
                    list_item.set_focusable(false);
                    list_item.set_selectable(false);
                }
            });

            factory.connect_bind(glib::clone!(
                #[weak(rename_to = preview)]
                self,
                move |_, list_item| {
                    let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
                    let item = list_item.item();
                    let preview_image = list_item
                        .child()
                        .and_then(|w| w.downcast::<PreviewImage>().ok());

                    if let (Some(page), Some(preview_image)) =
                        (item.and_downcast_ref::<PreviewObject>(), preview_image)
                    {
                        let page_num = page.page_number();
                        let (width_pt, height_pt) = page.dimensions();

                        // Set actual point dimensions; PreviewImage computes its
                        // own pixel size from these plus the shared document scale
                        preview_image.set_dimensions(width_pt, height_pt);
                        preview_image.set_page_number(page_num);

                        // Set the invert property for the pre-texture background color
                        let should_invert = preview
                            .action_group
                            .get()
                            .and_then(|g| g.action_state("invert-lightness"))
                            .and_then(|v| v.get::<bool>())
                            .unwrap_or(false)
                            && adw::StyleManager::default().is_dark();
                        preview_image.set_invert_bg(should_invert);
                        // Same value also drives the actual texture's
                        // GPU luma-invert now (see PreviewImage::snapshot())
                        preview_image.set_invert(should_invert);

                        // Set the initial colorblind-simulation mode
                        // (applied at paint time - see PreviewImage::snapshot())
                        let cb_mode = preview
                            .action_group
                            .get()
                            .and_then(|g| g.action_state("simulate-color-blindness"))
                            .and_then(|v| v.get::<String>())
                            .unwrap_or_else(|| "none".to_string());
                        preview_image.set_cb_mode(&cb_mode);

                        // This connects to the page for signal-based updates
                        preview_image.connect_to_page(page);

                        // Track this widget so a later zoom/scale change can
                        // queue_resize() it directly (see requeue_bound_images)
                        preview
                            .bound_images
                            .borrow_mut()
                            .push(preview_image.downgrade());
                    }
                }
            ));

            factory.connect_unbind(glib::clone!(
                #[weak(rename_to = preview)]
                self,
                move |_, obj| {
                    if let Some(list_item) = obj.downcast_ref::<gtk::ListItem>()
                        && let Some(preview_image) = list_item
                            .child()
                            .and_then(|w| w.downcast::<PreviewImage>().ok())
                    {
                        preview_image.disconnect_from_page();
                        preview_image.cleanup();

                        // Drop this widget (and any other dead entries) from
                        // the tracked list
                        preview.bound_images.borrow_mut().retain(|weak| {
                            weak.upgrade().is_some_and(|widget| widget != preview_image)
                        });
                    }
                }
            ));

            factory.connect_teardown(|_, list_item| {
                if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
                    list_item.set_child(None::<&PreviewImage>);
                }
            });

            let selection_model = gtk::NoSelection::new(Some(imp.page_store.clone()));
            imp.preview_list.set_factory(Some(&factory));
            imp.preview_list.set_model(Some(&selection_model));

            // Set up viewport monitoring
            let obj = imp.obj();

            // Connect to adjustment value changes for scroll tracking.
            obj.bind_vadjustment_listeners();

            // Also monitor for when adjustment becomes available or changes
            let scroll_handler = imp.preview_list.connect_notify_local(
                Some("vadjustment"),
                glib::clone!(
                    #[weak]
                    obj,
                    move |_, _| {
                        obj.bind_vadjustment_listeners();
                        // Trigger initial check
                        glib::idle_add_local_once(glib::clone!(
                            #[weak]
                            obj,
                            move || {
                                obj.check_visible_pages("notify::vadjustment reattach (idle)");
                            }
                        ));
                    }
                ),
            );
            imp.scroll_handler.replace(Some(scroll_handler));

            // Detect viewport width changes (window resize/maximize) the
            // same reliable way scroll position is tracked above:
            // via the adjustment's notify signal, rather than by
            // polling preview_list.width() inside size_allocate. The
            // adjustment's "page-size" is specifically "the size of the
            // visible area" and is only updated once GTK has actually
            // finished settling layout (reading .width() manually
            // mid-cascade can catch a stale pre-layout value, which
            // silently drops the resize).
            // bind_hadjustment_listeners() is idempotent for the same
            // reason as bind_vadjustment_listeners() above.
            obj.bind_hadjustment_listeners();

            let resize_handler = imp.preview_list.connect_notify_local(
                Some("hadjustment"),
                glib::clone!(
                    #[weak]
                    obj,
                    move |_, _| {
                        obj.bind_hadjustment_listeners();
                        obj.on_viewport_width_changed();
                    }
                ),
            );
            imp.resize_handler.replace(Some(resize_handler));

            // Set up kinetic scrolling with left-click drag
            let drag = gtk::GestureDrag::builder()
                .button(2)
                .propagation_phase(gtk::PropagationPhase::Capture)
                .build();

            drag.connect_drag_begin(glib::clone!(
                #[weak(rename_to = preview)]
                obj,
                move |gesture, x, y| {
                    let imp = preview.imp();

                    // If magnifier is active, deny the drag gesture immediately
                    if imp.magnifier.borrow().is_some() {
                        gesture.set_state(gtk::EventSequenceState::Denied);
                        return;
                    }

                    // Stop any ongoing animations
                    if let Some(id) = imp.drag_released_motion_signal.take() {
                        id.remove();
                    }
                    if let Some(id) = imp.drag_buffer_update_signal.take() {
                        id.remove();
                    }

                    // Set cursor to grabbing
                    if let Some(cursor) = gtk::gdk::Cursor::from_name("grabbing", None)
                        && let Some(native) = preview.native()
                        && let Some(surface) = native.surface()
                    {
                        surface.set_cursor(Some(&cursor));
                    }

                    // Initialize drag state
                    imp.drag_buffer.replace([(0.0, 0.0); 10]);
                    imp.drag_buffer_index.set(0);
                    imp.drag_ongoing.set(true);
                    imp.drag_momentum.set((0.0, 0.0));
                    imp.drag_start_pos.set((x, y));

                    // Store initial adjustment values
                    let start_h = imp
                        .preview_list
                        .hadjustment()
                        .map(|adj| adj.value())
                        .unwrap_or(0.0);
                    let start_v = imp
                        .preview_list
                        .vadjustment()
                        .map(|adj| adj.value())
                        .unwrap_or(0.0);
                    imp.drag_start_adj.set((start_h, start_v));

                    // Start buffer update timer (samples velocity every 10ms)
                    let update_id = glib::timeout_add_local(
                        std::time::Duration::from_millis(10),
                        glib::clone!(
                            #[weak]
                            preview,
                            #[upgrade_or]
                            glib::ControlFlow::Break,
                            move || {
                                let imp = preview.imp();
                                if !imp.drag_ongoing.get() {
                                    imp.drag_buffer_update_signal.take();
                                    return glib::ControlFlow::Break;
                                }

                                // Use circular buffer with index instead of shifting
                                let current_idx = imp.drag_buffer_index.get();
                                let oldest_idx = (current_idx + 1) % 10;

                                let buffer = imp.drag_buffer.borrow();
                                let current_val = buffer[current_idx];
                                let oldest_val = buffer[oldest_idx];

                                imp.drag_momentum.set((
                                    current_val.0 - oldest_val.0,
                                    current_val.1 - oldest_val.1,
                                ));

                                glib::ControlFlow::Continue
                            }
                        ),
                    );
                    imp.drag_buffer_update_signal.replace(Some(update_id));
                }
            ));

            drag.connect_drag_update(glib::clone!(
                #[weak(rename_to = preview)]
                obj,
                move |gesture, offset_x, offset_y| {
                    let imp = preview.imp();

                    // If magnifier is active, deny the gesture
                    if imp.magnifier.borrow().is_some() {
                        gesture.set_state(gtk::EventSequenceState::Denied);
                        return;
                    }

                    if !imp.drag_ongoing.get() {
                        return;
                    }

                    // Update circular buffer at current index, then advance
                    let idx = imp.drag_buffer_index.get();
                    imp.drag_buffer.borrow_mut()[idx] = (offset_x, offset_y);
                    imp.drag_buffer_index.set((idx + 1) % 10);

                    // Apply scroll offset (inverted because dragging down/right =
                    // scrolling up/left)
                    let (start_h, start_v) = imp.drag_start_adj.get();
                    if let Some(hadj) = imp.preview_list.hadjustment() {
                        hadj.set_value(start_h - offset_x);
                    }
                    if let Some(vadj) = imp.preview_list.vadjustment() {
                        vadj.set_value(start_v - offset_y);
                    }
                }
            ));

            drag.connect_drag_end(glib::clone!(
                #[weak(rename_to = preview)]
                obj,
                move |gesture, _offset_x, _offset_y| {
                    let imp = preview.imp();

                    if !imp.drag_ongoing.get() {
                        return;
                    }

                    imp.drag_ongoing.set(false);

                    // Restore cursor to default
                    if let Some(native) = preview.native()
                        && let Some(surface) = native.surface()
                    {
                        surface.set_cursor(None);
                    }

                    // Claim the gesture to prevent clicks
                    gesture.set_state(gtk::EventSequenceState::Claimed);

                    // Start momentum animation
                    let scroll_pos_x = std::rc::Rc::new(Cell::new(
                        imp.preview_list
                            .hadjustment()
                            .map(|adj| adj.value())
                            .unwrap_or(0.0),
                    ));
                    let scroll_pos_y = std::rc::Rc::new(Cell::new(
                        imp.preview_list
                            .vadjustment()
                            .map(|adj| adj.value())
                            .unwrap_or(0.0),
                    ));

                    let anim_id = glib::timeout_add_local(
                        std::time::Duration::from_millis(20),
                        glib::clone!(
                            #[weak]
                            preview,
                            #[strong]
                            scroll_pos_x,
                            #[strong]
                            scroll_pos_y,
                            #[upgrade_or]
                            glib::ControlFlow::Break,
                            move || {
                                let imp = preview.imp();

                                // Get current momentum (this tick's velocity, used for
                                // the actual displacement below) and compute its decayed
                                // value for next tick.
                                let (momentum_x, momentum_y) = imp.drag_momentum.get();
                                let decayed_momentum_x = momentum_x / 1.2;
                                let decayed_momentum_y = momentum_y / 1.2;

                                let Some(hadj) = imp.preview_list.hadjustment() else {
                                    imp.drag_released_motion_signal.take();
                                    return glib::ControlFlow::Break;
                                };
                                let Some(vadj) = imp.preview_list.vadjustment() else {
                                    imp.drag_released_motion_signal.take();
                                    return glib::ControlFlow::Break;
                                };

                                let viewport_width = hadj.page_size();
                                let total_width = hadj.upper();
                                let viewport_height = vadj.page_size();
                                let total_height = vadj.upper();

                                // Calculate adjustment
                                let adjust_value_x = momentum_x;
                                let adjust_value_y = momentum_y;
                                let old_value_x = scroll_pos_x.get();
                                let old_value_y = scroll_pos_y.get();

                                // Check bounds, zeroing next tick's momentum on
                                // whichever axis just ran off the end (leaving the
                                // decayed value for the other axis untouched).
                                let mut next_momentum_x = decayed_momentum_x;
                                if (old_value_x + adjust_value_x) > (total_width - viewport_width)
                                    || (old_value_x + adjust_value_x) < 0.0
                                {
                                    next_momentum_x = 0.0;
                                }
                                let mut next_momentum_y = decayed_momentum_y;
                                if (old_value_y + adjust_value_y) > (total_height - viewport_height)
                                    || (old_value_y + adjust_value_y) < 0.0
                                {
                                    next_momentum_y = 0.0;
                                }
                                imp.drag_momentum.set((next_momentum_x, next_momentum_y));

                                // Apply new scroll position
                                let new_value_x = (old_value_x + adjust_value_x)
                                    .clamp(0.0, total_width - viewport_width);
                                let new_value_y = (old_value_y + adjust_value_y)
                                    .clamp(0.0, total_height - viewport_height);
                                scroll_pos_x.set(new_value_x);
                                scroll_pos_y.set(new_value_y);
                                hadj.set_value(new_value_x);
                                vadj.set_value(new_value_y);

                                // Stop if momentum is negligible or new drag started
                                let (mx, my) = imp.drag_momentum.get();
                                if (mx.abs() < 1.0 && my.abs() < 1.0) || imp.drag_ongoing.get() {
                                    imp.drag_released_motion_signal.take();
                                    glib::ControlFlow::Break
                                } else {
                                    glib::ControlFlow::Continue
                                }
                            }
                        ),
                    );
                    imp.drag_released_motion_signal.replace(Some(anim_id));
                }
            ));

            imp.preview_list.add_controller(drag);

            obj.connect_notify_local(
                Some("warning-count"),
                glib::clone!(
                    #[weak]
                    obj,
                    move |_, _| {
                        let count = obj.warning_count();
                        let visible = count > 0;
                        obj.imp().bottom_bar_label.set_visible(visible);
                        let text = match count {
                            0 => gettext("No Warnings"),
                            1 => gettext("1 Warning"),
                            n => format!("{} {}", n, gettext("Warnings")),
                        };
                        obj.imp().warnings_number.set_label(&text);
                    }
                ),
            );

            imp.preview_pane.connect_notify_local(
                Some("reveal-bottom-bar"),
                glib::clone!(
                    #[weak(rename_to = pane)]
                    imp.preview_pane,
                    move |_, _| {
                        if pane.reveals_bottom_bar() {
                            pane.add_css_class("has-bottom-bar");
                        } else {
                            pane.remove_css_class("has-bottom-bar");
                        }
                    }
                ),
            );

            // Set up right click context menu
            let right_click = gtk::GestureClick::builder().button(3).build();

            right_click.connect_pressed(glib::clone!(
                #[weak]
                obj,
                move |_, _, x, y| {
                    let imp = obj.imp();

                    let menu_model = gtk::gio::Menu::new();

                    let style_manager = adw::StyleManager::default();
                    if style_manager.is_dark() {
                        let display_section = gtk::gio::Menu::new();
                        display_section.append(
                            Some(&gettext("Invert Lightness")),
                            Some("preview.invert-lightness"),
                        );
                        menu_model.append_section(None, &display_section);
                    }

                    let cb_section = gtk::gio::Menu::new();
                    cb_section.append(
                        Some(&gettext("None")),
                        Some("preview.simulate-color-blindness::none"),
                    );
                    cb_section.append(
                        Some(&gettext("Deuteranopia")),
                        Some("preview.simulate-color-blindness::deuteranopia"),
                    );
                    cb_section.append(
                        Some(&gettext("Protanopia")),
                        Some("preview.simulate-color-blindness::protanopia"),
                    );
                    cb_section.append(
                        Some(&gettext("Tritanopia")),
                        Some("preview.simulate-color-blindness::tritanopia"),
                    );
                    cb_section.append(
                        Some(&gettext("Achromatopsia")),
                        Some("preview.simulate-color-blindness::achromatopsia"),
                    );
                    menu_model
                        .append_section(Some(&gettext("Simulate Color Blindness")), &cb_section);

                    let popover = gtk::PopoverMenu::from_model(Some(&menu_model));
                    popover.set_has_arrow(false);
                    popover.set_parent(&*imp.preview_clamp);
                    popover.set_halign(gtk::Align::Start);
                    popover
                        .set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 0, 0)));
                    popover.popup();
                }
            ));

            imp.preview_scroller.add_controller(right_click);

            // Set up context menu actions
            let action_group = gtk::gio::SimpleActionGroup::new();

            // Invert lightness toggle (boolean state)
            let invert_action =
                gtk::gio::SimpleAction::new_stateful("invert-lightness", None, &false.to_variant());
            invert_action.connect_activate(glib::clone!(
                #[weak]
                obj,
                move |action, _| {
                    let current = action.state().unwrap().get::<bool>().unwrap();
                    let new_value = !current;
                    action.set_state(&new_value.to_variant());

                    // Save to settings
                    if let Some(window) = obj.find_parent_window() {
                        let _ = window.settings().set_boolean("invert-lightness", new_value);
                    }

                    // Lightness-invert is now applied at paint time via a
                    // GPU color matrix (see PreviewImage::snapshot() and
                    // crate::utils::combined_transform), not baked into the
                    // rendered texture - so, like the colorblind toggle
                    // below, this no longer needs a Typst recompile, a
                    // texture_cache.clear(), or marking pages dirty. Just
                    // update every bound widget's background-placeholder
                    // color and its actual texture invert, then repaint.
                    let should_invert = new_value && adw::StyleManager::default().is_dark();
                    let mut list_item = obj.imp().preview_list.first_child();
                    while let Some(item) = list_item {
                        if let Some(preview_image) =
                            item.first_child().and_downcast::<PreviewImage>()
                        {
                            preview_image.set_invert_bg(should_invert);
                            preview_image.set_invert(should_invert);
                            preview_image.queue_draw();
                        }
                        list_item = item.next_sibling();
                    }

                    // Same for the magnifier lens, if one's currently active
                    if let Some(magnifier) = obj.imp().magnifier.borrow().as_ref() {
                        magnifier.set_invert_lightness(should_invert);
                    }
                }
            ));
            action_group.add_action(&invert_action);

            // Color blindness simulation (string state)
            let cb_action = gtk::gio::SimpleAction::new_stateful(
                "simulate-color-blindness",
                Some(glib::VariantTy::STRING),
                &"none".to_variant(),
            );
            cb_action.connect_activate(glib::clone!(
                #[weak]
                obj,
                move |action, target| {
                    if let Some(t) = target {
                        action.set_state(t);

                        // Colorblind simulation is applied at paint time via
                        // a GPU color matrix (see PreviewImage::snapshot()),
                        // not baked into the rendered texture anymore - so
                        // changing modes no longer needs a Typst recompile,
                        // a texture_cache.clear(), or marking pages dirty.
                        // Just tell every currently-bound widget its new
                        // mode and ask it to repaint.
                        let new_mode = t.get::<String>().unwrap_or_else(|| "none".to_string());
                        let mut list_item = obj.imp().preview_list.first_child();
                        while let Some(item) = list_item {
                            if let Some(preview_image) =
                                item.first_child().and_downcast::<PreviewImage>()
                            {
                                preview_image.set_cb_mode(&new_mode);
                                preview_image.queue_draw();
                            }
                            list_item = item.next_sibling();
                        }

                        // Same for the magnifier lens, if one's currently active
                        if let Some(magnifier) = obj.imp().magnifier.borrow().as_ref() {
                            magnifier.set_cb_mode(&new_mode);
                        }
                    }
                }
            ));
            action_group.add_action(&cb_action);

            // Zoom to a specific percentage, from the zoom_menu in
            // preview.ui. Plain activatable action (not stateful/radio).
            // TODO: Conside rmaking it a toggle button or something.
            let set_zoom_action =
                gtk::gio::SimpleAction::new("set-zoom-level", Some(glib::VariantTy::INT32));
            set_zoom_action.connect_activate(glib::clone!(
                #[weak]
                obj,
                move |_, target| {
                    if let Some(percent) = target.and_then(|v| v.get::<i32>()) {
                        obj.set_zoom_percent(percent as f64);
                    }
                }
            ));
            action_group.add_action(&set_zoom_action);

            let _ = imp.action_group.set(action_group.clone());
            obj.insert_action_group("preview", Some(&action_group));

            // Listen to changes to the light/dark mode setting to refresh textures if necessary
            let style_manager = adw::StyleManager::default();

            style_manager.connect_dark_notify(glib::clone!(
                #[weak]
                obj,
                move |_| {
                    // Only matters if invert-lightness is toggled on -
                    // should_invert = toggle && is_dark, so a dark/light
                    // mode change can flip the effective invert state even
                    // though the toggle itself didn't change.
                    let invert_toggled = obj
                        .imp()
                        .action_group
                        .get()
                        .and_then(|g| g.action_state("invert-lightness"))
                        .and_then(|v| v.get::<bool>())
                        .unwrap_or(false);

                    if !invert_toggled {
                        return;
                    }

                    // Invert is applied at paint time via a GPU color
                    // matrix now (see PreviewImage::snapshot() and
                    // crate::utils::combined_transform), so - like the
                    // invert_action/cb_action toggles - this no longer
                    // needs a Typst recompile, a texture_cache.clear(), or
                    // marking pages dirty. Just push the new effective
                    // value to every bound widget and repaint.
                    let should_invert = adw::StyleManager::default().is_dark();
                    let mut list_item = obj.imp().preview_list.first_child();
                    while let Some(item) = list_item {
                        if let Some(preview_image) =
                            item.first_child().and_downcast::<PreviewImage>()
                        {
                            preview_image.set_invert_bg(should_invert);
                            preview_image.set_invert(should_invert);
                            preview_image.queue_draw();
                        }
                        list_item = item.next_sibling();
                    }

                    if let Some(magnifier) = obj.imp().magnifier.borrow().as_ref() {
                        magnifier.set_invert_lightness(should_invert);
                    }
                }
            ));

            // Keyboard scrolling for the preview pane
            let key_controller = gtk::EventControllerKey::new();
            key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);

            key_controller.connect_key_pressed(glib::clone!(
                #[weak(rename_to = preview)]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, key, _, _| {
                    let imp = preview.imp();
                    let vadj = imp.preview_scroller.vadjustment();
                    let current = vadj.value();
                    let page_size = vadj.page_size();
                    let upper = vadj.upper();

                    match key {
                        gtk::gdk::Key::Up => {
                            vadj.set_value((current - vadj.step_increment()).max(0.0));
                            glib::Propagation::Stop
                        }
                        gtk::gdk::Key::Down => {
                            vadj.set_value(
                                (current + vadj.step_increment()).min(upper - page_size),
                            );
                            glib::Propagation::Stop
                        }
                        gtk::gdk::Key::Page_Up => {
                            vadj.set_value((current - page_size).max(0.0));
                            glib::Propagation::Stop
                        }
                        gtk::gdk::Key::Page_Down => {
                            vadj.set_value((current + page_size).min(upper - page_size));
                            glib::Propagation::Stop
                        }
                        gtk::gdk::Key::Home => {
                            vadj.set_value(0.0);
                            glib::Propagation::Stop
                        }
                        gtk::gdk::Key::End => {
                            vadj.set_value(upper - page_size);
                            glib::Propagation::Stop
                        }
                        _ => glib::Propagation::Proceed,
                    }
                }
            ));

            imp.preview_scroller.add_controller(key_controller);

            // Track pointer position within the scroller (viewport-local
            // coordinates), purely so the Ctrl+Scroll handler below knows
            // where to anchor the zoom - GtkEventControllerScroll doesn't
            // report a position of its own.
            let pointer_motion = gtk::EventControllerMotion::new();

            pointer_motion.connect_motion(glib::clone!(
                #[weak(rename_to = preview)]
                obj,
                move |_, x, y| {
                    preview.imp().last_pointer_pos.set((x, y));
                }
            ));

            imp.preview_scroller.add_controller(pointer_motion);

            // Ctrl+Scroll to zoom in/out, anchored on the pointer position.
            // Added at the Capture phase directly on preview_scroller so it
            // gets first look at the event, ahead of the ScrolledWindow's
            // own (Bubble-phase) kinetic scrolling and PreviewImage's
            // magnifier-dismissing scroll handler.
            let scroll_zoom = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::VERTICAL
                    | gtk::EventControllerScrollFlags::HORIZONTAL,
            );
            scroll_zoom.set_propagation_phase(gtk::PropagationPhase::Capture);

            scroll_zoom.connect_scroll(glib::clone!(
                #[weak(rename_to = preview)]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |controller, _dx, dy| {
                    if !controller
                        .current_event_state()
                        .contains(gtk::gdk::ModifierType::CONTROL_MASK)
                    {
                        return glib::Propagation::Proceed;
                    }

                    if dy == 0.0 {
                        return glib::Propagation::Stop;
                    }

                    let (px, py) = preview.imp().last_pointer_pos.get();

                    // Negative dy = scrolled "up"/away = zoom in, matching
                    // the convention used by browsers and PDF viewers. A
                    // full wheel notch (|dy| == 1) zooms by exactly
                    // ZOOM_STEP, same as a single toolbar +/- click.
                    let factor = ZOOM_STEP.powf(-dy);
                    preview.zoom_at_point(factor, px, py);

                    glib::Propagation::Stop
                }
            ));

            imp.preview_scroller.add_controller(scroll_zoom);

            // Monitor scale factor changes to invalidate cache
            obj.connect_notify_local(Some("scale-factor"), |preview, _| {
                let imp = preview.imp();

                // Deactivate magnifier if active (it has old scale textures)
                if imp.magnifier.borrow().is_some() {
                    preview.deactivate_magnifier();
                }

                // Clear the entire cache since all textures were rendered at the old scale
                imp.texture_cache.clear();

                // Mark all pages as needing update
                for i in 0..imp.page_store.n_items() {
                    if let Some(obj) = imp.page_store.item(i)
                        && let Ok(page) = obj.downcast::<PreviewObject>()
                    {
                        page.clear_texture();
                        page.set_needs_update(true);
                    }
                }

                // Trigger re-render of visible pages at new scale
                if preview.is_visible() {
                    glib::idle_add_local_once(glib::clone!(
                        #[weak]
                        preview,
                        move || {
                            preview.check_visible_pages("scale-factor change (idle)");
                        }
                    ));
                }
            });

            // Zoom controls (buttons declared in preview.ui; the pinch/scroll
            // gesture equivalent can hook into `zoom_in`/`zoom_out` later)
            imp.zoom_in_button.connect_clicked(glib::clone!(
                #[weak]
                obj,
                move |_| {
                    obj.zoom_in();
                }
            ));
            imp.zoom_out_button.connect_clicked(glib::clone!(
                #[weak]
                obj,
                move |_| {
                    obj.zoom_out();
                }
            ));
            imp.reset_zoom_button.connect_clicked(glib::clone!(
                #[weak]
                obj,
                move |_| {
                    obj.reset_zoom();
                }
            ));

            // Show the zoom toolbar only while hovering the content area.
            // Deliberately attached to pane_overlay (the ancestor of both
            // preview_scroller and the overlaid zoom_controls_revealer
            // itself), not to preview_scroller - the toolbar buttons are a
            // sibling overlay child, not a descendant of preview_scroller,
            // so watching preview_scroller alone would register "leave" the
            // instant the pointer reached the buttons, hiding them out from
            // under the cursor before they could be clicked. Watching their
            // common ancestor instead means moving between content and
            // buttons stays "inside" the whole time.
            let hover_controller = gtk::EventControllerMotion::new();
            hover_controller.connect_enter(glib::clone!(
                #[weak]
                obj,
                move |_, _, _| {
                    obj.imp().is_hovering_content.set(true);
                    obj.update_toolbar_visibility();
                }
            ));
            hover_controller.connect_leave(glib::clone!(
                #[weak]
                obj,
                move |_| {
                    obj.imp().is_hovering_content.set(false);
                    obj.update_toolbar_visibility();
                }
            ));
            imp.pane_overlay.add_controller(hover_controller);

            // The zoom-level popover isn't a descendant of pane_overlay (it's
            // a separate transient surface), so opening it moves the pointer
            // off pane_overlay and the hover-leave handler above would hide
            // the toolbar - which unmaps zoom_level_button (the popover's
            // anchor) and closes the popover instantly. Track whether it's
            // open and factor that into visibility too, via
            // update_toolbar_visibility(), so the menu can actually stay open.
            match imp.zoom_level_button.popover() {
                Some(popover) => {
                    popover.connect_visible_notify(glib::clone!(
                        #[weak]
                        obj,
                        move |popover| {
                            obj.imp().zoom_menu_open.set(popover.is_visible());
                            obj.update_toolbar_visibility();
                        }
                    ));
                }
                None => {
                    // No-op
                }
            }

            // Reflect the starting 100% state in the label/revealer
            obj.update_zoom_ui();

            // Trigger initial check
            glib::idle_add_local_once(glib::clone!(
                #[weak]
                obj,
                move || {
                    obj.check_visible_pages("constructed() initial check (idle)");
                }
            ));
        }

        fn dispose(&self) {
            // Clean up loading timeout
            if let Some(id) = self.loading_timeout_id.borrow_mut().take() {
                id.remove();
            }

            // Clean up width change timeout
            if let Some(timeout_id) = self.width_change_timeout.borrow_mut().take() {
                timeout_id.remove();
            }

            // Clean up drag timers
            if let Some(id) = self.drag_buffer_update_signal.take() {
                id.remove();
            }
            if let Some(id) = self.drag_released_motion_signal.take() {
                id.remove();
            }

            // Disconnect handlers
            if let Some(handler_id) = self.scroll_handler.take() {
                self.preview_list.disconnect(handler_id);
            }
            if let Some(handler_id) = self.resize_handler.take() {
                self.preview_list.disconnect(handler_id);
            }

            // Clear cache
            self.texture_cache.clear();

            // Clean up all pages before disposal
            let items: Vec<_> = (0..self.page_store.n_items())
                .filter_map(|i| self.page_store.item(i))
                .filter_map(|obj| obj.downcast::<PreviewObject>().ok())
                .collect();

            for page in items {
                page.cleanup();
            }

            self.page_store.remove_all();
        }
    }

    impl WidgetImpl for TypesetterPreview {
        fn realize(&self) {
            self.parent_realize();

            // Restore "Invert Lightness" setting
            if let Some(window) = self.obj().find_parent_window()
                && let Some(group) = self.action_group.get()
            {
                let value = window.settings().boolean("invert-lightness");
                group.change_action_state("invert-lightness", &value.to_variant());
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);

            // Allocate space for magnifier if it exists
            if let Some(magnifier) = self.magnifier.borrow().as_ref() {
                // Magnifier should cover the entire preview_list
                let list = &self.preview_scroller;
                let list_width = list.width();
                let list_height = list.height();

                if list_width > 0 && list_height > 0 {
                    magnifier.allocate(list_width, list_height, baseline, None);
                }
            }
        }
    }

    impl BoxImpl for TypesetterPreview {}
}

glib::wrapper! {
    pub struct TypesetterPreview(ObjectSubclass<imp::TypesetterPreview>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Buildable, gtk::ConstraintTarget, gtk::Accessible, gtk::Orientable, gtk::Actionable, gtk::ShortcutManager;
}

#[allow(clippy::new_without_default)]
impl TypesetterPreview {
    pub fn new() -> Self {
        glib::Object::new::<Self>()
    }

    pub fn cleanup(&self) {
        let imp = self.imp();

        // Clear cache first
        imp.texture_cache.clear();

        // Collect items first to avoid iterator invalidation
        let items: Vec<_> = (0..imp.page_store.n_items())
            .filter_map(|i| imp.page_store.item(i))
            .filter_map(|obj| obj.downcast::<PreviewObject>().ok())
            .collect();

        for page in items {
            page.cleanup();
        }

        imp.page_store.remove_all();
        imp.preview_list
            .set_model(None::<gtk::SelectionModel>.as_ref());
        imp.preview_list
            .set_factory(None::<gtk::ListItemFactory>.as_ref());
        imp.bound_images.borrow_mut().clear();
    }

    /// Reset the preview state when switching documents, clearing pages and textures.
    pub fn reset(&self) {
        let imp = self.imp();
        // Advance generation to invalidate any in-flight rendering jobs
        imp.current_generation.set(imp.current_generation.get() + 1);

        imp.texture_cache.clear();

        let items: Vec<_> = (0..imp.page_store.n_items())
            .filter_map(|i| imp.page_store.item(i))
            .filter_map(|obj| obj.downcast::<PreviewObject>().ok())
            .collect();

        for page in items {
            page.cleanup();
        }

        imp.page_store.remove_all();

        // Show subtle loading indicator while the new document is being prepared
        self.show_loading();
    }

    /// Show the preview loading spinner overlay
    pub fn show_loading(&self) {
        let imp = self.imp();
        imp.loading_spinner.start();
        imp.loading_label.set_label(&gettext("Loading preview…"));
        imp.loading_revealer.set_reveal_child(true);

        // Cancel previous safety timeout
        if let Some(id) = imp.loading_timeout_id.borrow_mut().take() {
            id.remove();
        }

        // 15-second watchdog: if compilation or rendering takes longer than expected,
        // do NOT prematurely hide the indicator! Instead, update the status label to
        // inform the user that a complex compilation/render is still in progress.
        // Captures expected generation so timeout cannot affect a newer document generation.
        let self_weak = glib::SendWeakRef::from(self.downgrade());
        let expected_gen = imp.current_generation.get();
        let id = glib::timeout_add_local_once(std::time::Duration::from_secs(15), move || {
            if let Some(preview) = self_weak.upgrade()
                && preview.imp().current_generation.get() == expected_gen
            {
                preview
                    .imp()
                    .loading_label
                    .set_label(&gettext("Still compiling…"));
            }
        });
        *imp.loading_timeout_id.borrow_mut() = Some(id);
    }

    /// Hide the preview loading spinner overlay
    pub fn hide_loading(&self) {
        let imp = self.imp();
        if let Some(id) = imp.loading_timeout_id.borrow_mut().take() {
            id.remove();
        }
        imp.loading_spinner.stop();
        imp.loading_revealer.set_reveal_child(false);
        imp.loading_label.set_label(&gettext("Loading preview…"));
    }

    /// Called whenever a page texture belonging to `generation` has been successfully
    /// attached to a PreviewObject (either from cache hit or from background render).
    /// If the loading indicator is still shown, hides it immediately because the user
    /// now has visual content for the active document.
    pub(crate) fn on_page_texture_attached(&self, generation: u64, page_num: usize) {
        let imp = self.imp();
        if imp.current_generation.get() == generation {
            let is_visible_page = imp.visible_pages.borrow().contains(&page_num)
                || imp.visible_pages.borrow().is_empty()
                || page_num == 0;
            if is_visible_page {
                #[cfg(debug_assertions)]
                println!(
                    "TypesetterPreview: First valid texture attached for visible page {} (gen {}), hiding loading",
                    page_num, generation
                );
                self.hide_loading();
            }
        }
    }

    pub fn current_generation(&self) -> u64 {
        self.imp().current_generation.get()
    }

    pub fn texture_cache(&self) -> &TextureCache {
        &self.imp().texture_cache
    }

    pub fn preview_list(&self) -> &gtk::ListView {
        &self.imp().preview_list
    }

    /// The document-wide scale (points -> pixels) that every page is sized
    /// against. `PreviewImage::measure()` reads this to compute each page's
    /// true pixel size, so all pages stay correctly sized relative to one
    /// another regardless of mode.
    pub fn effective_pixels_per_point(&self) -> f64 {
        let imp = self.imp();
        if imp.is_auto_fit.get() {
            imp.base_pixels_per_point.get()
        } else {
            imp.manual_pixels_per_point.get()
        }
    }

    /// Whether the preview is in automatic (fit-widest-page-to-viewport)
    /// mode, as opposed to a manually-chosen, viewport-independent zoom.
    pub fn is_auto_fit(&self) -> bool {
        self.imp().is_auto_fit.get()
    }

    pub fn zoom_in(&self) {
        self.step_zoom(ZOOM_STEP);
    }

    pub fn zoom_out(&self) {
        self.step_zoom(1.0 / ZOOM_STEP);
    }

    /// Multiplies the current effective scale by `factor` and switches to
    /// manual mode if we weren't already in it. Anchored at the center of
    /// the viewport - unlike a cursor-driven Ctrl+Scroll zoom, a toolbar
    /// button click has no cursor position to anchor to, and zooming
    /// toward the viewport's center (rather than leaving the top-left
    /// corner fixed) is what "zoom in/out" buttons do elsewhere. Reuses
    /// `zoom_at_point`/`apply_pending_zoom_anchor` so this gets the exact
    /// same scroll-position correction as Ctrl+Scroll zooming.
    fn step_zoom(&self, factor: f64) {
        let imp = self.imp();

        let viewport_x = f64::from(imp.preview_scroller.width()) / 2.0;
        let viewport_y = f64::from(imp.preview_scroller.height()) / 2.0;

        self.zoom_at_point(factor, viewport_x, viewport_y);
    }

    /// Sets zoom to an absolute percentage of true 100% size (e.g. from the
    /// zoom_menu in preview.ui), switching to manual mode. Anchored at the
    /// viewport center, same as `step_zoom`, via `zoom_at_point` - see that
    /// function for why.
    pub fn set_zoom_percent(&self, percent: f64) {
        let imp = self.imp();

        let min = REFERENCE_PIXELS_PER_POINT * ZOOM_MIN_FACTOR;
        let max = REFERENCE_PIXELS_PER_POINT * ZOOM_MAX_FACTOR;
        let target = (REFERENCE_PIXELS_PER_POINT * (percent / 100.0)).clamp(min, max);
        let factor = target / self.effective_pixels_per_point();

        let viewport_x = f64::from(imp.preview_scroller.width()) / 2.0;
        let viewport_y = f64::from(imp.preview_scroller.height()) / 2.0;

        self.zoom_at_point(factor, viewport_x, viewport_y);
    }

    /// Multiplies the current effective scale by `factor`, switching to
    /// manual mode if needed, while keeping the document point under
    /// (`viewport_x`, `viewport_y`) - preview_scroller-local coordinates -
    /// fixed on screen. Shared by Ctrl+Scroll zooming (anchored at the
    /// cursor), the toolbar +/- buttons via `step_zoom` (anchored at the
    /// viewport center), and `set_zoom_percent` (same); either way, the
    /// scroll position can only be corrected once the resulting layout
    /// change has actually landed (see `apply_pending_zoom_anchor`).
    fn zoom_at_point(&self, factor: f64, viewport_x: f64, viewport_y: f64) {
        let imp = self.imp();

        // Always refresh what the pending correction should target, even
        // if this notch doesn't end up changing the scale (most commonly:
        // it's clamped because we're already at the zoom ceiling/floor and
        // the user keeps scrolling past it).
        if let Some(anchor) = self.capture_zoom_anchor(viewport_x, viewport_y) {
            imp.pending_zoom_anchor.replace(Some(anchor));
            self.ensure_zoom_anchor_hook();
        }

        let min = REFERENCE_PIXELS_PER_POINT * ZOOM_MIN_FACTOR;
        let max = REFERENCE_PIXELS_PER_POINT * ZOOM_MAX_FACTOR;
        let old_scale = self.effective_pixels_per_point();
        let new_scale = (old_scale * factor).clamp(min, max);

        if (new_scale - old_scale).abs() < f64::EPSILON {
            return;
        }

        imp.manual_pixels_per_point.set(new_scale);
        imp.is_auto_fit.set(false);

        self.update_clamp_for_mode();
        self.update_zoom_ui();
        self.schedule_immediate_resize();
        self.debounce_settle_render_scale();
    }

    /// Finds the page under (`viewport_x`, `viewport_y`) - in
    /// preview_scroller-local coordinates - and records it plus the
    /// fractional position within its current allocation, for
    /// `apply_pending_zoom_anchor` to re-derive once layout has moved the
    /// page around. Returns `None` if the point isn't over any page (e.g.
    /// blank space above/below the document), in which case the caller
    /// leaves any existing pending correction untouched.
    fn capture_zoom_anchor(&self, viewport_x: f64, viewport_y: f64) -> Option<ZoomAnchor> {
        let imp = self.imp();

        let (list_x, list_y) = {
            let point = gtk::graphene::Point::new(viewport_x as f32, viewport_y as f32);
            let translated = imp
                .preview_scroller
                .compute_point(&*imp.preview_list, &point)?;
            (translated.x() as f64, translated.y() as f64)
        };

        let hit = imp
            .preview_list
            .pick(list_x, list_y, gtk::PickFlags::DEFAULT)?;
        let page = hit
            .ancestor(PreviewImage::static_type())?
            .downcast::<PreviewImage>()
            .ok()?;

        let (page_x, page_y) = {
            let origin = gtk::graphene::Point::new(0.0, 0.0);
            let translated = page.compute_point(&*imp.preview_scroller, &origin)?;
            (translated.x() as f64, translated.y() as f64)
        };
        let page_width = f64::from(page.width());
        let page_height = f64::from(page.height());

        if page_width <= 0.0 || page_height <= 0.0 {
            return None;
        }

        Some(ZoomAnchor {
            page_number: page.page_number(),
            fx: (viewport_x - page_x) / page_width,
            fy: (viewport_y - page_y) / page_height,
            viewport_x,
            viewport_y,
        })
    }

    /// Connects `apply_pending_zoom_anchor` to the widget's frame clock
    /// "layout" signal, if not already done. This fires once per rendered
    /// frame, right after GTK has measured/allocated widgets for that
    /// frame but before painting, exactly when it's safe to trust
    /// page geometry, with no arbitrary delay and no risk of correction
    /// being starved by a steady stream of scroll events (as a debounce
    /// timer would be). Connected lazily here, rather than in
    /// `constructed()`, since the frame clock isn't available until the
    /// widget is realized, which it certainly is by the time this is
    /// called, since the user is actively interacting with it.
    fn ensure_zoom_anchor_hook(&self) {
        let imp = self.imp();

        if imp.zoom_anchor_hook_connected.get() {
            return;
        }

        let Some(frame_clock) = self.frame_clock() else {
            return;
        };

        frame_clock.connect_layout(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                obj.apply_pending_zoom_anchor();
            }
        ));

        imp.zoom_anchor_hook_connected.set(true);
    }

    /// Re-derives the scroll position that keeps a pending zoom's anchor
    /// point fixed on screen, and applies it. A no-op unless a zoom is
    /// actually pending, so it's cheap to call on every frame via
    /// `ensure_zoom_anchor_hook`.
    fn apply_pending_zoom_anchor(&self) {
        let imp = self.imp();

        let Some(anchor) = imp.pending_zoom_anchor.take() else {
            return;
        };

        // Re-resolve the anchor's page number against whichever widget is
        // *currently* bound to it, rather than a direct reference to the
        // widget seen at capture time. preview_list may have recycled
        // that specific widget instance for a different page since then
        // (common during a fast zoom-out burst, as shrinking pages bring
        // more rows into view). If no currently-bound widget shows this
        // page anymore (e.g. it scrolled out of range entirely), there's
        // nothing sensible to correct against.
        let Some(page) = imp
            .bound_images
            .borrow()
            .iter()
            .filter_map(glib::WeakRef::upgrade)
            .find(|widget| widget.page_number() == anchor.page_number)
        else {
            return;
        };

        let hadj = imp.preview_scroller.hadjustment();
        let vadj = imp.preview_scroller.vadjustment();

        let Some((page_x, page_y)) = ({
            let origin = gtk::graphene::Point::new(0.0, 0.0);
            page.compute_point(&*imp.preview_scroller, &origin)
                .map(|p| (p.x() as f64, p.y() as f64))
        }) else {
            return;
        };
        let page_width = f64::from(page.width());
        let page_height = f64::from(page.height());

        // Where the anchored point on the page actually is on screen now,
        // at its new (post-layout) size - compute_point already accounts
        // for the current (not-yet-corrected) scroll position, so this is
        // directly comparable to viewport_x/viewport_y.
        let current_screen_x = page_x + anchor.fx * page_width;
        let current_screen_y = page_y + anchor.fy * page_height;

        let new_h = (hadj.value() + current_screen_x - anchor.viewport_x)
            .clamp(0.0, (hadj.upper() - hadj.page_size()).max(0.0));
        let new_v = (vadj.value() + current_screen_y - anchor.viewport_y)
            .clamp(0.0, (vadj.upper() - vadj.page_size()).max(0.0));

        hadj.set_value(new_h);
        vadj.set_value(new_v);
    }

    /// Returns to automatic (fit-widest-page-to-viewport) mode.
    pub fn reset_zoom(&self) {
        let imp = self.imp();

        if imp.is_auto_fit.get() {
            return;
        }

        imp.is_auto_fit.set(true);

        self.update_clamp_for_mode();
        self.update_zoom_ui();
        self.schedule_immediate_resize();
        self.debounce_settle_render_scale();
    }

    /// Lifts (manual) or restores (automatic) the clamp's width cap.
    fn update_clamp_for_mode(&self) {
        let imp = self.imp();
        if imp.is_auto_fit.get() {
            imp.preview_clamp
                .set_maximum_size(imp.auto_fit_clamp_max.get());
        } else {
            imp.preview_clamp.set_maximum_size(i32::MAX);
        }
    }

    /// Shows the zoom toolbar if either the pointer is over the content
    /// area or a popover launched from within the toolbar (currently just
    /// the zoom-level menu) is open - except while the magnifier is
    /// active, when it's always hidden. The magnifier is parented onto
    /// preview_scroller (see `activate_magnifier`), which is pane_overlay's
    /// main child; GtkOverlay always paints overlay children (this
    /// toolbar) above the main child, so there's no z-order trick that
    /// would let the toolbar coexist on screen with the magnifier without
    /// visually sitting on top of it.
    fn update_toolbar_visibility(&self) {
        let imp = self.imp();

        if imp.magnifier.borrow().is_some() {
            imp.zoom_controls_revealer.set_reveal_child(false);
            return;
        }

        let should_show = imp.is_hovering_content.get() || imp.zoom_menu_open.get();

        imp.zoom_controls_revealer.set_reveal_child(should_show);
    }

    /// Refreshes the zoom percentage label and button sensitivity to match
    /// the current mode/scale. Percentage is always relative to true 100%
    /// size (`REFERENCE_PIXELS_PER_POINT`), not to the viewport.
    fn update_zoom_ui(&self) {
        let imp = self.imp();
        let effective = self.effective_pixels_per_point();
        let percent = (effective / REFERENCE_PIXELS_PER_POINT * 100.0).round() as i64;

        imp.zoom_label.set_label(&format!("{percent}%"));

        let is_manual = !imp.is_auto_fit.get();
        imp.reset_zoom_button.set_sensitive(is_manual);
        imp.zoom_info_revealer.set_reveal_child(is_manual);

        let min = REFERENCE_PIXELS_PER_POINT * ZOOM_MIN_FACTOR;
        let max = REFERENCE_PIXELS_PER_POINT * ZOOM_MAX_FACTOR;
        imp.zoom_in_button
            .set_sensitive(effective < max - f64::EPSILON);
        imp.zoom_out_button
            .set_sensitive(effective > min + f64::EPSILON);
    }

    /// Called whenever the viewport's actual visible width changes (see the
    /// `hadjustment` "page-size" listener in `constructed()`).
    ///
    /// Widget resizing is immediate (well, idle-deferred, see
    /// schedule_immediate_resize()) on every call, so the interface stays
    /// fluid during a live resize drag. The expensive part, clearing
    /// the texture cache and asking the compiler to re-render at the new
    /// resolution, is on the 200ms debounce.
    fn on_viewport_width_changed(&self) {
        if self.refresh_auto_fit_scale() {
            self.debounce_settle_render_scale();
        }
    }

    // TODO: Triage the underlying bug and attempt to fix it so we don't need to defer this.
    /// Queues an immediate widget resize on the next main-loop iteration.
    /// Deferred to idle rather than calling requeue_bound_images() directly
    /// in order to prevent incorrect widget sizing.
    fn schedule_immediate_resize(&self) {
        glib::idle_add_local_once(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            move || {
                obj.requeue_bound_images();
            }
        ));
    }

    /// Schedules `settle_render_scale()` to run once 200ms have passed with
    /// no further scale change.
    fn debounce_settle_render_scale(&self) {
        let imp = self.imp();

        if let Some(timeout_id) = imp.width_change_timeout.take() {
            timeout_id.remove();
        }

        let timeout_id = glib::timeout_add_local_once(
            std::time::Duration::from_millis(200),
            glib::clone!(
                #[weak(rename_to = obj)]
                self,
                move || {
                    obj.imp().width_change_timeout.take();
                    obj.settle_render_scale();
                }
            ),
        );

        imp.width_change_timeout.replace(Some(timeout_id));
    }

    /// Recomputes `base_pixels_per_point` so the widest page in the document
    /// exactly fits the target reading width. This is what automatic
    /// mode uses, and it's kept up to date in the background even while in
    /// manual zoom, ready for the next zoom-in (seeds from the current
    /// effective scale) or reset.
    ///
    /// The available width is derived by re-running AdwClampLayout's own
    /// tightening-threshold easing formula directly against
    /// `preview_scroller.width()`, rather than reading any widget's allocated
    /// size back out of the tree.
    pub(crate) fn recompute_base_scale(&self) {
        let imp = self.imp();

        // Match the margin/padding allowance the old width-based rendering
        // used, so the widest page doesn't visually touch the scroller edges
        let for_size = f64::from(imp.preview_scroller.width());
        let maximum_size = f64::from(imp.auto_fit_clamp_max.get());
        let tightening_threshold = f64::from(imp.preview_clamp.tightening_threshold());

        let eased_width = clamp_eased_width(for_size, maximum_size, tightening_threshold);
        let available_width_px = (eased_width - 36.0).max(100.0);

        let max_width_pt = (0..imp.page_store.n_items())
            .filter_map(|i| imp.page_store.item(i))
            .filter_map(|obj| obj.downcast::<PreviewObject>().ok())
            .map(|page| page.dimensions().0 as f64)
            .fold(0.0_f64, f64::max);

        if max_width_pt <= 0.0 {
            return;
        }

        let base_pixels_per_point = available_width_px / max_width_pt;

        imp.base_pixels_per_point.set(base_pixels_per_point);
    }

    /// Recomputes the auto-fit scale and, only if that's the scale actually
    /// in effect right now *and* it changed, updates the zoom label and
    /// schedules an immediate (idle-deferred) widget resize. Doesn't touch
    /// the texture cache or ask the compiler for anything. Callers decide
    /// separately how to follow up with the expensive re-render. Returns
    /// whether the effective scale actually changed.
    pub(crate) fn refresh_auto_fit_scale(&self) -> bool {
        let before = self.effective_pixels_per_point();
        self.recompute_base_scale();
        let after = self.effective_pixels_per_point();
        let changed = (before - after).abs() > 0.0001;

        if changed {
            self.update_zoom_ui();
            self.schedule_immediate_resize();
        }
        changed
    }

    // TODO: Can `effective_pixels_per_point` be made into a gproperty and would that mean we can get rid of this?
    /// Forces every currently-bound `PreviewImage` to re-measure itself
    /// against the current `effective_pixels_per_point()`. Needed because
    /// that value lives outside GTK's property/notify system, so nothing
    /// tells a widget's `measure()` is now stale on its own.
    fn requeue_bound_images(&self) {
        let imp = self.imp();

        imp.bound_images.borrow_mut().retain(|weak| {
            let Some(widget) = weak.upgrade() else {
                return false;
            };

            widget.queue_resize();
            true
        });
    }

    /// The expensive half of applying a scale change: invalidates the
    /// texture cache (old textures are the wrong resolution now) and marks
    /// every page dirty so the compiler re-renders them at the new resolution.
    /// Also re-requeues bound images as a safety net (harmless/idempotent
    /// if schedule_immediate_resize's idle callback already ran).
    pub(crate) fn settle_render_scale(&self) {
        let imp = self.imp();

        self.requeue_bound_images();

        imp.texture_cache.clear();

        for i in 0..imp.page_store.n_items() {
            if let Some(obj) = imp.page_store.item(i)
                && let Ok(page) = obj.downcast::<PreviewObject>()
            {
                page.set_needs_update(true);
            }
        }

        self.check_visible_pages("settle_render_scale");
    }

    pub(crate) fn find_parent_window(&self) -> Option<crate::window::TypesetterWindow> {
        let imp = self.imp();

        imp.parent_window.get().cloned().or_else(|| {
            let mut parent = self.parent();
            while let Some(widget) = parent {
                if let Ok(window) = widget.clone().downcast::<crate::window::TypesetterWindow>() {
                    let _ = imp.parent_window.set(window.clone());
                    return Some(window);
                }
                parent = widget.parent();
            }
            None
        })
    }

    pub(crate) fn find_editor_buffer(&self) -> Option<sourceview::Buffer> {
        let imp = self.imp();

        if let Some(buffer) = imp.editor_buffer.get() {
            Some(buffer.clone())
        } else {
            let buffer = self.find_parent_window().and_then(|window| {
                window
                    .imp()
                    .editor_container
                    .get()
                    .imp()
                    .buffer
                    .get()
                    .cloned()
            });

            if let Some(ref buf) = buffer {
                let _ = imp.editor_buffer.set(buf.clone());
            }

            buffer
        }
    }

    pub(crate) fn find_view(&self) -> Option<crate::editor::view::TypesetterEditorView> {
        let imp = self.imp();

        if let Some(view) = imp.view.get() {
            return Some(view.clone());
        }

        let window = self.find_parent_window()?;
        let editor = window.imp().editor_container.get();
        let view = editor.imp().view.get();
        let _ = imp.view.set(view.clone());
        Some(view)
    }

    pub(super) fn update_preview(&self, text: &str) {
        let current_file_path = self
            .find_parent_window()
            .and_then(|w| w.current_file_path());

        let project_root = self.find_parent_window().and_then(|w| w.project_root());

        let text = text.to_string();

        self.handle_compilation_result(text, current_file_path, project_root);
    }

    pub fn scroll_to_preview_location(&self, location: &crate::typst_system::PreviewLocation) {
        if !self.is_visible() {
            return;
        }

        let preview_list = self.imp().preview_list.clone();
        let page_number = location.page_number;

        let Some(adjustment) = preview_list.vadjustment() else {
            return;
        };

        let current_value = adjustment.value();
        let page_size = adjustment.page_size();
        let upper = adjustment.upper();

        // Get model height from store
        let page_store = &self.imp().page_store;
        let total_pages = page_store.n_items() as f64;

        let model_height = page_store
            .item(page_number as u32)
            .and_downcast::<PreviewObject>()
            .map(|obj| {
                let (_width_pt, height_pt) = obj.dimensions();
                (height_pt as f64) * self.effective_pixels_per_point()
            })
            .unwrap_or(800.0);

        // Calculate position
        let page_percentage = (location.y as f64) / model_height;
        let margin = 0.0;
        let total_margins = margin * (total_pages + 1.0);
        let content_height = upper - total_margins;
        let page_height_in_scroll = content_height / total_pages;

        let page_top = margin + (page_number as f64 * (page_height_in_scroll + margin));
        let target_y_absolute = page_top + (page_percentage * page_height_in_scroll);
        let target_value = (target_y_absolute - (page_size / 3.0)).clamp(0.0, upper - page_size);

        // Skip if already there
        if (target_value - current_value).abs() < 1.0 {
            return;
        }

        // Animate
        let target = adw::CallbackAnimationTarget::new(glib::clone! {
            #[weak] adjustment,
            move |value| adjustment.set_value(value)
        });

        let anim =
            adw::TimedAnimation::new(&preview_list, current_value, target_value, 250, target);

        anim.set_easing(adw::Easing::EaseOutCubic);
        anim.play();
    }

    pub fn set_preview_visible(&self, visible: bool) {
        let was_visible = self.is_visible();
        self.set_is_visible(visible);

        if visible && !was_visible {
            let imp = self.imp();

            // Check if any visible page already has a valid texture for the current generation
            let current_gen = imp.current_generation.get();
            self.update_visible_pages_tracking();
            let visible_pages = imp.visible_pages.borrow().clone();
            let mut has_valid_visible_texture = false;

            for &p in &visible_pages {
                if let Some(obj) = imp.page_store.item(p as u32)
                    && let Ok(page) = obj.downcast::<PreviewObject>()
                    && page.generation() == current_gen
                    && page.has_texture()
                {
                    has_valid_visible_texture = true;
                    break;
                }
            }

            if has_valid_visible_texture {
                self.hide_loading();
            }

            if imp.page_store.n_items() > 0 {
                // Pages already compiled, just render them
                self.render_visible_pages();
            } else {
                // No compile has happened yet (e.g. new empty document with preview
                // pane closed at startup, then user opens it)
                if let Some(window) = self.find_parent_window() {
                    let text = window.imp().editor_container.contents();
                    if !text.trim().is_empty() {
                        self.update_preview(&text);
                    }
                }
            }
        } else if !visible && was_visible {
            // Preview hidden: clear textures from PreviewObjects to free memory.
            // Textures remain safely in TextureCache for fast restoration if re-opened.
            let imp = self.imp();
            for i in 0..imp.page_store.n_items() {
                if let Some(obj) = imp.page_store.item(i)
                    && let Ok(page) = obj.downcast::<PreviewObject>()
                    && page.has_texture()
                {
                    page.clear_texture();
                }
            }
        }
    }

    pub fn cleanup_textures(&self) {
        let imp = self.imp();

        // Clear the cache first
        imp.texture_cache.clear();

        // Collect all pages
        let items: Vec<_> = (0..imp.page_store.n_items())
            .filter_map(|i| imp.page_store.item(i))
            .filter_map(|obj| obj.downcast::<PreviewObject>().ok())
            .collect();

        // Clean up textures from all pages
        for page in items {
            page.cleanup_texture();
        }

        // Clear visible pages tracking
        imp.visible_pages.borrow_mut().clear();

        // More aggressive memory hints
        for _ in 0..5 {
            glib::idle_add_once(|| {});
        }
    }

    /// Attaches the `value-changed`/`page-size`/`upper` listeners (that
    /// drive `check_visible_pages()`) to `preview_list`'s current
    /// vadjustment, unless we're already attached to that exact instance.
    ///
    /// This is called both eagerly at setup and from the `notify::vadjustment`
    /// handler, because GTK doesn't guarantee that signal only fires when the
    /// adjustment object actually changes - a virtualized GtkListView can
    /// re-signal it for the same instance (model resets, recycling, being
    /// re-parented, etc). Without the identity check below, each such firing
    /// used to attach a *second* full set of listeners on top of the first,
    /// silently multiplying how many times a single scroll tick ran
    /// `check_visible_pages()` (and therefore how much duplicate work -
    /// worst case, duplicate `render_page_async()` calls - a scroll could
    /// trigger), with no way to ever undo it short of restarting the widget.
    fn bind_vadjustment_listeners(&self) {
        let imp = self.imp();
        let Some(vadj) = imp.preview_list.vadjustment() else {
            return;
        };

        let already_bound = imp
            .bound_vadjustment
            .borrow()
            .as_ref()
            .and_then(glib::WeakRef::upgrade)
            .is_some_and(|current| current == vadj);
        if already_bound {
            return;
        }
        imp.bound_vadjustment.replace(Some(vadj.downgrade()));

        vadj.connect_value_changed(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                obj.check_visible_pages("vadjustment value-changed");
            }
        ));
        vadj.connect_notify_local(
            Some("page-size"),
            glib::clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.check_visible_pages("vadjustment page-size notify");
                }
            ),
        );
        vadj.connect_notify_local(
            Some("upper"),
            glib::clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.check_visible_pages("vadjustment upper notify");
                }
            ),
        );
    }

    /// Same idempotent-attach pattern as `bind_vadjustment_listeners()`,
    /// but for the `hadjustment` "page-size" listener that drives
    /// `on_viewport_width_changed()`.
    fn bind_hadjustment_listeners(&self) {
        let imp = self.imp();
        let Some(hadj) = imp.preview_list.hadjustment() else {
            return;
        };

        let already_bound = imp
            .bound_hadjustment
            .borrow()
            .as_ref()
            .and_then(glib::WeakRef::upgrade)
            .is_some_and(|current| current == hadj);
        if already_bound {
            return;
        }
        imp.bound_hadjustment.replace(Some(hadj.downgrade()));

        hadj.connect_notify_local(
            Some("page-size"),
            glib::clone!(
                #[weak(rename_to = obj)]
                self,
                move |adj, _| {
                    let _ = adj;
                    obj.on_viewport_width_changed();
                }
            ),
        );
    }

    /// Check which pages are actually visible in the viewport
    fn check_visible_pages(&self, caller: &str) {
        // Don't render if preview is not visible
        if !self.is_visible() {
            return;
        }

        let imp = self.imp();

        // Debounce: only check every 200ms. This is a *trailing* debounce -
        // if we're inside the window, we don't just drop the call, we
        // schedule exactly one follow-up for when the window closes.
        // Confirmed via logging (2026-07 debugging session) that a plain
        // leading-edge drop could silently lose the one call that mattered:
        // settle_render_scale()'s call, made right after a compile finishes,
        // could land inside another caller's 200ms window and get dropped
        // with nothing to guarantee a later retry - whether the page ever
        // rendered came down to how fast that particular compile happened
        // to be relative to an arbitrary 200ms window.
        let now = glib::monotonic_time();
        let last = imp.last_viewport_check.get();
        if now - last < 200_000 {
            let remaining_ms = ((200_000 - (now - last)) / 1000).max(1) as u64;
            if imp.pending_viewport_recheck.borrow().is_none() {
                let caller = caller.to_string();
                let source_id = glib::timeout_add_local_once(
                    std::time::Duration::from_millis(remaining_ms),
                    glib::clone!(
                        #[weak(rename_to = obj)]
                        self,
                        move || {
                            obj.imp().pending_viewport_recheck.take();
                            obj.check_visible_pages(&format!("{} (trailing debounce)", caller));
                        }
                    ),
                );
                imp.pending_viewport_recheck.replace(Some(source_id));
            }
            return;
        }
        imp.last_viewport_check.set(now);

        let page_store = &imp.page_store;
        let total_pages = page_store.n_items() as usize;

        if total_pages == 0 {
            return;
        }

        // Get scroll position
        let Some(vadj) = imp.preview_list.vadjustment() else {
            return;
        };

        let viewport_top = vadj.value();
        let viewport_height = vadj.page_size();
        let viewport_bottom = viewport_top + viewport_height;
        let total_height = vadj.upper();

        // Calculate which pages are visible based on proportional distribution
        // Assume pages are evenly distributed across the total height
        // TODO: Of course, pages might not be equal height. This needs to be improved down the road.
        let buffer_pages = 3;
        let mut new_visible = std::collections::HashSet::new();

        if total_height > 0.0 {
            // Simple approach: divide total height by number of pages
            let avg_page_height = total_height / total_pages as f64;

            for i in 0..total_pages {
                let page_top = i as f64 * avg_page_height;
                let page_bottom = page_top + avg_page_height;

                // Check if page is in or near viewport with buffer
                let buffer = viewport_height * buffer_pages as f64;
                if page_bottom >= viewport_top - buffer && page_top <= viewport_bottom + buffer {
                    new_visible.insert(i);
                }
            }
        } else {
            // Fallback if height not available yet
            new_visible = (0..total_pages.min(7)).collect();
        }

        // Update textures based on visibility
        let old_visible = imp.visible_pages.borrow().clone();
        let current_gen = imp.current_generation.get();

        // Render newly visible pages
        for &page_num in new_visible.difference(&old_visible) {
            if let Some(obj) = page_store.item(page_num as u32)
                && let Ok(page) = obj.downcast::<PreviewObject>()
                && !page.has_texture()
                && !page.is_rendering()
            {
                self.render_page_async(current_gen, page_num);
            }
        }

        // Also check if any currently visible pages need re-rendering
        for &page_num in &new_visible {
            if let Some(obj) = page_store.item(page_num as u32)
                && let Ok(page) = obj.downcast::<PreviewObject>()
            {
                // Render if no texture, or if it needs update (content changed)
                if !page.is_rendering() && (!page.has_texture() || page.needs_update()) {
                    self.render_page_async(current_gen, page_num);
                }
            }
        }

        // Clear textures from PreviewObjects that are no longer visible.
        // Checking all pages in page_store guarantees no orphaned textures remain
        // attached to PreviewObjects after rapid scrolling past in-flight renders.
        for i in 0..total_pages {
            if !new_visible.contains(&i)
                && let Some(obj) = page_store.item(i as u32)
                && let Ok(page) = obj.downcast::<PreviewObject>()
                && page.has_texture()
            {
                page.clear_texture();
            }
        }

        *imp.visible_pages.borrow_mut() = new_visible;
    }

    /// Force re-render of all visible pages (called after compilation)
    pub(crate) fn render_visible_pages(&self) {
        let imp = self.imp();
        let current_gen = imp.current_generation.get();

        // Update viewport without triggering renders
        self.update_visible_pages_tracking();

        let visible = imp.visible_pages.borrow().clone();
        let page_store = &imp.page_store;

        if visible.is_empty() {
            return;
        }

        // Get the center of viewport to determine which page is most important
        let viewport_center = if let Some(vadj) = imp.preview_list.vadjustment() {
            vadj.value() + (vadj.page_size() / 2.0)
        } else {
            0.0
        };

        let total_pages = page_store.n_items() as usize;
        let total_height = imp
            .preview_list
            .vadjustment()
            .map(|vadj| vadj.upper())
            .unwrap_or(1.0);

        // Calculate which page is at the center of viewport
        let avg_page_height = if total_height > 0.0 && total_pages > 0 {
            total_height / total_pages as f64
        } else {
            1.0
        };

        let center_page = if avg_page_height > 0.0 {
            ((viewport_center / avg_page_height).floor() as usize)
                .min(total_pages.saturating_sub(1))
        } else {
            0
        };

        // Build priority list: center page first, then spiral outward
        let mut priority_order = Vec::new();
        priority_order.push(center_page);

        // Add pages spiraling out from center
        for distance in 1..total_pages {
            // Try page below center
            if center_page + distance < total_pages && visible.contains(&(center_page + distance)) {
                priority_order.push(center_page + distance);
            }
            // Try page above center
            if distance <= center_page && visible.contains(&(center_page - distance)) {
                priority_order.push(center_page - distance);
            }
        }

        // Add any remaining visible pages not yet in the list
        for &page_num in &visible {
            if !priority_order.contains(&page_num) {
                priority_order.push(page_num);
            }
        }

        // Render in priority order
        for page_num in priority_order {
            if let Some(obj) = page_store.item(page_num as u32)
                && let Ok(page) = obj.downcast::<PreviewObject>()
                && !page.is_rendering()
            {
                self.render_page_async(current_gen, page_num);
            }
        }

        // Schedule cleanup of old textures after all renders complete
        // This gives time for new textures to be created and set
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            // Hint that GC can run
            glib::idle_add_once(|| {
                // Idle callback to let Rust drop cycle run
            });
        });
    }

    /// Deliver the first page as soon as possible, then render remaining visible pages progressively
    pub(crate) fn render_first_page_then_visible(&self, generation: u64) {
        let imp = self.imp();
        let total_pages = imp.page_store.n_items() as usize;
        if total_pages == 0 {
            self.hide_loading();
            return;
        }

        // Priority 1: Render first page immediately
        self.render_page_async(generation, 0);

        // Priority 2: If multiple pages, schedule remaining visible pages in idle tick
        if total_pages > 1 {
            let self_clone = self.clone();
            glib::idle_add_local_once(move || {
                if self_clone.imp().current_generation.get() != generation {
                    return;
                }
                self_clone.render_remaining_visible_pages(generation);
            });
        }
    }

    /// Render remaining visible pages in priority order
    pub(crate) fn render_remaining_visible_pages(&self, generation: u64) {
        let imp = self.imp();
        if imp.current_generation.get() != generation {
            return;
        }

        self.update_visible_pages_tracking();
        let visible = imp.visible_pages.borrow().clone();
        let page_store = &imp.page_store;
        let total_pages = page_store.n_items() as usize;

        for page_num in 1..total_pages {
            if visible.contains(&page_num)
                && let Some(obj) = page_store.item(page_num as u32)
                && let Ok(page) = obj.downcast::<PreviewObject>()
                && !page.is_rendering()
                && (!page.has_texture() || page.needs_update())
            {
                self.render_page_async(generation, page_num);
            }
        }
    }

    /// Update visible pages tracking without triggering renders
    fn update_visible_pages_tracking(&self) {
        let imp = self.imp();

        let page_store = &imp.page_store;
        let total_pages = page_store.n_items() as usize;

        if total_pages == 0 {
            return;
        }

        let Some(vadj) = imp.preview_list.vadjustment() else {
            return;
        };

        let viewport_top = vadj.value();
        let viewport_height = vadj.page_size();
        let viewport_bottom = viewport_top + viewport_height;
        let total_height = vadj.upper();

        let buffer_pages = 3;
        let mut new_visible = std::collections::HashSet::new();

        if total_height > 0.0 {
            let avg_page_height = total_height / total_pages as f64;

            for i in 0..total_pages {
                let page_top = i as f64 * avg_page_height;
                let page_bottom = page_top + avg_page_height;

                let buffer = viewport_height * buffer_pages as f64;
                if page_bottom >= viewport_top - buffer && page_top <= viewport_bottom + buffer {
                    new_visible.insert(i);
                }
            }
        } else {
            new_visible = (0..total_pages.min(7)).collect();
        }

        *imp.visible_pages.borrow_mut() = new_visible;
    }

    /// Render a single page asynchronously
    fn render_page_async(&self, generation: u64, page_num: usize) {
        let imp = self.imp();

        if imp.current_generation.get() != generation {
            return;
        }

        // Get scale factor for high-DPI support
        let scale_factor = self.scale_factor() as f64;
        let ppp = self.effective_pixels_per_point();
        let pixel_per_pt = ppp * scale_factor;
        let scale_key = (pixel_per_pt * 100.0).round() as u32;

        // First check if we have it in cache for this exact generation and scale
        if let Some(cached_texture) = imp.texture_cache.get(generation, page_num, scale_key) {
            if let Some(obj) = imp.page_store.item(page_num as u32)
                && let Ok(page) = obj.downcast::<PreviewObject>()
            {
                page.set_texture(cached_texture);
                page.set_needs_update(false);
                self.on_page_texture_attached(generation, page_num);
            }
            return;
        }

        let Some(obj) = imp.page_store.item(page_num as u32) else {
            return;
        };

        let Ok(page) = obj.downcast::<PreviewObject>() else {
            return;
        };

        // Check if already rendering
        if page.is_rendering() {
            return;
        }

        // Mark needs_update false so check_visible_pages doesn't queue a second render while this one is in flight
        page.set_needs_update(false);
        page.set_rendering(true);

        // Invert-lightness and colorblind simulation are both applied at
        // paint time now via a GPU color matrix (see PreviewImage::
        // snapshot() and crate::utils::combined_transform), instead of
        // being baked into the rendered pixels, so neither needs to be
        // threaded through the compile/render path at all anymore.

        let page_clone = page.clone();
        let self_clone = self.clone();

        glib::spawn_future_local(async move {
            if let Some(window) = self_clone.find_parent_window()
                && let Ok(compiler) = window.compiler_system().await
            {
                // Render at the exact pixel_per_pt ratio (the rendering
                // worker no longer needs to re-derive it from a rounded
                // width)
                match compiler.render_page(generation, page_num, pixel_per_pt).await {
                    crate::typst_system::RenderResponse::PageImage(mut img) => {
                        let current_scale_key = (self_clone.effective_pixels_per_point()
                            * self_clone.scale_factor() as f64
                            * 100.0)
                            .round() as u32;

                        // Validate generation and zoom scale before updating UI or cache
                        if self_clone.imp().current_generation.get() != generation
                            || page_clone.generation() != generation
                            || current_scale_key != scale_key
                        {
                            #[cfg(debug_assertions)]
                            println!(
                                "TypesetterPreview: Discarding obsolete render result for page {} (generation {}, scale {})",
                                page_num, generation, scale_key
                            );
                            drop(img);
                            page_clone.set_rendering(false);
                            return;
                        }

                        // Create texture directly from owned pixels without copying
                        let texture = {
                            let stride = img.width as usize * 4;
                            let pixels = std::mem::take(&mut img.pixels);
                            let bytes = glib::Bytes::from_owned(pixels);

                            gtk::gdk::MemoryTexture::new(
                                img.width,
                                img.height,
                                gtk::gdk::MemoryFormat::R8g8b8a8,
                                &bytes,
                                stride,
                            )
                        };

                        // Track creation
                        #[cfg(debug_assertions)]
                        crate::memory_tracking::MEMORY_COUNTERS
                            .textures_created
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                        // Store in cache
                        self_clone
                            .imp()
                            .texture_cache
                            .insert(generation, page_num, scale_key, texture.clone());

                        // Set on page object only if currently visible (or initial unallocated pass)
                        let is_visible = self_clone.imp().visible_pages.borrow().contains(&page_num)
                            || self_clone.imp().visible_pages.borrow().is_empty();

                        if is_visible {
                            page_clone.set_texture(texture);
                            page_clone.set_needs_update(false);
                            self_clone.on_page_texture_attached(generation, page_num);
                        } else {
                            // Page scrolled out of view while render was in-flight.
                            // The texture is cached for fast retrieval when scrolled to,
                            // but not retained on PreviewObject to prevent memory leakage.
                            page_clone.set_needs_update(false);
                        }
                    }
                    crate::typst_system::RenderResponse::Cancelled => {
                        #[cfg(debug_assertions)]
                        println!(
                            "TypesetterPreview: Render cancelled for page {} (gen {})",
                            page_num, generation
                        );
                    }
                    crate::typst_system::RenderResponse::Error(e) => {
                        eprintln!("Failed to render page {}: {}", page_num, e);
                        self_clone.hide_loading();
                    }
                }
            } else {
                self_clone.hide_loading();
            }
            page_clone.set_rendering(false);
        });
    }

    /// Activate magnifier with normalized page coordinates
    pub fn activate_magnifier(
        &self,
        page_num: usize,
        list_x: f64,
        list_y: f64,
        norm_x: f64,
        norm_y: f64,
    ) {
        let imp = self.imp();

        // If a previous magnifier is still fading out (or in), stop that
        // animation and finish tearing down the old widget immediately -
        // otherwise we'd end up with two overlapping magnifiers.
        if let Some(anim) = imp.magnifier_fade_animation.take() {
            anim.pause();
        }
        if let Some(old_magnifier) = imp.magnifier.take() {
            old_magnifier.set_texture(None);
            old_magnifier.unparent();
        }

        let zoom_factor = 2.5;

        // Transform coordinates from preview_list to preview_scroller space
        let (overlay_x, overlay_y) = {
            let point = gtk::graphene::Point::new(list_x as f32, list_y as f32);
            if let Some(translated) = imp
                .preview_list
                .compute_point(&*imp.preview_scroller, &point)
            {
                (translated.x() as f64, translated.y() as f64)
            } else {
                // Fallback if transformation fails
                (list_x, list_y)
            }
        };

        let invert = self
            .imp()
            .action_group
            .get()
            .and_then(|g| g.action_state("invert-lightness"))
            .and_then(|v| v.get::<bool>())
            .unwrap_or(false);
        let is_dark = adw::StyleManager::default().is_dark();
        let cb_mode = self
            .imp()
            .action_group
            .get()
            .and_then(|g| g.action_state("simulate-color-blindness"))
            .and_then(|v| v.get::<String>())
            .unwrap_or_else(|| "none".to_string());

        // Create magnifier widget
        let magnifier = PreviewMagnifier::new(invert && is_dark);
        magnifier.set_cb_mode(&cb_mode);
        magnifier.set_parent(&*imp.preview_scroller);
        magnifier.set_position(overlay_x, overlay_y); // Use transformed coordinates
        magnifier.set_zoom_factor(zoom_factor);
        magnifier.set_opacity(0.0);
        magnifier.set_visible(true);

        magnifier.allocate(
            imp.preview_scroller.width(),
            imp.preview_scroller.height(),
            -1,
            None,
        );

        imp.magnifier.replace(Some(magnifier.clone()));
        self.update_toolbar_visibility();

        let target = adw::CallbackAnimationTarget::new(glib::clone!(
            #[weak]
            magnifier,
            move |value| {
                magnifier.set_opacity(value);
            }
        ));
        let animation = adw::TimedAnimation::new(&magnifier, 0.0, 1.0, 200, target);
        animation.set_easing(adw::Easing::EaseOutCubic);
        imp.magnifier_fade_animation
            .replace(Some(animation.clone()));
        animation.play();

        self.render_magnifier_texture(
            page_num,
            zoom_factor,
            move |texture: Option<gtk::gdk::MemoryTexture>| {
                if let Some(tex) = texture {
                    let tex_width = tex.width() as f64;
                    let tex_height = tex.height() as f64;

                    let source_x = norm_x * tex_width;
                    let source_y = norm_y * tex_height;

                    magnifier.set_texture(Some(tex.upcast()));
                    magnifier.set_source_position(source_x, source_y);
                }
            },
        );
    }

    /// Update magnifier position with normalized page coordinates
    pub fn update_magnifier_position(&self, list_x: f64, list_y: f64, norm_x: f64, norm_y: f64) {
        let imp = self.imp();

        if let Some(magnifier) = imp.magnifier.borrow().as_ref() {
            // Transform coordinates from preview_list to preview_scroller space
            let (overlay_x, overlay_y) = {
                let point = gtk::graphene::Point::new(list_x as f32, list_y as f32);
                if let Some(translated) = imp
                    .preview_list
                    .compute_point(&*imp.preview_scroller, &point)
                {
                    (translated.x() as f64, translated.y() as f64)
                } else {
                    (list_x, list_y)
                }
            };

            // Update magnifier position in overlay coordinates
            magnifier.set_position(overlay_x, overlay_y);

            // Update source position using normalized coordinates
            if let Some(texture) = magnifier.imp().texture.borrow().as_ref() {
                let tex_width = texture.width() as f64;
                let tex_height = texture.height() as f64;

                let source_x = norm_x * tex_width;
                let source_y = norm_y * tex_height;

                magnifier.set_source_position(source_x, source_y);
            }
        }
    }

    pub fn deactivate_magnifier(&self) {
        let imp = self.imp();

        let Some(magnifier) = imp.magnifier.borrow().clone() else {
            return;
        };

        // Stop any in-flight fade (most likely still fading in) and start
        // the fade-out from wherever the opacity currently is, rather than
        // assuming it was fully visible.
        if let Some(anim) = imp.magnifier_fade_animation.take() {
            anim.pause();
        }

        let current_opacity = magnifier.opacity();

        let target = adw::CallbackAnimationTarget::new(glib::clone!(
            #[weak]
            magnifier,
            move |value| {
                magnifier.set_opacity(value);
            }
        ));
        let animation = adw::TimedAnimation::new(&magnifier, current_opacity, 0.0, 200, target);
        animation.set_easing(adw::Easing::EaseOutCubic);

        animation.connect_done(glib::clone!(
            #[weak(rename_to = preview)]
            self,
            move |_| {
                let imp = preview.imp();
                imp.magnifier_fade_animation.take();

                if let Some(magnifier) = imp.magnifier.take() {
                    // Clear texture explicitly first
                    magnifier.set_texture(None);

                    // Unparent widget (triggers dispose)
                    magnifier.unparent();

                    // Explicit drop to ensure cleanup happens now
                    drop(magnifier);
                }

                preview.update_toolbar_visibility();
            }
        ));

        imp.magnifier_fade_animation
            .replace(Some(animation.clone()));
        animation.play();
    }

    fn render_magnifier_texture(
        &self,
        page_num: usize,
        zoom_factor: f64,
        callback: impl FnOnce(Option<gtk::gdk::MemoryTexture>) + 'static,
    ) {
        let self_clone = self.clone();

        glib::spawn_future_local(async move {
            // Check if magnifier is still active before doing expensive work
            if self_clone.imp().magnifier.borrow().is_none() {
                return;
            }

            if let Some(window) = self_clone.find_parent_window()
                && let Ok(compiler) = window.compiler_system().await
            {
                // Check again after await - magnifier might have been deactivated
                if self_clone.imp().magnifier.borrow().is_none() {
                    return;
                }

                // Get scale factor for high-DPI support
                let scale_factor = self_clone.scale_factor() as f64;

                // For the magnifier, we want zoom_factor worth of detail in logical pixels,
                // but rendered at scale_factor physical pixels for sharpness.
                // Passed as an exact ratio rather than a pre-rounded target
                // width - see RenderingWorker::render_page's doc comment.
                let pixel_per_pt =
                    self_clone.effective_pixels_per_point() * zoom_factor * scale_factor;

                let current_gen = self_clone.imp().current_generation.get();
                match compiler.render_page(current_gen, page_num, pixel_per_pt).await {
                    crate::typst_system::RenderResponse::PageImage(mut img) => {
                        // Final check before setting texture
                        if self_clone.imp().magnifier.borrow().is_none() {
                            // Explicitly drop the image data
                            drop(img);
                            return;
                        }

                        // Invert-lightness and colorblind simulation are
                        // both handled by PreviewMagnifier's own snapshot()
                        // now (GPU color matrix, set via
                        // set_invert_lightness()/set_cb_mode() at activation
                        // time below) rather than baked in here.

                        let stride = img.width as usize * 4;
                        let pixels = std::mem::take(&mut img.pixels);
                        let bytes = glib::Bytes::from_owned(pixels);
                        let texture = gtk::gdk::MemoryTexture::new(
                            img.width,
                            img.height,
                            gtk::gdk::MemoryFormat::R8g8b8a8,
                            &bytes,
                            stride,
                        );

                        // Drop image data immediately after texture creation
                        drop(img);

                        callback(Some(texture));
                    }
                    _ => callback(None),
                }
            }
        });
    }
}
