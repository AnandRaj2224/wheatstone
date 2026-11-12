// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::subclass::prelude::ObjectSubclassIsExt;
use gtk::{gdk, gio, glib, prelude::*};
use sourceview;
use std::cell::{Cell, RefCell};

const MIN_MARGIN: i32 = 16;
const CENTERED_SCROLL_POSITION: f64 = 0.5;
/// Default line length in characters, used when no user override is set.
const DEFAULT_CHARS_PER_LINE: f64 = 65.0;
const MIN_CHARS_PER_LINE: f64 = 30.0;
const MAX_CHARS_PER_LINE: f64 = 120.0;
const CHARS_PER_LINE_STEP: f64 = 5.0;
const AVG_CHAR_WIDTH: f64 = 0.605;
/// Fallback DPI used if the display's GtkSettings are unavailable.
const DPI: f64 = 96.0;
const POINTS_PER_INCH: f64 = 72.0;
/// Zoom step ratio: 2^(1/7) ≈ 1.1047. Using a fixed ratio means every
/// zoom-in is exactly undone by a zoom-out (x * r / r = x).
const ZOOM_STEP: f32 = 1.104_729_f32;

mod imp {
    use super::*;
    use glib::subclass::prelude::*;
    use gtk::subclass::{prelude::*, widget::WidgetImplExt};
    use sourceview::prelude::ViewExt;

    struct BufferActionGuard<'a>(&'a sourceview::Buffer);

    impl<'a> Drop for BufferActionGuard<'a> {
        fn drop(&mut self) {
            self.0.end_user_action();
        }
    }

    #[derive(Default)]
    pub struct TypesetterEditorView {
        pub last_font_size: Cell<f32>,
        /// When `Some`, disables dynamic font scaling and uses this fixed pt
        /// value instead. Set by zoom-in/zoom-out, cleared by zoom-reset.
        pub font_size_override: Cell<Option<f32>>,
        /// Desired line length in characters. `0.0` (the `Cell` default)
        /// means "unset"; use `chars_per_line()` to read the effective
        /// value, which falls back to `DEFAULT_CHARS_PER_LINE`. Set by
        /// line-length-increase/decrease, cleared by line-length-reset.
        pub chars_per_line: Cell<f64>,
        /// CssProvider used to apply the font size to the widget.
        pub font_provider: RefCell<gtk::CssProvider>,
        /// Unique per-instance CSS class used to scope the font-size rule, so
        /// it can't be overridden by another window's CssProvider sharing the
        /// same display.
        pub instance_css_class: std::cell::OnceCell<String>,
        /// Accumulates Ctrl+scroll delta until it crosses a threshold,
        /// preventing touchpad smooth scrolling from zooming too fast.
        pub scroll_zoom_accumulator: Cell<f64>,
        pub scroll_controller: RefCell<Option<gtk::EventControllerScroll>>,
        pub cached_scroll_fraction: Cell<f64>,
        pub centered_scrolling_active: Cell<bool>,
        pub buffer_cursor_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub buffer_insert_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub click_controller: RefCell<Option<gtk::GestureClick>>,
        pub key_controller: RefCell<Option<gtk::EventControllerKey>>,
        pub motion_controller: RefCell<Option<gtk::EventControllerMotion>>,
        pub settings_handler: RefCell<Option<glib::SignalHandlerId>>,
        /// Handler for `gtk-xft-dpi` notify signal on the display's
        /// GtkSettings, used to relayout when the text-scaling / "Large
        /// Text" accessibility setting changes at runtime.
        pub gtk_settings_handler: RefCell<Option<glib::SignalHandlerId>>,
        // pub vim_im_context: RefCell<Option<sourceview::VimIMContext>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterEditorView {
        const NAME: &'static str = "TypesetterEditorView";
        type Type = super::TypesetterEditorView;
        type ParentType = sourceview::View;

        fn class_init(klass: &mut Self::Class) {
            // Completion action
            klass.install_action("buffer.trigger-completion", None, |view, _, _| {
                let sv_view = view.upcast_ref::<sourceview::View>();
                let completion = sv_view.completion();
                completion.show();
            });
            klass.add_binding_action(
                gdk::Key::space,
                gdk::ModifierType::CONTROL_MASK,
                "buffer.trigger-completion",
            );

            // Spelling navigation actions
            klass.install_action("buffer.goto-next-spelling-error", None, |view, _, _| {
                if let Some(editor) = Self::find_typesetter_editor(view.parent()) {
                    let adapter = editor.imp().spell_adapter.borrow();
                    if let Some(adapter) = adapter.as_ref()
                        && let Some(tag) = adapter.tag()
                    {
                        Self::goto_text_tag(view, &tag, true);
                    }
                }
            });
            klass.add_binding_action(
                gdk::Key::bracketright,
                gdk::ModifierType::CONTROL_MASK,
                "buffer.goto-next-spelling-error",
            );

            klass.install_action("buffer.goto-previous-spelling-error", None, |view, _, _| {
                if let Some(editor) = Self::find_typesetter_editor(view.parent()) {
                    let adapter = editor.imp().spell_adapter.borrow();
                    if let Some(adapter) = adapter.as_ref()
                        && let Some(tag) = adapter.tag()
                    {
                        Self::goto_text_tag(view, &tag, false);
                    }
                }
            });
            klass.add_binding_action(
                gdk::Key::bracketleft,
                gdk::ModifierType::CONTROL_MASK,
                "buffer.goto-previous-spelling-error",
            );

            // Grammar navigation actions
            klass.install_action("buffer.goto-next-grammar-error", None, |view, _, _| {
                if let Some(editor) = Self::find_typesetter_editor(view.parent()) {
                    let buffer = editor.buffer();
                    if let Some(tag) = buffer.tag_table().lookup("harper-lint") {
                        Self::goto_text_tag(view, &tag, true);
                    }
                }
            });
            klass.add_binding_action(
                gdk::Key::braceright,
                gdk::ModifierType::CONTROL_MASK,
                "buffer.goto-next-grammar-error",
            );

            klass.install_action("buffer.goto-previous-grammar-error", None, |view, _, _| {
                if let Some(editor) = Self::find_typesetter_editor(view.parent()) {
                    let buffer = editor.buffer();
                    if let Some(tag) = buffer.tag_table().lookup("harper-lint") {
                        Self::goto_text_tag(view, &tag, false);
                    }
                }
            });
            klass.add_binding_action(
                gdk::Key::braceleft,
                gdk::ModifierType::CONTROL_MASK,
                "buffer.goto-previous-grammar-error",
            );

            klass.install_action("buffer.toggle-comment", None, move |window, _, _| {
                if let Some(editor) = Self::find_typesetter_editor(window.parent()) {
                    Self::toggle_line_comment(editor.buffer());
                }
            });
            klass.add_binding_action(
                gdk::Key::m,
                gdk::ModifierType::CONTROL_MASK,
                "buffer.toggle-comment",
            );

            // Text formatting keyboard shortcuts
            let text_shortcuts = [
                ("buffer.toggle-bold", gdk::Key::b, "*"),
                ("buffer.toggle-italics", gdk::Key::i, "_"),
                ("buffer.toggle-code", gdk::Key::grave, "`"),
                ("buffer.toggle-math", gdk::Key::dollar, "$"),
            ];

            for (action, key, marker) in text_shortcuts {
                klass.install_action(action, None, move |window, _, _| {
                    if let Some(editor) = Self::find_typesetter_editor(window.parent()) {
                        Self::toggle_text_formatting(editor.buffer(), marker);
                    }
                });
                klass.add_binding_action(key, gdk::ModifierType::CONTROL_MASK, action);
            }

            // Heading keyboard shortcuts
            let heading_shortcuts = [
                (gdk::Key::_1, "="),
                (gdk::Key::_2, "=="),
                (gdk::Key::_3, "==="),
                (gdk::Key::_4, "===="),
                (gdk::Key::_5, "====="),
                (gdk::Key::_6, "======"),
            ];

            for (i, (key, marker)) in heading_shortcuts.iter().enumerate() {
                let action_name = format!("buffer.toggle-heading-{}", i + 1);
                let marker = *marker;
                klass.install_action(&action_name, None, move |window, _, _| {
                    if let Some(editor) = Self::find_typesetter_editor(window.parent()) {
                        Self::toggle_paragraph_formatting(editor.buffer(), marker);
                    }
                });
                klass.add_binding_action(*key, gdk::ModifierType::CONTROL_MASK, &action_name);
            }

            // List keyboard shortcuts
            let list_shortcuts = [
                ("buffer.toggle-ordered-list", gdk::Key::l, "+"),
                ("buffer.toggle-unordered-list", gdk::Key::u, "-"),
            ];

            for (action, key, marker) in list_shortcuts {
                klass.install_action(action, None, move |window, _, _| {
                    if let Some(editor) = Self::find_typesetter_editor(window.parent()) {
                        Self::toggle_paragraph_formatting(editor.buffer(), marker);
                    }
                });
                klass.add_binding_action(key, gdk::ModifierType::CONTROL_MASK, action);
            }

            // Zoom actions. zoom-in/zoom-out fix the font size at the current
            // effective size (dynamic or already fixed) plus/minus a step,
            // disabling dynamic scaling. zoom-reset clears the override and
            // re-enables dynamic scaling.
            klass.install_action("buffer.zoom-in", None, |view, _, _| {
                let imp = view.imp();
                let current = imp
                    .font_size_override
                    .get()
                    .unwrap_or_else(|| imp.last_font_size.get());
                let new_size = (current * ZOOM_STEP * 10.0).round() / 10.0;
                if new_size <= 25.0 {
                    imp.font_size_override.set(Some(new_size));
                    view.queue_allocate();
                }
            });
            klass.add_binding_action(
                gdk::Key::equal,
                gdk::ModifierType::CONTROL_MASK,
                "buffer.zoom-in",
            );
            klass.add_binding_action(
                gdk::Key::plus,
                gdk::ModifierType::CONTROL_MASK,
                "buffer.zoom-in",
            );

            klass.install_action("buffer.zoom-out", None, |view, _, _| {
                let imp = view.imp();
                let current = imp
                    .font_size_override
                    .get()
                    .unwrap_or_else(|| imp.last_font_size.get());
                let new_size = (current / ZOOM_STEP * 10.0).round() / 10.0;
                if new_size >= 9.0 {
                    imp.font_size_override.set(Some(new_size));
                    view.queue_allocate();
                }
            });
            klass.add_binding_action(
                gdk::Key::minus,
                gdk::ModifierType::CONTROL_MASK,
                "buffer.zoom-out",
            );

            klass.install_action("buffer.zoom-reset", None, |view, _, _| {
                // Clear the override to re-enable dynamic font scaling.
                view.imp().font_size_override.set(None);
                view.queue_allocate();
            });
            klass.add_binding_action(
                gdk::Key::_0,
                gdk::ModifierType::CONTROL_MASK,
                "buffer.zoom-reset",
            );

            // Line-length actions. Adjust the number of characters targeted
            // per line, which in turn changes the editor's margins.
            klass.install_action("buffer.line-length-increase", None, |view, _, _| {
                let imp = view.imp();
                let current = imp.chars_per_line();
                let new_value =
                    (current + CHARS_PER_LINE_STEP).clamp(MIN_CHARS_PER_LINE, MAX_CHARS_PER_LINE);
                imp.chars_per_line.set(new_value);
                view.queue_allocate();
            });
            klass.add_binding_action(
                gdk::Key::equal,
                gdk::ModifierType::ALT_MASK,
                "buffer.line-length-increase",
            );

            klass.install_action("buffer.line-length-decrease", None, |view, _, _| {
                let imp = view.imp();
                let current = imp.chars_per_line();
                let new_value =
                    (current - CHARS_PER_LINE_STEP).clamp(MIN_CHARS_PER_LINE, MAX_CHARS_PER_LINE);
                imp.chars_per_line.set(new_value);
                view.queue_allocate();
            });
            klass.add_binding_action(
                gdk::Key::minus,
                gdk::ModifierType::ALT_MASK,
                "buffer.line-length-decrease",
            );

            klass.install_action("buffer.line-length-reset", None, |view, _, _| {
                view.imp().chars_per_line.set(0.0);
                view.queue_allocate();
            });
            klass.add_binding_action(
                gdk::Key::_0,
                gdk::ModifierType::ALT_MASK,
                "buffer.line-length-reset",
            );
        }
    }

    impl ObjectImpl for TypesetterEditorView {}

    impl WidgetImpl for TypesetterEditorView {
        fn realize(&self) {
            self.parent_realize();

            let obj = self.obj();

            // Get Settings from the window (don't create a new one!)
            let Some(settings) = Self::get_settings_from_window(&obj) else {
                #[cfg(debug_assertions)]
                eprintln!("EditorView: Could not find window Settings");
                return;
            };

            // Setup settings change handler
            let handler_id = settings.connect_changed(
                Some("centered-scrolling"),
                glib::clone! {
                    #[weak] obj,
                    move |settings, _| {
                        let imp = obj.imp();
                        imp.handle_settings_change(settings, &obj);
                    }
                },
            );

            // Store the handler ID so we can disconnect it later
            self.settings_handler.replace(Some(handler_id));

            // Relayout when the desktop's text-scaling / "Large Text"
            // accessibility setting changes.
            let gtk_settings = gtk::Settings::for_display(&obj.display());
            let gtk_settings_handler_id = gtk_settings.connect_notify_local(
                Some("gtk-xft-dpi"),
                glib::clone! {
                    #[weak] obj,
                    move |_, _| obj.queue_resize()
                },
            );
            self.gtk_settings_handler
                .replace(Some(gtk_settings_handler_id));

            // Give this view a unique CSS class so its font-size rule can't be
            // overridden by another window's view sharing the same display.
            static NEXT_VIEW_ID: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let id = NEXT_VIEW_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let css_class = format!("typesetter-editor-view-{id}");
            obj.add_css_class(&css_class);
            let _ = self.instance_css_class.set(css_class);

            // Attach the font CssProvider so we can set exact pt sizes.
            let provider = self.font_provider.borrow().clone();
            gtk::style_context_add_provider_for_display(
                &obj.display(),
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );

            // Ctrl+scroll to zoom
            let scroll_controller =
                gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
            scroll_controller.connect_scroll(glib::clone! {
                #[weak] obj,
                #[upgrade_or] glib::Propagation::Proceed,
                move |controller, _dx, dy| {
                    if controller.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                        let imp = obj.imp();
                        let acc = imp.scroll_zoom_accumulator.get() + dy;
                        if acc.abs() >= 0.5 {
                            if acc < 0.0 {
                                obj.activate_action("buffer.zoom-in", None).ok();
                            } else {
                                obj.activate_action("buffer.zoom-out", None).ok();
                            }
                            imp.scroll_zoom_accumulator.set(0.0);
                        } else {
                            imp.scroll_zoom_accumulator.set(acc);
                        }
                        return glib::Propagation::Stop;
                    }
                    // Reset accumulator when Ctrl is released mid-scroll
                    obj.imp().scroll_zoom_accumulator.set(0.0);
                    glib::Propagation::Proceed
                }
            });
            obj.add_controller(scroll_controller.clone());
            self.scroll_controller.replace(Some(scroll_controller));

            // Setup centered scrolling if enabled
            if settings.boolean("centered-scrolling") {
                self.setup_centered_scrolling(&obj);
            }

            // Setup vim mode if enabled
            // if settings.boolean("vim-mode") {
            //     self.setup_vim_mode(&obj);
            // }
        }

        fn unrealize(&self) {
            // Disconnect settings handler
            if let Some(handler_id) = self.settings_handler.borrow_mut().take()
                && let Some(settings) = Self::get_settings_from_window(&self.obj())
            {
                settings.disconnect(handler_id);
            }

            // Disconnect gtk-xft-dpi handler
            if let Some(handler_id) = self.gtk_settings_handler.borrow_mut().take() {
                let gtk_settings = gtk::Settings::for_display(&self.obj().display());
                gtk_settings.disconnect(handler_id);
            }

            // Remove the font CssProvider from the display
            gtk::style_context_remove_provider_for_display(
                &self.obj().display(),
                &*self.font_provider.borrow(),
            );

            // Remove scroll controller
            if let Some(controller) = self.scroll_controller.borrow_mut().take() {
                self.obj().remove_controller(&controller);
            }

            // Clean up centered scrolling if it's active
            self.cleanup_centered_scrolling();

            self.parent_unrealize();
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let obj = self.obj();

            // Use the fixed override if set, otherwise compute dynamically.
            let font_size_pt = self
                .font_size_override
                .get()
                .unwrap_or_else(|| Self::compute_font_size(width));

            // Apply the font size if it changed.
            if self.last_font_size.replace(font_size_pt) != font_size_pt {
                let css_class = self
                    .instance_css_class
                    .get()
                    .expect("instance_css_class set in realize");
                let css =
                    format!(".{css_class} {{ font-size: {font_size_pt}pt; line-height: 1.3; }}");
                self.font_provider.borrow().load_from_string(&css);
            }

            // Compute the max line width in px for this font size, using the
            // effective DPI so the "Large Text" accessibility setting is
            // accounted for.
            let dpi = Self::effective_dpi(&obj);
            let max_width = Self::compute_line_width(font_size_pt, dpi, self.chars_per_line());

            // Calculate left/right margins to center text within the allocation
            let margin = if width > max_width {
                std::cmp::max((width - max_width) / 2, MIN_MARGIN)
            } else {
                MIN_MARGIN
            };

            obj.set_left_margin(margin);
            obj.set_right_margin(margin);

            // Set bottom margin so the last line can be scrolled to the vertical
            // centre, matching the autocenter-caret behaviour. iter_location()
            // returns buffer-space glyph metrics unaffected by scroll position.
            let line_height = obj.iter_location(&obj.buffer().start_iter()).height();
            let bottom_margin = if line_height > 0 {
                ((height - line_height) / 2).max(MIN_MARGIN)
            } else {
                (height / 2).max(MIN_MARGIN)
            };
            obj.set_bottom_margin(bottom_margin);

            // Cache the exact scroll fraction for use by centered-scrolling
            // handlers. Derived from the same values so it stays in sync.
            let scroll_fraction = if line_height > 0 && height > 0 {
                ((height - line_height) as f64 / (2.0 * height as f64)).clamp(0.0, 1.0)
            } else {
                CENTERED_SCROLL_POSITION
            };
            self.cached_scroll_fraction.set(scroll_fraction);

            // Continue normal size allocation
            self.parent_size_allocate(width, height, baseline);
        }
    }

    impl gtk::subclass::text_view::TextViewImpl for TypesetterEditorView {}

    impl sourceview::subclass::view::ViewImpl for TypesetterEditorView {}

    impl TypesetterEditorView {
        /// Returns the cached scroll fraction for centered scrolling. This is
        /// computed once per resize in size_allocate.
        pub(crate) fn cached_scroll_fraction(&self) -> f64 {
            let fraction = self.cached_scroll_fraction.get();
            // 0.0 means uninitialised (widget not yet laid out)
            if fraction == 0.0 {
                CENTERED_SCROLL_POSITION
            } else {
                fraction
            }
        }

        /// Returns the effective line length in characters, i.e. the
        /// user's override if one is set via line-length-increase/decrease,
        /// otherwise `DEFAULT_CHARS_PER_LINE`.
        fn chars_per_line(&self) -> f64 {
            let value = self.chars_per_line.get();
            // 0.0 means unset (never overridden, or reset).
            if value == 0.0 {
                DEFAULT_CHARS_PER_LINE
            } else {
                value
            }
        }

        /// Get Settings from ancestor TypesetterWindow
        fn get_settings_from_window(view: &super::TypesetterEditorView) -> Option<gio::Settings> {
            let mut widget = Some(view.clone().upcast::<gtk::Widget>());

            while let Some(w) = widget {
                if let Ok(window) = w.clone().downcast::<crate::TypesetterWindow>() {
                    return Some(window.settings());
                }
                widget = w.parent();
            }

            None
        }

        fn handle_settings_change(
            &self,
            settings: &gio::Settings,
            view: &super::TypesetterEditorView,
        ) {
            let should_be_enabled = settings.boolean("centered-scrolling");
            let is_currently_enabled = self.buffer_cursor_handler.borrow().is_some();

            if should_be_enabled && !is_currently_enabled {
                self.setup_centered_scrolling(view);
            } else if !should_be_enabled && is_currently_enabled {
                self.cleanup_centered_scrolling();
            }
        }

        fn setup_centered_scrolling(&self, view: &super::TypesetterEditorView) {
            // Get the buffer directly from the view
            let buffer = view.buffer();

            // Handle cursor movement (arrow keys, etc.)
            let mark_handler = buffer.connect_mark_set(glib::clone! {
                #[weak] view,
                move |_, _location, mark| {
                    if mark.name().as_deref() != Some("insert") {
                        return;
                    }

                    let imp = view.imp();
                    if imp.centered_scrolling_active.get() {
                        let fraction = imp.cached_scroll_fraction();
                        glib::idle_add_local_once(glib::clone! {
                            #[weak] view,
                            move || {
                                let buffer = view.buffer();
                                let insert_mark = buffer.get_insert();
                                let mut iter = buffer.iter_at_mark(&insert_mark);
                                view.scroll_to_iter(&mut iter, 0.0, true, 0.0, fraction);
                            }
                        });
                    }
                }
            });
            self.buffer_cursor_handler.replace(Some(mark_handler));

            // Handle changes to the editor buffer (insertions/deletions)
            let changed_handler = buffer.connect_changed(glib::clone! {
                #[weak] view,
                move |_| {
                    let imp = view.imp();
                    if !imp.centered_scrolling_active.get() {
                        imp.centered_scrolling_active.set(true);
                    }

                    let fraction = imp.cached_scroll_fraction();
                    glib::idle_add_local_once(glib::clone! {
                        #[weak] view,
                        move || {
                            let buffer = view.buffer();
                            let insert_mark = buffer.get_insert();
                            let mut iter = buffer.iter_at_mark(&insert_mark);
                            view.scroll_to_iter(&mut iter, 0.0, true, 0.0, fraction);
                        }
                    });
                }
            });
            self.buffer_insert_handler.replace(Some(changed_handler));

            // Disable centered scrolling on click (user is selecting/navigating)
            let click_controller = gtk::GestureClick::new();
            click_controller.connect_pressed(glib::clone! {
                #[weak] view,
                move |_, _, _, _| {
                    let imp = view.imp();
                    if imp.centered_scrolling_active.get() {
                        imp.centered_scrolling_active.set(false);
                    }
                }
            });
            view.add_controller(click_controller.clone());
            self.click_controller.replace(Some(click_controller));

            // Re-enable centered scrolling on any key press
            let key_controller = gtk::EventControllerKey::new();
            key_controller.connect_key_pressed(glib::clone! {
                #[weak] view,
                #[upgrade_or] glib::Propagation::Proceed,
                move |_, _, _, _| {
                    let imp = view.imp();
                    if !imp.centered_scrolling_active.get() {
                        imp.centered_scrolling_active.set(true);
                    }
                    glib::Propagation::Proceed
                }
            });
            view.add_controller(key_controller.clone());
            self.key_controller.replace(Some(key_controller));

            // Disable centered scrolling on mouse movement
            let motion_controller = gtk::EventControllerMotion::new();
            motion_controller.connect_motion(glib::clone! {
                #[weak] view,
                move |_, _, _| {
                    let imp = view.imp();
                    if imp.centered_scrolling_active.get() {
                        imp.centered_scrolling_active.set(false);
                    }
                }
            });
            view.add_controller(motion_controller.clone());
            self.motion_controller.replace(Some(motion_controller));
        }

        fn cleanup_centered_scrolling(&self) {
            let obj = self.obj();
            let buffer = obj.buffer();

            // Disconnect buffer signal handlers
            if let Some(handler_id) = self.buffer_cursor_handler.borrow_mut().take() {
                buffer.disconnect(handler_id);
            }

            if let Some(handler_id) = self.buffer_insert_handler.borrow_mut().take() {
                buffer.disconnect(handler_id);
            }

            // Remove event controllers
            if let Some(controller) = self.click_controller.borrow_mut().take() {
                obj.remove_controller(&controller);
            }

            if let Some(controller) = self.key_controller.borrow_mut().take() {
                obj.remove_controller(&controller);
            }

            if let Some(controller) = self.motion_controller.borrow_mut().take() {
                obj.remove_controller(&controller);
            }

            // Reset the active flag
            self.centered_scrolling_active.set(false);
        }

        fn find_typesetter_editor(
            mut widget: Option<gtk::Widget>,
        ) -> Option<crate::editor::TypesetterEditor> {
            while let Some(w) = widget {
                if let Ok(editor) = w.clone().downcast::<crate::editor::TypesetterEditor>() {
                    return Some(editor);
                }
                widget = w.parent();
            }
            None
        }

        fn toggle_line_comment(buffer: &sourceview::Buffer) {
            const COMMENT_PREFIX: &str = "// ";
            const COMMENT_PREFIX_BARE: &str = "//";

            fn is_commented(line: &str) -> bool {
                let trimmed = line.trim_start();
                trimmed.starts_with(COMMENT_PREFIX) || trimmed.starts_with(COMMENT_PREFIX_BARE)
            }

            fn toggle_comment(l: &str, comment: bool) -> String {
                let indent = l.len() - l.trim_start().len();
                let after_indent = &l[indent..];
                if comment {
                    format!("{}{COMMENT_PREFIX}{after_indent}", &l[..indent])
                } else if let Some(stripped) = after_indent.strip_prefix(COMMENT_PREFIX) {
                    format!("{}{stripped}", &l[..indent])
                } else if let Some(stripped) = after_indent.strip_prefix(COMMENT_PREFIX_BARE) {
                    format!("{}{stripped}", &l[..indent])
                } else {
                    l.to_string()
                }
            }

            let _guard = BufferActionGuard(buffer);
            buffer.begin_user_action();

            if let Some((mut start, mut end)) = buffer.selection_bounds() {
                let start_line = start.line();
                let end_line = end.line();

                start.set_line_offset(0);
                if end.line_offset() > 0 {
                    end.forward_to_line_end();
                }

                let insert_offset = start.offset();
                let text = buffer.text(&start, &end, false);

                let all_commented = text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .all(is_commented);

                let new_lines: Vec<String> = text
                    .lines()
                    .map(|l| {
                        if l.trim().is_empty() {
                            l.to_string()
                        } else {
                            toggle_comment(l, !all_commented)
                        }
                    })
                    .collect();

                let mut new_text = new_lines.join("\n");
                if text.ends_with('\n') {
                    new_text.push('\n');
                }

                buffer.delete(&mut start, &mut end);
                let mut insert_iter = buffer.iter_at_offset(insert_offset);
                buffer.insert(&mut insert_iter, &new_text);

                // Restore selection
                if let (Some(new_start), Some(new_end)) = (
                    buffer.iter_at_line(start_line),
                    buffer.iter_at_line(end_line),
                ) {
                    let mut new_end = new_end;
                    new_end.forward_to_line_end();
                    buffer.select_range(&new_start, &new_end);
                }
            } else if let Some(mark) = buffer.mark("insert") {
                let iter = buffer.iter_at_mark(&mark);
                let line = iter.line();

                if let Some(mut line_start) = buffer.iter_at_line(line) {
                    let mut line_end = line_start;
                    line_end.forward_to_line_end();
                    let line_text = buffer.text(&line_start, &line_end, false);

                    let indent = line_text.len() - line_text.trim_start().len();
                    let after_indent = &line_text[indent..];

                    let (new_text, caret_delta) = if let Some(stripped) =
                        after_indent.strip_prefix(COMMENT_PREFIX)
                    {
                        (
                            format!("{}{stripped}", &line_text[..indent]),
                            -(COMMENT_PREFIX.len() as i32),
                        )
                    } else if let Some(stripped) = after_indent.strip_prefix(COMMENT_PREFIX_BARE) {
                        (
                            format!("{}{stripped}", &line_text[..indent]),
                            -(COMMENT_PREFIX_BARE.len() as i32),
                        )
                    } else {
                        (
                            format!("{}{COMMENT_PREFIX}{after_indent}", &line_text[..indent]),
                            COMMENT_PREFIX.len() as i32,
                        )
                    };

                    let caret_col = iter.line_offset();
                    buffer.delete(&mut line_start, &mut line_end);
                    if let Some(mut insert_iter) = buffer.iter_at_line(line) {
                        buffer.insert(&mut insert_iter, &new_text);
                    }

                    let new_col = (caret_col + caret_delta).max(0);
                    if let Some(new_iter) = buffer.iter_at_line_offset(line, new_col) {
                        buffer.place_cursor(&new_iter);
                    }
                }
            }
        }

        /// Navigate to the next or previous specified text tag in the buffer,
        /// wrapping around at the end/start if no further tags are found.
        /// The matched tag region is selected so the user can see exactly what is flagged.
        fn goto_text_tag(view: &super::TypesetterEditorView, tag: &gtk::TextTag, forward: bool) {
            let buffer = view.buffer();
            let fraction = view.imp().cached_scroll_fraction();

            // Always use the start of any current selection as the reference point,
            // so navigation is consistent regardless of which end insert is on.
            let cursor = {
                let insert = buffer.iter_at_mark(&buffer.get_insert());
                let bound = buffer.iter_at_mark(&buffer.selection_bound());
                if insert.offset() < bound.offset() {
                    insert
                } else {
                    bound
                }
            };

            /// Select the full tag region starting at `start` and scroll to it.
            /// Places insert at the start of the region so subsequent navigation
            /// calls have a consistent reference point.
            fn select_and_scroll(
                view: &super::TypesetterEditorView,
                buffer: &gtk::TextBuffer,
                start: &gtk::TextIter,
                tag: &gtk::TextTag,
                fraction: f64,
            ) {
                let mut end = *start;
                end.forward_to_tag_toggle(Some(tag));
                // select_range(a, b) places insert at a and selection_bound at b.
                // We want insert at start so the next navigation call starts correctly.
                buffer.select_range(start, &end);
                view.scroll_to_mark(&buffer.get_insert(), 0.0, true, 0.0, fraction);
            }

            if forward {
                let mut iter = cursor;
                // If already inside the tag, move past it first
                if iter.has_tag(tag) {
                    iter.forward_to_tag_toggle(Some(tag));
                }
                // Find the next tag start
                if !iter.has_tag(tag) {
                    iter.forward_to_tag_toggle(Some(tag));
                }
                // If we found one before the end, go there
                if iter.has_tag(tag) && iter != buffer.end_iter() {
                    select_and_scroll(view, &buffer, &iter, tag, fraction);
                    return;
                }
                // Wrap around from the beginning
                let mut iter = buffer.start_iter();
                if !iter.has_tag(tag) {
                    iter.forward_to_tag_toggle(Some(tag));
                }
                if iter.has_tag(tag) && iter.offset() <= cursor.offset() {
                    select_and_scroll(view, &buffer, &iter, tag, fraction);
                }
            } else {
                let mut iter = cursor;
                // If inside a tag and not at its very start, move back to the start.
                // If already at the start, skip this — otherwise we'd jump back two.
                if iter.has_tag(tag) && !iter.starts_tag(Some(tag)) {
                    iter.backward_to_tag_toggle(Some(tag));
                }
                // Step backward to the next toggle point
                if iter.backward_to_tag_toggle(Some(tag)) {
                    if !iter.has_tag(tag) {
                        // Landed at a tag-end boundary — step back to find the start
                        iter.backward_to_tag_toggle(Some(tag));
                    }
                    select_and_scroll(view, &buffer, &iter, tag, fraction);
                    return;
                }
                // Wrap around from the end
                let mut iter = buffer.end_iter();
                if iter.backward_to_tag_toggle(Some(tag)) {
                    // iter is now at the end of the last tag region — find its start
                    let mut start = iter;
                    if start.backward_to_tag_toggle(Some(tag)) && start.has_tag(tag) {
                        // Successfully found the start of the last region
                        select_and_scroll(view, &buffer, &start, tag, fraction);
                    } else {
                        // Only one region and backward_to_tag_toggle couldn't step back —
                        // iter itself is the start of the region
                        select_and_scroll(view, &buffer, &iter, tag, fraction);
                    }
                }
            }
        }

        /// Select the appropriate font size based on the editor width.
        fn compute_font_size(width: i32) -> f32 {
            match width {
                0..=719 => 11.5,
                720..=959 => 12.6,
                960..=1199 => 13.7,
                1200..=1439 => 14.8,
                1440..=1659 => 15.9,
                _ => 17.0,
            }
        }

        /// Calculate the maximum line width based on the font size, DPI, and
        /// desired line length in characters.
        fn compute_line_width(font_size_pt: f32, dpi: f64, chars_per_line: f64) -> i32 {
            let px =
                (font_size_pt as f64 * dpi * chars_per_line * AVG_CHAR_WIDTH) / POINTS_PER_INCH;
            px.round() as i32
        }

        /// Read the effective DPI from the display's GtkSettings.
        /// Falls back to the nominal `DPI` constant if the value is unset.
        fn effective_dpi(obj: &super::TypesetterEditorView) -> f64 {
            let settings = gtk::Settings::for_display(&obj.display());
            let xft_dpi = settings.property::<i32>("gtk-xft-dpi");
            if xft_dpi > 0 {
                xft_dpi as f64 / 1024.0
            } else {
                DPI
            }
        }

        /// Helper function for toggling formatting characters at the start of lines.
        fn toggle_paragraph_formatting(buffer: &sourceview::Buffer, syntax: &str) {
            const KNOWN_PREFIXES: &[&str] = &[
                "- ", "+ ", "= ", "== ", "=== ", "==== ", "===== ", "====== ",
            ];

            /// Map old caret offset to new offset given old/new line text.
            fn remap_offset(
                old_line_text: &str,
                new_line_text: &str,
                old_offset: i32,
                known_prefixes: &[&str],
            ) -> i32 {
                let old_prefix_len = known_prefixes
                    .iter()
                    .find(|p| old_line_text.starts_with(**p))
                    .map(|p| p.len() as i32)
                    .unwrap_or(0);

                let new_prefix_len = known_prefixes
                    .iter()
                    .find(|p| new_line_text.starts_with(**p))
                    .map(|p| p.len() as i32)
                    .unwrap_or(0);

                if old_offset <= old_prefix_len {
                    new_prefix_len
                } else {
                    let rel = old_offset - old_prefix_len;
                    new_prefix_len + rel
                }
            }

            /// Strip any known prefix from a line.
            fn strip_known_prefix<'line, 'prefix>(
                line: &'line str,
                known: &'prefix [&'prefix str],
            ) -> (&'line str, Option<&'prefix str>) {
                for p in known {
                    if line.starts_with(*p) {
                        return (&line[p.len()..], Some(*p));
                    }
                }
                (line, None)
            }

            /// Toggle a prefix for a single line.
            fn toggle_line_prefix(
                line: &str,
                prefix: &str,
                known_prefixes: &[&str],
                remove: bool,
            ) -> String {
                let (stripped, _) = strip_known_prefix(line, known_prefixes);
                if line.trim().is_empty() {
                    line.to_string()
                } else if remove && line.starts_with(prefix) {
                    stripped.to_string()
                } else {
                    format!("{prefix}{stripped}")
                }
            }

            let _guard = BufferActionGuard(buffer);
            buffer.begin_user_action();

            let prefix = format!("{syntax} ");

            if let Some((mut start, mut end)) = buffer.selection_bounds() {
                // Save numeric positions before mutation.
                let start_line = start.line();
                let start_col = start.line_offset();
                let end_line = end.line();
                let end_col = end.line_offset();

                // Normalize selection to full lines.
                start.set_line_offset(0);
                if end.line_offset() > 0 {
                    end.forward_to_line_end();
                }

                let insert_offset = start.offset();
                let text = buffer.text(&start, &end, false);

                let all_prefixed = text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .all(|l| l.starts_with(&prefix));

                let new_lines: Vec<String> = text
                    .lines()
                    .map(|l| toggle_line_prefix(l, &prefix, KNOWN_PREFIXES, all_prefixed))
                    .collect();

                let mut new_text = new_lines.join("\n");
                if text.ends_with('\n') {
                    new_text.push('\n');
                }

                buffer.delete(&mut start, &mut end);
                let mut insert_iter = buffer.iter_at_offset(insert_offset);
                buffer.insert(&mut insert_iter, &new_text);

                // Restore selection with remapped offsets.
                let new_start_col = text
                    .lines()
                    .next()
                    .zip(new_lines.first())
                    .map(|(old, new)| remap_offset(old, new, start_col, KNOWN_PREFIXES))
                    .unwrap_or(start_col);

                let new_end_col = text
                    .lines()
                    .nth((end_line - start_line) as usize)
                    .zip(new_lines.get((end_line - start_line) as usize))
                    .map(|(old, new)| remap_offset(old, new, end_col, KNOWN_PREFIXES))
                    .unwrap_or(end_col);

                if let (Some(new_start), Some(new_end)) = (
                    buffer.iter_at_line_offset(start_line, new_start_col),
                    buffer.iter_at_line_offset(end_line, new_end_col),
                ) {
                    buffer.select_range(&new_start, &new_end);
                }
            } else if let Some(mark) = buffer.mark("insert") {
                // Caret-only case.
                let iter = buffer.iter_at_mark(&mark);
                let line = iter.line();
                let caret_col = iter.line_offset();

                if let Some(mut line_start) = buffer.iter_at_line(line) {
                    let mut line_end = line_start;
                    line_end.forward_to_line_end();
                    let line_text = buffer.text(&line_start, &line_end, false);

                    if line_text.starts_with(&prefix) {
                        // Remove our prefix.
                        let content = &line_text[prefix.len()..];
                        let caret_in_content =
                            (caret_col as isize - prefix.len() as isize).max(0) as i32;

                        buffer.delete(&mut line_start, &mut line_end);
                        if let Some(mut insert_iter) = buffer.iter_at_line(line) {
                            buffer.insert(&mut insert_iter, content);
                        }

                        if let Some(new_iter) = buffer.iter_at_line_offset(line, caret_in_content) {
                            buffer.place_cursor(&new_iter);
                        }
                    } else {
                        // Add new prefix.
                        let (stripped, _) = strip_known_prefix(&line_text, KNOWN_PREFIXES);
                        let existing_prefix_len = line_text.len() - stripped.len();
                        let caret_in_content =
                            (caret_col as isize - existing_prefix_len as isize).max(0) as i32;

                        buffer.delete(&mut line_start, &mut line_end);
                        if let Some(mut insert_iter) = buffer.iter_at_line(line) {
                            buffer.insert(&mut insert_iter, &format!("{prefix}{stripped}"));
                        }

                        let new_line_offset = prefix.len() as i32 + caret_in_content;
                        if let Some(new_iter) = buffer.iter_at_line_offset(line, new_line_offset) {
                            buffer.place_cursor(&new_iter);
                        }
                    }
                }
            }
        }

        /// Toggle inline text formatting (like bold/italics) for selection or caret
        fn toggle_text_formatting(buffer: &sourceview::Buffer, syntax: &str) {
            /// Check if a line is wrapped with the syntax
            fn is_wrapped(line: &str, syntax: &str) -> bool {
                let len = syntax.len();
                line.len() >= 2 * len && line.starts_with(syntax) && line.ends_with(syntax)
            }

            /// Remove wrapping syntax from a line
            fn unwrap_line(line: &str, syntax: &str) -> String {
                let len = syntax.len();
                line[len..line.len() - len].to_string()
            }

            let _guard = BufferActionGuard(buffer);
            buffer.begin_user_action();

            let syntax_len = syntax.len() as i32;

            if let Some((mut start, mut end)) = buffer.selection_bounds() {
                // Expand selection if inside existing markers
                let mut expanded = false;
                {
                    let mut left = start;
                    let mut right = end;

                    if left.backward_chars(syntax_len)
                        && right.forward_chars(syntax_len)
                        && buffer.text(&left, &start, false) == syntax
                        && buffer.text(&end, &right, false) == syntax
                    {
                        start = left;
                        end = right;
                        expanded = true;
                    }
                }

                // Transform each line
                let text = buffer.text(&start, &end, false);
                let new_lines: Vec<String> = text
                    .lines()
                    .map(|l| {
                        if l.trim().is_empty() {
                            l.to_string()
                        } else if expanded || is_wrapped(l, syntax) {
                            unwrap_line(l, syntax)
                        } else {
                            format!("{syntax}{l}{syntax}")
                        }
                    })
                    .collect();

                // Reassemble text
                let mut new_text = new_lines.join("\n");
                if text.ends_with('\n') {
                    new_text.push('\n');
                }

                buffer.delete(&mut start, &mut end);
                buffer.insert(&mut start, &new_text);

                // Reselect the newly inserted text
                start.backward_chars(new_text.len() as i32);
                let end_iter = buffer.iter_at_offset(start.offset() + new_text.len() as i32);
                buffer.select_range(&start, &end_iter);
            } else if let Some(mark) = buffer.mark("insert") {
                // Caret-only toggle
                let offset = buffer.iter_at_mark(&mark).offset();
                let char_count = buffer.char_count();

                let can_check_before = offset >= syntax_len;
                let can_check_after = (offset + syntax_len) <= char_count;

                if can_check_before && can_check_after {
                    let before_start = buffer.iter_at_offset(offset - syntax_len);
                    let before_end = buffer.iter_at_offset(offset);
                    let after_start = buffer.iter_at_offset(offset);
                    let after_end = buffer.iter_at_offset(offset + syntax_len);

                    if buffer.text(&before_start, &before_end, false) == syntax
                        && buffer.text(&after_start, &after_end, false) == syntax
                    {
                        // Remove existing markers
                        buffer.delete(
                            &mut buffer.iter_at_offset(offset - syntax_len),
                            &mut buffer.iter_at_offset(offset),
                        );
                        buffer.delete(
                            &mut buffer.iter_at_offset(offset - syntax_len),
                            &mut buffer.iter_at_offset(offset),
                        );
                        buffer.place_cursor(&buffer.iter_at_offset(offset - syntax_len));
                        return;
                    }
                }

                // Insert new marker pair and place caret between
                let mut iter = buffer.iter_at_mark(&mark);
                buffer.insert(&mut iter, &format!("{syntax}{syntax}"));
                iter.backward_chars(syntax_len);
                buffer.place_cursor(&iter);
            }
        }
    }
}

glib::wrapper! {
    pub struct TypesetterEditorView(ObjectSubclass<imp::TypesetterEditorView>)
        @extends sourceview::View, gtk::TextView, gtk::Widget, gtk::Scrollable,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

#[allow(clippy::new_without_default)]
impl TypesetterEditorView {
    pub fn new() -> Self {
        glib::Object::new::<Self>()
    }

    pub fn source_buffer(&self) -> sourceview::Buffer {
        self.buffer()
            .downcast::<sourceview::Buffer>()
            .expect("Expected GtkSourceBuffer")
    }

    pub fn scroll_fraction(&self) -> f64 {
        self.imp().cached_scroll_fraction()
    }

    // fn setup_vim_mode(&self, obj: &super::TypesetterEditorView) {
    //     let im_context = sourceview::VimIMContext::new();
    //     let key_controller = gtk::EventControllerKey::new();

    //     key_controller.set_im_context(Some(&im_context));
    //     key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    //     obj.add_controller(key_controller.clone());

    //     use gtk::prelude::IMContextExt;
    //     im_context.set_client_widget(Some(obj.upcast_ref::<gtk::Widget>()));

    // TODO: wire command_bar_text and command_text to a floating popover

    //     self.vim_im_context.replace(Some(im_context));
    // }

    // fn cleanup_vim_mode(&self) {
    //     self.vim_im_context.borrow_mut().take();
    // }
}
