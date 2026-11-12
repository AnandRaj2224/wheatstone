// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::{gio, glib, prelude::*, subclass::prelude::*};
use sourceview::{Snippet, SnippetChunk};
use sourceview::{prelude::ViewManualExt, subclass::prelude::*};
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use typst_ide::Completion;

use crate::window::TypesetterWindow;

mod provider_imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct TypesetterCompletionProvider {}

    /// Parsed chunk information (without GTK objects for testing)
    #[derive(Debug, PartialEq)]
    pub struct ChunkInfo {
        pub text: String,
        pub focus_position: Option<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterCompletionProvider {
        const NAME: &'static str = "TypesetterCompletionProvider";
        type Type = super::TypesetterCompletionProvider;
        type ParentType = glib::Object;
        type Interfaces = (sourceview::CompletionProvider,);
    }

    impl ObjectImpl for TypesetterCompletionProvider {}

    impl CompletionProviderImpl for TypesetterCompletionProvider {
        fn title(&self) -> Option<glib::GString> {
            Some("Typst".into())
        }

        fn display(
            &self,
            _context: &sourceview::CompletionContext,
            proposal: &sourceview::CompletionProposal,
            cell: &sourceview::CompletionCell,
        ) {
            let completion = proposal
                .downcast_ref::<TypesetterCompletionProposal>()
                .expect("not a TypesetterCompletionProposal");

            match cell.column() {
                sourceview::CompletionColumn::Icon => {
                    cell.set_icon_name(&completion.icon_name());
                }
                sourceview::CompletionColumn::TypedText => {
                    cell.set_text(Some(&completion.label()));
                }
                sourceview::CompletionColumn::Comment => {
                    cell.set_text(completion.comment().as_deref());
                }
                sourceview::CompletionColumn::Details => {
                    cell.set_text(None);
                }
                _ => {}
            }
        }

        fn populate_future(
            &self,
            context: &sourceview::CompletionContext,
        ) -> Pin<Box<dyn Future<Output = Result<gio::ListModel, glib::Error>>>> {
            Box::pin(Self::generate_completion_results(context.clone()))
        }

        fn refilter(&self, context: &sourceview::CompletionContext, model: &gio::ListModel) {
            // Get the partial word that's already been typed
            let word = Self::get_current_word(context);

            let Some(filter_list) = model.downcast_ref::<gtk::FilterListModel>() else {
                return;
            };
            let Some(filter) = filter_list.filter().and_downcast::<gtk::StringFilter>() else {
                return;
            };

            // Set the search term to filter by the partial word
            if word.is_empty() {
                filter.set_search(None);
            } else {
                filter.set_search(Some(&word));
            }

            self.parent_refilter(context, model);
        }

        fn priority(&self, _context: &sourceview::CompletionContext) -> i32 {
            0
        }

        fn activate(
            &self,
            context: &sourceview::CompletionContext,
            proposal: &sourceview::CompletionProposal,
        ) {
            let completion = proposal
                .downcast_ref::<TypesetterCompletionProposal>()
                .expect("not a TypesetterCompletionProposal");

            let apply = completion.apply().unwrap_or_else(|| completion.label());
            let position = completion.position();

            let Some(buffer) = context.buffer() else {
                return;
            };
            let Some(view) = context.view() else { return };

            let cursor_pos = buffer.cursor_position();
            let current_iter = buffer.iter_at_offset(cursor_pos);

            // Find the start of the current word by moving backward from cursor
            let mut word_start = current_iter;
            while !word_start.starts_line() {
                // Check the character before the current position
                let mut check_iter = word_start;
                if !check_iter.backward_char() {
                    break;
                }
                let prev_char = check_iter.char();
                // Stop at whitespace or non-word characters (except underscore and hyphen)
                if !prev_char.is_alphanumeric() && prev_char != '_' && prev_char != '-' {
                    break;
                }
                word_start = check_iter;
            }

            // Use the earlier of the word start we found or the stored position
            let delete_start_offset = std::cmp::min(word_start.offset(), position as i32);
            let mut delete_start = buffer.iter_at_offset(delete_start_offset);
            let mut delete_end = buffer.iter_at_offset(cursor_pos);

            buffer.begin_user_action();
            buffer.delete(&mut delete_start, &mut delete_end);

            // After deletion, get the insert position (which is where delete_start now points)
            let mut insert_iter = buffer.iter_at_offset(delete_start_offset);

            // Parse the apply string into snippet chunks
            let chunk_infos = Self::parse_apply_string(&apply);
            let has_placeholders = chunk_infos.iter().any(|c| c.focus_position.is_some());

            if has_placeholders {
                // Check if we're at the end of the buffer
                let at_end = insert_iter.is_end();

                // If at end, insert a newline first to bound the snippet
                if at_end {
                    buffer.insert(&mut insert_iter, "\n");
                    // Move back before the newline
                    insert_iter.backward_char();
                }

                // If we have placeholders, use a snippet
                let snippet = Snippet::new(None, Some("typst"));

                // Convert to SnippetChunks and track if we added any focus positions
                let mut max_focus = 0;
                for info in chunk_infos {
                    let builder = SnippetChunk::builder().spec(&info.text);
                    let chunk = if let Some(pos) = info.focus_position {
                        if pos > 0 && pos > max_focus {
                            max_focus = pos;
                        }
                        builder.focus_position(pos).build()
                    } else {
                        builder.build()
                    };
                    snippet.add_chunk(&chunk);
                }

                // Add explicit final tab stop at position 0
                if max_focus > 0 {
                    let final_chunk = SnippetChunk::builder().spec("").focus_position(0).build();
                    snippet.add_chunk(&final_chunk);
                }

                #[cfg(debug_assertions)]
                println!(
                    "Pushing snippet with {} chunks, max_focus: {}, at_end: {} for: {}",
                    snippet.n_chunks(),
                    max_focus,
                    at_end,
                    apply
                );

                view.push_snippet(&snippet, Some(&mut insert_iter));
            } else {
                // Simple text insertion - no placeholders
                buffer.insert(&mut insert_iter, &apply);
            }

            buffer.end_user_action();
        }
    }

    impl TypesetterCompletionProvider {
        fn find_typesetter_window(widget: &gtk::Widget) -> Option<TypesetterWindow> {
            let mut current = widget.clone();
            loop {
                if let Ok(window) = current.clone().downcast::<TypesetterWindow>() {
                    return Some(window);
                }
                current = current.parent()?;
            }
        }

        // Remove line breaks and normalize whitespace
        pub fn clean_comment_text(text: &str) -> String {
            text.lines()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        }

        /// Extract the current partial word from the buffer
        fn get_current_word(context: &sourceview::CompletionContext) -> String {
            let Some(buffer) = context.buffer() else {
                return String::new();
            };

            let cursor_pos = buffer.cursor_position();
            let current_iter = buffer.iter_at_offset(cursor_pos);

            // Find the start of the current word by moving backward from cursor
            let mut word_start = current_iter;
            while !word_start.starts_line() {
                let mut check_iter = word_start;
                if !check_iter.backward_char() {
                    break;
                }
                let prev_char = check_iter.char();
                // Stop at whitespace or non-word characters (except underscore and hyphen)
                if !prev_char.is_alphanumeric() && prev_char != '_' && prev_char != '-' {
                    break;
                }
                word_start = check_iter;
            }

            // Get the text from word start to cursor
            buffer.text(&word_start, &current_iter, false).to_string()
        }

        async fn generate_completion_results(
            context: sourceview::CompletionContext,
        ) -> Result<gio::ListModel, glib::Error> {
            let explicit = context.activation() == sourceview::CompletionActivation::UserRequested;

            #[cfg(debug_assertions)]
            println!(
                "Completion triggered: activation={:?}, explicit={}",
                context.activation(),
                explicit
            );

            if !explicit {
                let empty = gio::ListStore::new::<sourceview::CompletionProposal>();
                return Ok(empty.upcast());
            }

            let view = context
                .view()
                .ok_or_else(|| glib::Error::new(gio::IOErrorEnum::Failed, "No view in context"))?;

            let window = Self::find_typesetter_window(view.upcast_ref::<gtk::Widget>())
                .ok_or_else(|| {
                    glib::Error::new(gio::IOErrorEnum::Failed, "Could not find TypesetterWindow")
                })?;

            // Get the editor and use the byte offset helper
            let editor = window.imp().editor_container.get();
            let cursor_offset = editor.cursor_byte_offset();

            // Get completions from Typst
            let typst_system = window.compiler_system().await.map_err(|e| {
                glib::Error::new(
                    gio::IOErrorEnum::Failed,
                    &format!("Failed to get compiler: {:?}", e),
                )
            })?;

            let (pos, completions) = typst_system
                .request_autocomplete(cursor_offset, explicit)
                .await
                .map(|completions| (cursor_offset, completions))
                .unwrap_or((0, Vec::new()));

            let list = gio::ListStore::new::<sourceview::CompletionProposal>();
            let completions = completions
                .into_iter()
                .map(|completion| TypesetterCompletionProposal::from_completion(completion, pos))
                .collect::<Vec<_>>();
            list.splice(0, 0, &completions);

            let filter = gtk::StringFilter::builder()
                .expression(gtk::PropertyExpression::new(
                    TypesetterCompletionProposal::static_type(),
                    gtk::Expression::NONE,
                    "label",
                ))
                .match_mode(gtk::StringFilterMatchMode::Prefix)
                .ignore_case(true)
                .build();

            // Apply initial filter based on current word
            let word = Self::get_current_word(&context);
            if !word.is_empty() {
                filter.set_search(Some(&word));
            }

            let filter_list = gtk::FilterListModel::new(Some(list), Some(filter));

            Ok(filter_list.upcast())
        }

        /// Parse the apply string into chunk information
        /// This is separated from GTK object creation to make it testable
        pub fn parse_apply_string(apply: &str) -> Vec<ChunkInfo> {
            let mut chunks = Vec::new();
            let mut edit_position = 1;
            let mut remaining = apply;

            loop {
                if let Some(start) = remaining.find("${") {
                    if let Some(end) = remaining[start..].find('}') {
                        // Add text before the placeholder
                        let before = &remaining[..start];
                        if !before.is_empty() {
                            chunks.push(ChunkInfo {
                                text: before.to_string(),
                                focus_position: None,
                            });
                        }

                        // Add the placeholder content
                        if end >= 2 {
                            let edit_point = &remaining[start + 2..start + end];
                            chunks.push(ChunkInfo {
                                text: edit_point.to_string(),
                                focus_position: Some(edit_position),
                            });
                            edit_position += 1;
                        }

                        // Move past this placeholder
                        remaining = &remaining[start + end + 1..];
                    } else {
                        // No closing brace found, treat rest as literal text
                        if !remaining.is_empty() {
                            chunks.push(ChunkInfo {
                                text: remaining.to_string(),
                                focus_position: None,
                            });
                        }
                        break;
                    }
                } else {
                    // No more placeholders, add remaining text
                    if !remaining.is_empty() {
                        chunks.push(ChunkInfo {
                            text: remaining.to_string(),
                            focus_position: None,
                        });
                    }
                    break;
                }
            }

            chunks
        }

        /// Convert parsed chunk info into actual GtkSourceView SnippetChunks
        pub fn parse_apply_to_chunks(apply: &str) -> Vec<SnippetChunk> {
            Self::parse_apply_string(apply)
                .into_iter()
                .map(|info| {
                    let builder = SnippetChunk::builder().spec(&info.text);
                    if let Some(pos) = info.focus_position {
                        builder.focus_position(pos).build()
                    } else {
                        builder.build()
                    }
                })
                .collect()
        }
    }
}

glib::wrapper! {
    pub struct TypesetterCompletionProvider(ObjectSubclass<provider_imp::TypesetterCompletionProvider>)
        @implements sourceview::CompletionProvider;
}

impl TypesetterCompletionProvider {
    pub fn new() -> Self {
        glib::Object::new()
    }
}

impl Default for TypesetterCompletionProvider {
    fn default() -> Self {
        Self::new()
    }
}

mod proposal_imp {
    use super::*;

    #[derive(glib::Properties, Default)]
    #[properties(wrapper_type = super::TypesetterCompletionProposal)]
    pub struct TypesetterCompletionProposal {
        #[property(get)]
        pub(super) icon_name: RefCell<String>,
        #[property(get)]
        pub(super) label: RefCell<String>,
        #[property(get, nullable)]
        pub(super) comment: RefCell<Option<String>>,
        #[property(get, nullable)]
        pub(super) apply: RefCell<Option<String>>,
        #[property(get)]
        pub(super) position: Cell<u32>,
    }

    #[glib::derived_properties]
    impl ObjectImpl for TypesetterCompletionProposal {}

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterCompletionProposal {
        const NAME: &'static str = "TypesetterCompletionProposal";
        type Type = super::TypesetterCompletionProposal;
        type Interfaces = (sourceview::CompletionProposal,);
    }

    impl CompletionProposalImpl for TypesetterCompletionProposal {}
}

glib::wrapper! {
    pub struct TypesetterCompletionProposal(ObjectSubclass<proposal_imp::TypesetterCompletionProposal>)
        @implements sourceview::CompletionProposal;
}

impl Default for TypesetterCompletionProposal {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl TypesetterCompletionProposal {
    pub fn from_completion(value: Completion, position: usize) -> Self {
        let completion = Self::default();
        let imp = completion.imp();
        imp.label.replace(value.label.to_string());
        imp.icon_name
            .replace(Self::icon_name_from_kind(&value.kind).into());

        // Clean the comment text to remove line breaks
        imp.comment.replace(value.detail.map(|comment| {
            provider_imp::TypesetterCompletionProvider::clean_comment_text(&comment)
        }));

        imp.apply
            .replace(value.apply.map(|apply| apply.to_string()));
        imp.position.set(position as u32);
        completion
    }

    fn icon_name_from_kind(kind: &typst_ide::CompletionKind) -> &'static str {
        use typst_ide::CompletionKind as CK;
        match kind {
            CK::Syntax => "code-symbolic",
            CK::Func => "lang-function-symbolic",
            CK::Type => "lang-class-symbolic",
            CK::Param => "configure-symbolic",
            CK::Constant => "lang-namespace-symbolic",
            CK::Symbol(_) => "accessories-character-map-symbolic",
            CK::Path => "folder-symbolic",
            CK::Package => "package-x-generic-symbolic",
            CK::Label => "format-text-rich-symbolic",
            CK::Font => "font-x-generic-symbolic",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_imp::ChunkInfo;

    #[test]
    fn test_parse_apply_simple() {
        let apply = "title";
        let chunks = provider_imp::TypesetterCompletionProvider::parse_apply_string(apply);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "title");
        assert_eq!(chunks[0].focus_position, None);
    }

    #[test]
    fn test_parse_apply_with_placeholder() {
        let apply = "title(${})";
        let chunks = provider_imp::TypesetterCompletionProvider::parse_apply_string(apply);
        assert_eq!(chunks.len(), 3);
        assert_eq!(
            chunks[0],
            ChunkInfo {
                text: "title(".to_string(),
                focus_position: None
            }
        );
        assert_eq!(
            chunks[1],
            ChunkInfo {
                text: "".to_string(),
                focus_position: Some(1)
            }
        );
        assert_eq!(
            chunks[2],
            ChunkInfo {
                text: ")".to_string(),
                focus_position: None
            }
        );
    }

    #[test]
    fn test_parse_apply_with_named_placeholder() {
        let apply = "import \"${file}.typ\": ${items}";
        let chunks = provider_imp::TypesetterCompletionProvider::parse_apply_string(apply);
        assert_eq!(
            chunks[0],
            ChunkInfo {
                text: "import \"".to_string(),
                focus_position: None
            }
        );
        assert_eq!(
            chunks[1],
            ChunkInfo {
                text: "file".to_string(),
                focus_position: Some(1)
            }
        );
        assert_eq!(
            chunks[2],
            ChunkInfo {
                text: ".typ\": ".to_string(),
                focus_position: None
            }
        );
        assert_eq!(
            chunks[3],
            ChunkInfo {
                text: "items".to_string(),
                focus_position: Some(2)
            }
        );
    }

    #[test]
    fn test_parse_apply_math_block() {
        let apply = "$ ${sum_x^2} $";
        let chunks = provider_imp::TypesetterCompletionProvider::parse_apply_string(apply);
        assert_eq!(
            chunks[0],
            ChunkInfo {
                text: "$ ".to_string(),
                focus_position: None
            }
        );
        assert_eq!(
            chunks[1],
            ChunkInfo {
                text: "sum_x^2".to_string(),
                focus_position: Some(1)
            }
        );
        assert_eq!(
            chunks[2],
            ChunkInfo {
                text: " $".to_string(),
                focus_position: None
            }
        );
    }

    #[test]
    fn test_parse_apply_multiple_placeholders() {
        let apply = "${a} and ${b} and ${c}";
        let chunks = provider_imp::TypesetterCompletionProvider::parse_apply_string(apply);
        assert_eq!(chunks.len(), 5);
        assert_eq!(
            chunks[0],
            ChunkInfo {
                text: "a".to_string(),
                focus_position: Some(1)
            }
        );
        assert_eq!(
            chunks[1],
            ChunkInfo {
                text: " and ".to_string(),
                focus_position: None
            }
        );
        assert_eq!(
            chunks[2],
            ChunkInfo {
                text: "b".to_string(),
                focus_position: Some(2)
            }
        );
        assert_eq!(
            chunks[3],
            ChunkInfo {
                text: " and ".to_string(),
                focus_position: None
            }
        );
        assert_eq!(
            chunks[4],
            ChunkInfo {
                text: "c".to_string(),
                focus_position: Some(3)
            }
        );
    }

    #[test]
    fn test_parse_apply_no_placeholders() {
        let apply = "plain text";
        let chunks = provider_imp::TypesetterCompletionProvider::parse_apply_string(apply);
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0],
            ChunkInfo {
                text: "plain text".to_string(),
                focus_position: None
            }
        );
    }
}
