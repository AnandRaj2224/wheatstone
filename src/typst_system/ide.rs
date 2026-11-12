// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use typst::{
    World, WorldExt,
    introspection::{Introspector, PagedPosition},
    layout::{FrameItem, Point},
    model::Outlinable,
    syntax::{Side, Source},
};
use typst_ide::Completion;
use unicode_script::{Script, UnicodeScript};

use super::DocumentArena;
use super::types::{
    CompilerError, DocumentHeading, DocumentStats, HoverResult, JumpResult, PreviewLocation,
    SourceLocation, TooltipContent,
};

use crate::utils;

#[derive(Clone, Debug)]
pub struct IdeWorker {
    cancellation_token: CancellationToken,
    sender: mpsc::Sender<IdeMessage>,
}

enum IdeMessage {
    JumpFromPreview {
        page: usize,
        x: f32,
        y: f32,
        response: oneshot::Sender<Result<JumpResult, CompilerError>>,
    },
    JumpFromSource {
        cursor_offset: usize,
        response: oneshot::Sender<Result<JumpResult, CompilerError>>,
    },
    Autocomplete {
        cursor_offset: usize,
        explicit: bool,
        response: oneshot::Sender<Result<Vec<Completion>, CompilerError>>,
    },
    Hover {
        cursor_offset: usize,
        response: oneshot::Sender<Result<Option<HoverResult>, CompilerError>>,
    },
    DocumentStats {
        response: oneshot::Sender<Result<DocumentStats, CompilerError>>,
    },
    DocumentOutline {
        response: oneshot::Sender<Result<Vec<DocumentHeading>, CompilerError>>,
    },
}

impl IdeWorker {
    pub fn new(arena: Arc<DocumentArena>) -> Self {
        let cancellation_token = CancellationToken::new();
        let (sender, mut receiver) = mpsc::channel::<IdeMessage>(16);

        let worker = Self {
            cancellation_token: cancellation_token.clone(),
            sender: sender.clone(),
        };

        let runtime = crate::application::TypesetterApplication::tokio_runtime();
        runtime.spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        #[cfg(debug_assertions)]
                        println!("IdeWorker: Cancellation requested, stopping");
                        break;
                    }

                    msg = receiver.recv() => {
                        match msg {
                            Some(IdeMessage::JumpFromPreview { page, x, y, response }) => {
                                let arena = Arc::clone(&arena);
                                tokio::task::spawn_blocking(move || {
                                    let result = Self::handle_jump_from_preview(&arena, page, x, y);
                                    let _ = response.send(result);
                                });
                            }
                            Some(IdeMessage::JumpFromSource { cursor_offset, response }) => {
                                let arena = Arc::clone(&arena);
                                tokio::task::spawn_blocking(move || {
                                    let result = Self::handle_jump_from_source(&arena, cursor_offset);
                                    let _ = response.send(result);
                                });
                            }
                            Some(IdeMessage::Autocomplete { cursor_offset, explicit, response }) => {
                                let arena = Arc::clone(&arena);
                                tokio::task::spawn_blocking(move || {
                                    let result = Self::handle_autocomplete(&arena, cursor_offset, explicit);
                                    let _ = response.send(result);
                                });
                            }
                            Some(IdeMessage::Hover { cursor_offset, response }) => {
                                let arena = Arc::clone(&arena);
                                tokio::task::spawn_blocking(move || {
                                    let result = Self::handle_hover(&arena, cursor_offset);
                                    let _ = response.send(result);
                                });
                            }
                            Some(IdeMessage::DocumentStats { response }) => {
                                let arena = Arc::clone(&arena);
                                tokio::task::spawn_blocking(move || {
                                    let result = Self::handle_document_stats(&arena);
                                    let _ = response.send(result);
                                });
                            }
                            Some(IdeMessage::DocumentOutline { response }) => {
                                let arena = Arc::clone(&arena);
                                tokio::task::spawn_blocking(move || {
                                    let result = Self::handle_document_outline(&arena);
                                    let _ = response.send(result);
                                });
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        worker
    }

    pub async fn jump_from_preview(
        &self,
        page: usize,
        x: f32,
        y: f32,
    ) -> Result<JumpResult, CompilerError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(IdeMessage::JumpFromPreview {
                page,
                x,
                y,
                response: tx,
            })
            .await
            .map_err(|e| CompilerError::SendError(e.to_string()))?;
        rx.await
            .map_err(|e| CompilerError::RecvError(e.to_string()))?
    }

    pub async fn jump_from_source(
        &self,
        cursor_offset: usize,
    ) -> Result<JumpResult, CompilerError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(IdeMessage::JumpFromSource {
                cursor_offset,
                response: tx,
            })
            .await
            .map_err(|e| CompilerError::SendError(e.to_string()))?;
        rx.await
            .map_err(|e| CompilerError::RecvError(e.to_string()))?
    }

    pub async fn autocomplete(
        &self,
        cursor_offset: usize,
        explicit: bool,
    ) -> Result<Vec<Completion>, CompilerError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(IdeMessage::Autocomplete {
                cursor_offset,
                explicit,
                response: tx,
            })
            .await
            .map_err(|e| CompilerError::SendError(e.to_string()))?;
        rx.await
            .map_err(|e| CompilerError::RecvError(e.to_string()))?
    }

    pub async fn hover(&self, cursor_offset: usize) -> Result<Option<HoverResult>, CompilerError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(IdeMessage::Hover {
                cursor_offset,
                response: tx,
            })
            .await
            .map_err(|e| CompilerError::SendError(e.to_string()))?;
        rx.await
            .map_err(|e| CompilerError::RecvError(e.to_string()))?
    }

    pub async fn document_stats(&self) -> Result<DocumentStats, CompilerError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(IdeMessage::DocumentStats { response: tx })
            .await
            .map_err(|e| CompilerError::SendError(e.to_string()))?;
        rx.await
            .map_err(|e| CompilerError::RecvError(e.to_string()))?
    }

    pub async fn document_outline(&self) -> Result<Vec<DocumentHeading>, CompilerError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(IdeMessage::DocumentOutline { response: tx })
            .await
            .map_err(|e| CompilerError::SendError(e.to_string()))?;
        rx.await
            .map_err(|e| CompilerError::RecvError(e.to_string()))?
    }

    pub fn cancel(&self) {
        self.cancellation_token.cancel();
    }

    fn handle_jump_from_preview(
        arena: &DocumentArena,
        page: usize,
        x: f32,
        y: f32,
    ) -> Result<JumpResult, CompilerError> {
        // If the last compile failed, the world and document are from different
        // source versions. Clicks on the stale preview would resolve to source
        // locations in the old text, which would be confusing since the editor
        // now contains different content. Return None instead.
        if !arena.last_compile_succeeded() {
            return Ok(JumpResult::None);
        }

        arena.with_document(|world, doc_opt| {
            let Some(doc) = doc_opt else {
                return Ok(JumpResult::None);
            };

            let pages = doc.pages();
            if page >= pages.len() {
                return Ok(JumpResult::None);
            }

            let frame = &pages[page].frame;
            let page_width = frame.width();
            let page_height = frame.height();
            let point = Point::new(page_width * (x as f64), page_height * (y as f64));
            let position = PagedPosition {
                page: NonZeroUsize::new(page + 1).expect("page index is valid"),
                point,
            };

            match typst_ide::jump_from_click(world, doc, &position) {
                Some(typst_ide::Jump::Url(url)) => Ok(JumpResult::Url(url.to_string())),
                Some(typst_ide::Jump::File(file_id, offset)) => {
                    if file_id != world.main() {
                        return Ok(JumpResult::None);
                    }
                    Ok(JumpResult::Source(SourceLocation { offset }))
                }
                Some(typst_ide::Jump::Position(_)) => Ok(JumpResult::None),
                None => Ok(JumpResult::None),
            }
        })
    }

    fn handle_jump_from_source(
        arena: &DocumentArena,
        cursor_offset: usize,
    ) -> Result<JumpResult, CompilerError> {
        // If the last compile failed, the world's source text reflects the
        // erroring version but the document reflects the old layout. Cursor
        // offsets from the current editor text will not correspond to the
        // stale document's frames, so we skip the jump rather than jumping
        // to a misleading location.
        if !arena.last_compile_succeeded() {
            return Ok(JumpResult::None);
        }

        arena.with_document(|world, doc_opt| {
            let Some(doc) = doc_opt else {
                return Err(CompilerError::CompilationFailed(
                    "No document available for jump".to_string(),
                ));
            };

            let main_id = world.main();
            let source = world.source(main_id).map_err(|e| {
                CompilerError::CompilationFailed(format!("Failed to get source: {:?}", e))
            })?;

            if cursor_offset > source.text().len() {
                return Ok(JumpResult::None);
            }

            let positions = typst_ide::jump_from_cursor(doc, &source, cursor_offset);

            if positions.is_empty() {
                return Ok(JumpResult::None);
            }

            let locations: Vec<PreviewLocation> = positions
                .into_iter()
                .map(|pos| PreviewLocation {
                    page_number: pos.page.get() - 1,
                    x: pos.point.x.to_pt() as f32,
                    y: pos.point.y.to_pt() as f32,
                })
                .collect();

            if locations.len() > 1 {
                return Ok(JumpResult::None);
            }

            Ok(JumpResult::Preview(locations))
        })
    }

    fn handle_autocomplete(
        arena: &DocumentArena,
        cursor_offset: usize,
        explicit: bool,
    ) -> Result<Vec<Completion>, CompilerError> {
        arena.with_document(|world, doc_opt| {
            let main_id = world.main();
            let source = world.source(main_id).map_err(|e| {
                CompilerError::CompilationFailed(format!("Failed to get source: {:?}", e))
            })?;

            if cursor_offset > source.text().len() {
                return Ok(Vec::new());
            }

            let result = typst_ide::autocomplete(world, doc_opt, &source, cursor_offset, explicit);

            match result {
                Some((_offset, completions)) => Ok(completions),
                None => Ok(Vec::new()),
            }
        })
    }

    fn handle_hover(
        arena: &DocumentArena,
        cursor_offset: usize,
    ) -> Result<Option<HoverResult>, CompilerError> {
        arena.with_document(|world, doc_opt| {
            let main_id = world.main();
            let source = world.source(main_id).map_err(|e| {
                CompilerError::CompilationFailed(format!("Failed to get source: {:?}", e))
            })?;

            if cursor_offset > source.text().len() {
                return Ok(None);
            }

            let tooltip = typst_ide::tooltip(world, doc_opt, &source, cursor_offset, Side::Before);

            match tooltip {
                Some(ide_tooltip) => {
                    let content = TooltipContent::from(ide_tooltip);
                    let range = Self::find_hover_range(&source, cursor_offset);
                    Ok(Some(HoverResult { content, range }))
                }
                None => Ok(None),
            }
        })
    }

    fn find_hover_range(source: &Source, cursor_offset: usize) -> Option<(usize, usize)> {
        let text = source.text();
        let cursor_byte = utils::char_to_byte_offset(text, cursor_offset);
        let bytes = text.as_bytes();

        if cursor_byte >= bytes.len() {
            return None;
        }

        let is_ident_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';

        let mut start = cursor_byte;
        while start > 0 && is_ident_char(bytes[start - 1]) {
            start -= 1;
        }

        let mut end = cursor_byte;
        while end < bytes.len() && is_ident_char(bytes[end]) {
            end += 1;
        }

        if start == end {
            return None;
        }

        let start_char = utils::byte_to_char_offset(text, start);
        let end_char = utils::byte_to_char_offset(text, end);

        Some((start_char, end_char))
    }

    fn handle_document_stats(arena: &DocumentArena) -> Result<DocumentStats, CompilerError> {
        arena.with_document(|_world, doc_opt| {
            let Some(doc) = doc_opt else {
                return Err(CompilerError::CompilationFailed(
                    "No document available for statistics".to_string(),
                ));
            };

            let pages = doc.pages();
            let page_count = pages.len();
            let mut counter = StatsCounter::new();

            for page in pages {
                Self::count_frame(&page.frame, &mut counter);
            }

            Ok(counter.finish(page_count))
        })
    }

    fn handle_document_outline(
        arena: &DocumentArena,
    ) -> Result<Vec<DocumentHeading>, CompilerError> {
        arena.with_document(|world, doc_opt| {
            let Some(doc) = doc_opt else {
                return Err(CompilerError::CompilationFailed(
                    "No document available for outline".to_string(),
                ));
            };

            let main_id = world.main();

            let selector = typst::foundations::Selector::Elem(
                <typst::model::HeadingElem as typst::foundations::NativeElement>::ELEM,
                None,
            );

            let headings = doc
                .introspector()
                .query(&selector)
                .into_iter()
                .filter_map(|located| {
                    let elem = located.to_packed::<typst::model::HeadingElem>()?;

                    // Respect outlined: false
                    if !elem.outlined() {
                        return None;
                    }

                    // Get file_id from span, skip headings not in the main file
                    let span = elem.span();
                    let file_id = span.id()?;
                    if file_id != main_id {
                        return None;
                    }

                    // Get the byte range within the source
                    let range = world.range(span)?;

                    let level = elem
                        .resolve_level(typst::foundations::StyleChain::default())
                        .get();

                    let body_text = elem.body.plain_text().trim().to_string();
                    let label = match &elem.numbers {
                        Some(numbers) => format!("{}. {}", numbers, body_text),
                        None => body_text,
                    };

                    Some(DocumentHeading {
                        label,
                        level,
                        source_offset: range.start,
                    })
                })
                .collect();

            Ok(headings)
        })
    }

    fn count_frame(frame: &typst::layout::Frame, counter: &mut StatsCounter) {
        for (_, item) in frame.items() {
            match item {
                FrameItem::Group(group) => {
                    Self::count_frame(&group.frame, counter);
                }
                FrameItem::Text(text) => {
                    if !counter.in_equation {
                        counter.process_text(&text.text);
                        counter.reset_state();
                    }
                }
                FrameItem::Tag(tag) => {
                    use typst::introspection::Tag;
                    match tag {
                        Tag::Start(elem, _) => {
                            let func_name = elem.func().name();
                            match func_name {
                                "heading" => counter.increment_headings(),
                                "par" => counter.increment_paragraphs(),
                                "equation" => {
                                    counter.increment_equations();
                                    counter.enter_equation();
                                }
                                _ => {}
                            }
                        }
                        Tag::End(..) => {
                            if counter.in_equation {
                                counter.exit_equation();
                            }
                        }
                    }
                }
                FrameItem::Shape(..) | FrameItem::Image(..) | FrameItem::Link(..) => {}
            }
        }
    }
}

struct StatsCounter {
    words: usize,
    chars: usize,
    cjk_chars: usize,
    spaces: usize,
    paragraphs: usize,
    headings: usize,
    equations: usize,
    in_equation: bool,
    state: CountState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CountState {
    InSpace,
    InNonCJK,
    InCJK,
}

impl StatsCounter {
    fn new() -> Self {
        Self {
            words: 0,
            chars: 0,
            cjk_chars: 0,
            spaces: 0,
            paragraphs: 0,
            headings: 0,
            equations: 0,
            in_equation: false,
            state: CountState::InSpace,
        }
    }

    fn process_text(&mut self, text: &str) {
        for c in text.chars() {
            self.chars += 1;

            if c.is_whitespace() {
                if self.state != CountState::InSpace {
                    self.spaces += 1;
                }
                self.state = CountState::InSpace;
                continue;
            }

            if Self::is_cjk(c) {
                self.words += 1;
                self.cjk_chars += 1;
                self.state = CountState::InCJK;
            } else {
                if self.state != CountState::InNonCJK {
                    self.words += 1;
                }
                self.state = CountState::InNonCJK;
            }
        }
    }

    fn reset_state(&mut self) {
        self.state = CountState::InSpace;
    }

    fn increment_paragraphs(&mut self) {
        self.paragraphs += 1;
    }

    fn increment_headings(&mut self) {
        self.headings += 1;
    }

    fn increment_equations(&mut self) {
        self.equations += 1;
    }

    fn enter_equation(&mut self) {
        self.in_equation = true;
    }

    fn exit_equation(&mut self) {
        self.in_equation = false;
    }

    fn is_cjk(c: char) -> bool {
        matches!(
            c.script(),
            Script::Han
                | Script::Hiragana
                | Script::Katakana
                | Script::Hangul
                | Script::Bopomofo
                | Script::Yi
        ) || matches!(c as u32,
            0x3000..=0x303F | 0x3040..=0x309F | 0x30A0..=0x30FF |
            0x3300..=0x33FF | 0x3400..=0x4DBF |
            0xFF01..=0xFF60 | 0xFFE0..=0xFFEE
        )
    }

    fn finish(self, pages: usize) -> DocumentStats {
        DocumentStats {
            words: self.words,
            chars: self.chars,
            spaces: self.spaces,
            cjk_chars: self.cjk_chars,
            pages,
            paragraphs: self.paragraphs,
            headings: self.headings,
            equations: self.equations,
        }
    }
}
