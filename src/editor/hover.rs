// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::*;
use gtk::{glib, glib::subclass::prelude::*};
use sourceview::{prelude::*, subclass::prelude::*};
use std::{cell::RefCell, future::Future, pin::Pin, rc::Rc};

use crate::editor::{LintSuggestion, harper::HarperLint};
use crate::typst_system::TooltipContent;
use crate::window::TypesetterWindow;

mod provider_imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct TypesetterTypstHoverProvider {}

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterTypstHoverProvider {
        const NAME: &'static str = "TypesetterTypstHoverProvider";
        type Type = super::TypesetterTypstHoverProvider;
        type ParentType = glib::Object;
        type Interfaces = (sourceview::HoverProvider,);
    }

    impl ObjectImpl for TypesetterTypstHoverProvider {}

    impl HoverProviderImpl for TypesetterTypstHoverProvider {
        fn populate_future(
            &self,
            context: &sourceview::HoverContext,
            display: &sourceview::HoverDisplay,
        ) -> Pin<Box<dyn Future<Output = Result<(), glib::Error>>>> {
            Box::pin(Self::generate_hover_content(
                context.clone(),
                display.clone(),
            ))
        }
    }

    impl TypesetterTypstHoverProvider {
        fn find_typesetter_window(widget: &gtk::Widget) -> Option<TypesetterWindow> {
            let mut current = widget.clone();
            loop {
                if let Ok(window) = current.clone().downcast::<TypesetterWindow>() {
                    return Some(window);
                }
                current = current.parent()?;
            }
        }

        fn find_editor(widget: &gtk::Widget) -> Option<crate::editor::TypesetterEditor> {
            let mut current = widget.clone();
            loop {
                if let Ok(editor) = current
                    .clone()
                    .downcast::<crate::editor::TypesetterEditor>()
                {
                    return Some(editor);
                }
                current = current.parent()?;
            }
        }

        async fn generate_hover_content(
            context: sourceview::HoverContext,
            display: sourceview::HoverDisplay,
        ) -> Result<(), glib::Error> {
            let view = context.view();

            let window = Self::find_typesetter_window(view.upcast_ref::<gtk::Widget>())
                .ok_or_else(|| {
                    glib::Error::new(
                        gtk::gio::IOErrorEnum::Failed,
                        "Could not find TypesetterWindow",
                    )
                })?;

            let editor = Self::find_editor(view.upcast_ref::<gtk::Widget>()).ok_or_else(|| {
                glib::Error::new(
                    gtk::gio::IOErrorEnum::Failed,
                    "Could not find TypesetterEditor",
                )
            })?;

            // Get the iterator at the hover location
            let Some(iter) = context.iter() else {
                return Err(glib::Error::new(
                    gtk::gio::IOErrorEnum::Failed,
                    "No iterator in context",
                ));
            };
            let cursor_offset = editor.iter_to_byte_offset(&iter);

            // Get hover information from Typst
            let typst_system = window.compiler_system().await.map_err(|e| {
                glib::Error::new(
                    gtk::gio::IOErrorEnum::Failed,
                    &format!("Failed to get compiler: {:?}", e),
                )
            })?;

            let hover_result = typst_system
                .request_hover(cursor_offset)
                .await
                .map_err(|e| {
                    glib::Error::new(
                        gtk::gio::IOErrorEnum::Failed,
                        &format!("Hover request failed: {:?}", e),
                    )
                })?;

            if let Some(result) = hover_result {
                let widget = Self::create_tooltip_widget(&result.content);
                display.append(&widget);
                Ok(())
            } else {
                Err(glib::Error::new(
                    gtk::gio::IOErrorEnum::Failed,
                    "No hover information available",
                ))
            }
        }

        fn clean_tooltip_text(text: &str) -> String {
            // Remove line breaks and normalize whitespace
            text.lines()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        }

        fn create_tooltip_widget(content: &TooltipContent) -> gtk::Widget {
            match content {
                TooltipContent::Text(text) => {
                    let cleaned_text = Self::clean_tooltip_text(text);
                    let label = gtk::Label::new(Some(&cleaned_text));
                    label.set_wrap(true);
                    label.set_max_width_chars(60);
                    label.set_margin_top(8);
                    label.set_margin_bottom(8);
                    label.set_margin_start(12);
                    label.set_margin_end(12);
                    label.upcast()
                }
                TooltipContent::Code(code) => {
                    // Create a syntax-highlighted code view
                    let buffer = sourceview::Buffer::new(None);
                    buffer.set_text(code);

                    // Try to set Typst language for syntax highlighting
                    let lang_manager = sourceview::LanguageManager::new();
                    let mut search_paths = lang_manager.search_path();
                    search_paths.push("resource:///dev/whetstone/editor/languages/".into());
                    lang_manager.set_search_path(
                        &search_paths.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    );

                    if let Some(language) = lang_manager.language("typst") {
                        buffer.set_language(Some(&language));
                    }

                    let view = sourceview::View::with_buffer(&buffer);
                    view.set_editable(false);
                    view.set_cursor_visible(false);
                    view.set_monospace(true);
                    view.set_show_line_numbers(false);
                    view.set_margin_top(4);
                    view.set_margin_bottom(4);
                    view.set_margin_start(8);
                    view.set_margin_end(8);

                    // Wrap in a frame for better appearance
                    let frame = gtk::Frame::new(None);
                    frame.set_child(Some(&view));
                    frame.upcast()
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct TypesetterTypstHoverProvider(ObjectSubclass<provider_imp::TypesetterTypstHoverProvider>)
        @implements sourceview::HoverProvider;
}

impl TypesetterTypstHoverProvider {
    pub fn new() -> Self {
        glib::Object::new()
    }
}

impl Default for TypesetterTypstHoverProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Setup hover tooltip functionality on the editor view.
///
/// Registers three providers on the view's hover controller:
/// - `TypesetterTypstHoverProvider` for Typst symbol tooltips
/// - `TypesetterHarperHoverProvider` for Harper grammar lint messages
/// - `TypesetterDiagnosticHoverProvider` for Typst diagnostic messages
///
/// All providers are returned so the caller can store them for cleanup.
pub fn setup_hover(
    view: &crate::editor::view::TypesetterEditorView,
    harper_lints: Rc<RefCell<Vec<HarperLint>>>,
    diagnostic_lints: Rc<RefCell<Vec<DiagnosticLint>>>,
) -> (
    TypesetterTypstHoverProvider,
    TypesetterHarperHoverProvider,
    TypesetterDiagnosticHoverProvider,
) {
    let hover = view.hover();

    let typst_provider = TypesetterTypstHoverProvider::new();
    hover.add_provider(&typst_provider.clone().upcast::<sourceview::HoverProvider>());

    let harper_provider = TypesetterHarperHoverProvider::new(harper_lints);
    hover.add_provider(
        &harper_provider
            .clone()
            .upcast::<sourceview::HoverProvider>(),
    );

    let diagnostic_provider = TypesetterDiagnosticHoverProvider::new(diagnostic_lints);
    hover.add_provider(
        &diagnostic_provider
            .clone()
            .upcast::<sourceview::HoverProvider>(),
    );

    (typst_provider, harper_provider, diagnostic_provider)
}

// ── Harper hover provider ─────────────────────────────────────────────────

mod harper_provider_imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct TypesetterHarperHoverProvider {
        pub(crate) lints: RefCell<Option<Rc<RefCell<Vec<HarperLint>>>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterHarperHoverProvider {
        const NAME: &'static str = "TypesetterHarperHoverProvider";
        type Type = super::TypesetterHarperHoverProvider;
        type ParentType = glib::Object;
        type Interfaces = (sourceview::HoverProvider,);
    }

    impl ObjectImpl for TypesetterHarperHoverProvider {}

    impl HoverProviderImpl for TypesetterHarperHoverProvider {
        fn populate_future(
            &self,
            context: &sourceview::HoverContext,
            display: &sourceview::HoverDisplay,
        ) -> Pin<Box<dyn Future<Output = Result<(), glib::Error>>>> {
            // Collect what we need before entering the async block —
            // neither context nor display are Send, but we don't need
            // to move across threads, just into an async block on the
            // same thread.
            let context = context.clone();
            let display = display.clone();
            let lints = self.lints.borrow().clone();

            Box::pin(async move { Self::generate_hover_content(context, display, lints).await })
        }
    }

    impl TypesetterHarperHoverProvider {
        async fn generate_hover_content(
            context: sourceview::HoverContext,
            display: sourceview::HoverDisplay,
            lints: Option<Rc<RefCell<Vec<HarperLint>>>>,
        ) -> Result<(), glib::Error> {
            let some_lints = lints.ok_or_else(|| {
                glib::Error::new(gtk::gio::IOErrorEnum::Failed, "No lint list available")
            })?;

            // Get the iter at the hover position.
            let iter = context.iter().ok_or_else(|| {
                glib::Error::new(gtk::gio::IOErrorEnum::Failed, "No iterator in context")
            })?;

            // Derive the buffer live from the context so it is always current,
            // even if the view's buffer were ever replaced after construction.
            let buffer = context
                .view()
                .buffer()
                .downcast::<sourceview::Buffer>()
                .map_err(|_| {
                    glib::Error::new(
                        gtk::gio::IOErrorEnum::Failed,
                        "Could not downcast buffer to sourceview::Buffer",
                    )
                })?;

            let hover_offset = iter.offset();

            // Find the first lint whose span straddles the hover position,
            // cloning the marks so button callbacks can resolve their live
            // positions at click time rather than at tooltip-build time.
            let found = some_lints.borrow().iter().find_map(|lint| {
                let start = lint.current_start(&buffer)?;
                let end = lint.current_end(&buffer)?;
                if hover_offset >= start && hover_offset < end {
                    let start_iter = buffer.iter_at_offset(start);
                    let end_iter = buffer.iter_at_offset(end);
                    let span_text = buffer.text(&start_iter, &end_iter, false).to_string();
                    Some((
                        lint.message.clone(),
                        lint.suggestions.clone(),
                        span_text,
                        lint.start_mark.clone(),
                        lint.end_mark.clone(),
                    ))
                } else {
                    None
                }
            });

            match found {
                Some((message, suggestions, span_text, start_mark, end_mark)) => {
                    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
                    vbox.set_margin_top(8);
                    vbox.set_margin_bottom(8);
                    vbox.set_margin_start(12);
                    vbox.set_margin_end(12);

                    // TODO: GTK 4.10 deprecated lookup_color. Find a non-deprecated way to resolve accent_color.
                    #[allow(deprecated)]
                    let accent_hex = context
                        .view()
                        .style_context()
                        .lookup_color("accent_color")
                        .map(|c| {
                            format!(
                                "#{:02x}{:02x}{:02x}",
                                (c.red() * 255.0) as u8,
                                (c.green() * 255.0) as u8,
                                (c.blue() * 255.0) as u8,
                            )
                        });

                    // Escape the message for Pango markup, converting `backtick
                    // spans` to underlined runs in the accent colour.
                    let markup = glib::markup_escape_text(&message);
                    let markup = markup.replace('`', "\x00");
                    let markup = {
                        let mut out = String::with_capacity(markup.len());
                        let mut in_code = false;
                        for part in markup.split('\x00') {
                            if in_code {
                                match &accent_hex {
                                    Some(color) => {
                                        out.push_str(&format!(
                                            "<span underline=\"single\" underline_color=\"{color}\">"
                                        ));
                                        out.push_str(part);
                                        out.push_str("</span>");
                                    }
                                    None => {
                                        out.push_str("<u>");
                                        out.push_str(part);
                                        out.push_str("</u>");
                                    }
                                }
                            } else {
                                out.push_str(part);
                            }
                            in_code = !in_code;
                        }
                        out
                    };
                    let label = gtk::Label::new(None);
                    label.set_markup(&markup);
                    label.set_wrap(true);
                    label.set_max_width_chars(60);
                    label.set_xalign(0.0);
                    vbox.append(&label);

                    let buffer_weak = buffer.downgrade();

                    if !suggestions.is_empty() {
                        let btn_box = adw::WrapBox::new();
                        btn_box.set_child_spacing(4);
                        btn_box.set_line_spacing(2);
                        btn_box.set_margin_top(4);

                        for suggestion in &suggestions {
                            let label_text =
                                format!("→ {}", suggestion.label_with_context(&span_text));
                            let button = gtk::Button::with_label(&label_text);
                            button.set_halign(gtk::Align::Start);
                            button.add_css_class("suggested-action");
                            button.add_css_class("harper-button");

                            let buffer_weak = buffer_weak.clone();
                            let lints_weak = Rc::downgrade(&some_lints);
                            let start_mark = start_mark.clone();
                            let end_mark = end_mark.clone();
                            let suggestion = suggestion.clone();

                            button.connect_clicked(move |_| {
                                let Some(buffer) = buffer_weak.upgrade() else {
                                    return;
                                };
                                let Some(lints) = lints_weak.upgrade() else {
                                    return;
                                };
                                let cursor_offset = match &suggestion {
                                    LintSuggestion::ReplaceWith(text) => {
                                        let start = buffer.iter_at_mark(&start_mark).offset();
                                        start + text.chars().count() as i32
                                    }
                                    LintSuggestion::InsertAfter(text) => {
                                        let end = buffer.iter_at_mark(&end_mark).offset();
                                        end + text.chars().count() as i32
                                    }
                                    LintSuggestion::Remove => {
                                        buffer.iter_at_mark(&start_mark).offset()
                                    }
                                };
                                crate::editor::harper::apply_suggestion(
                                    &buffer,
                                    &mut lints.borrow_mut(),
                                    &start_mark,
                                    &end_mark,
                                    &suggestion,
                                );
                                let cursor_pos = buffer.iter_at_offset(cursor_offset);
                                buffer.place_cursor(&cursor_pos);
                            });

                            btn_box.append(&button);
                        }

                        vbox.append(&btn_box);
                    }

                    display.append(&vbox);
                    Ok(())
                }
                None => {
                    // No lint at this position — return Err so GtkSourceView
                    // suppresses the empty hover popup.
                    Err(glib::Error::new(
                        gtk::gio::IOErrorEnum::Failed,
                        "No Harper lint at hover position",
                    ))
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct TypesetterHarperHoverProvider(
        ObjectSubclass<harper_provider_imp::TypesetterHarperHoverProvider>
    )
        @implements sourceview::HoverProvider;
}

impl TypesetterHarperHoverProvider {
    pub(crate) fn new(lints: Rc<RefCell<Vec<HarperLint>>>) -> Self {
        let obj: Self = glib::Object::new();
        *obj.imp().lints.borrow_mut() = Some(lints);
        obj
    }
}

// ── Diagnostic hover provider ─────────────────────────────────────────────

/// A single compiler diagnostic anchored into the buffer via mark pairs,
/// mirroring the HarperLint pattern.
#[derive(Debug)]
pub struct DiagnosticLint {
    pub start_mark: gtk::TextMark,
    pub end_mark: gtk::TextMark,
    pub message: String,
    pub hints: Vec<String>,
}

impl DiagnosticLint {
    pub fn new(
        buffer: &sourceview::Buffer,
        start: i32,
        end: i32,
        message: impl Into<String>,
        hints: Vec<String>,
    ) -> Self {
        let start_iter = buffer.iter_at_offset(start);
        let end_iter = buffer.iter_at_offset(end);
        let start_mark = buffer.create_mark(None, &start_iter, true);
        let end_mark = buffer.create_mark(None, &end_iter, false);
        Self {
            start_mark,
            end_mark,
            message: message.into(),
            hints,
        }
    }

    pub fn current_start(&self, buffer: &sourceview::Buffer) -> Option<i32> {
        if self.start_mark.is_deleted() {
            return None;
        }
        Some(buffer.iter_at_mark(&self.start_mark).offset())
    }

    pub fn current_end(&self, buffer: &sourceview::Buffer) -> Option<i32> {
        if self.end_mark.is_deleted() {
            return None;
        }
        Some(buffer.iter_at_mark(&self.end_mark).offset())
    }

    pub fn delete_marks(&self, buffer: &sourceview::Buffer) {
        if !self.start_mark.is_deleted() {
            buffer.delete_mark(&self.start_mark);
        }
        if !self.end_mark.is_deleted() {
            buffer.delete_mark(&self.end_mark);
        }
    }
}

mod diagnostic_provider_imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct TypesetterDiagnosticHoverProvider {
        pub(crate) lints: RefCell<Option<Rc<RefCell<Vec<DiagnosticLint>>>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterDiagnosticHoverProvider {
        const NAME: &'static str = "TypesetterDiagnosticHoverProvider";
        type Type = super::TypesetterDiagnosticHoverProvider;
        type ParentType = glib::Object;
        type Interfaces = (sourceview::HoverProvider,);
    }

    impl ObjectImpl for TypesetterDiagnosticHoverProvider {}

    impl HoverProviderImpl for TypesetterDiagnosticHoverProvider {
        fn populate_future(
            &self,
            context: &sourceview::HoverContext,
            display: &sourceview::HoverDisplay,
        ) -> Pin<Box<dyn Future<Output = Result<(), glib::Error>>>> {
            let context = context.clone();
            let display = display.clone();
            let lints = self.lints.borrow().clone();
            Box::pin(async move { Self::generate_hover_content(context, display, lints).await })
        }
    }

    impl TypesetterDiagnosticHoverProvider {
        async fn generate_hover_content(
            context: sourceview::HoverContext,
            display: sourceview::HoverDisplay,
            lints: Option<Rc<RefCell<Vec<DiagnosticLint>>>>,
        ) -> Result<(), glib::Error> {
            let some_lints = lints.ok_or_else(|| {
                glib::Error::new(gtk::gio::IOErrorEnum::Failed, "No diagnostic lint list")
            })?;

            let iter = context.iter().ok_or_else(|| {
                glib::Error::new(gtk::gio::IOErrorEnum::Failed, "No iterator in context")
            })?;

            let buffer = context
                .view()
                .buffer()
                .downcast::<sourceview::Buffer>()
                .map_err(|_| {
                    glib::Error::new(gtk::gio::IOErrorEnum::Failed, "Could not downcast buffer")
                })?;

            let hover_offset = iter.offset();

            let found = some_lints.borrow().iter().find_map(|lint| {
                let start = lint.current_start(&buffer)?;
                let end = lint.current_end(&buffer)?;
                if hover_offset >= start && hover_offset < end {
                    Some((lint.message.clone(), lint.hints.clone()))
                } else {
                    None
                }
            });

            match found {
                Some((message, hints)) => {
                    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
                    vbox.set_margin_top(8);
                    vbox.set_margin_bottom(8);
                    vbox.set_margin_start(12);
                    vbox.set_margin_end(12);

                    // Message
                    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    let msg_label =
                        gtk::Label::new(Some(&crate::utils::capitalize_first_letter(&message)));
                    msg_label.set_wrap(true);
                    msg_label.set_max_width_chars(60);
                    msg_label.set_xalign(0.0);
                    header.append(&msg_label);
                    vbox.append(&header);

                    // Hints below, dim-label style
                    if !hints.is_empty() {
                        let hint_text = hints
                            .iter()
                            .map(|h| format!("→ {}", h))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let hint_label = gtk::Label::new(Some(&hint_text));
                        hint_label.set_xalign(0.0);
                        hint_label.set_wrap(true);
                        hint_label.set_max_width_chars(60);
                        hint_label.add_css_class("dim-label");
                        vbox.append(&hint_label);
                    }

                    display.append(&vbox);
                    Ok(())
                }
                None => Err(glib::Error::new(
                    gtk::gio::IOErrorEnum::Failed,
                    "No diagnostic at hover position",
                )),
            }
        }
    }
}

glib::wrapper! {
    pub struct TypesetterDiagnosticHoverProvider(
        ObjectSubclass<diagnostic_provider_imp::TypesetterDiagnosticHoverProvider>
    )
        @implements sourceview::HoverProvider;
}

impl TypesetterDiagnosticHoverProvider {
    pub(crate) fn new(lints: Rc<RefCell<Vec<DiagnosticLint>>>) -> Self {
        let obj: Self = glib::Object::new();
        *obj.imp().lints.borrow_mut() = Some(lints);
        obj
    }
}
