// Copyright 2026 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::{gdk, pango, prelude::*};
use harper_core::{
    Dialect, Document,
    linting::{LintGroup, Linter, Suggestion},
    spell::FstDictionary,
};
use harper_typst::Typst;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

// ─────────────────────────────────────────────────────────────────────────────
// § 1  Language / dialect helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` if Harper should be active for the given language tag.
///
/// Accepts both BCP 47 hyphens (`"en-GB"`) and locale underscores (`"en_GB"`).
pub(crate) fn harper_should_be_active(language_tag: &str) -> bool {
    language_tag == "en" || language_tag.starts_with("en-") || language_tag.starts_with("en_")
}

/// Map a BCP 47 language tag to a Harper [`Dialect`].
///
/// Only the subtag after `"en-"` is examined; the match is
/// case-insensitive so `"en-gb"` and `"en-GB"` both work.
/// Any tag that does not match a known English regional
/// variant falls back to American English.
pub fn dialect_from_bcp47(tag: &str) -> Dialect {
    let region = tag
        .split_once(['-', '_'])
        .map(|(_, rest)| rest.to_ascii_uppercase())
        .unwrap_or_default();

    match region.as_str() {
        "GB" => Dialect::British,
        "AU" => Dialect::Australian,
        "CA" => Dialect::Canadian,
        "IN" => Dialect::Indian,
        _ => Dialect::American,
    }
}

#[cfg(test)]
mod language_guard_tests {
    use super::*;

    #[test]
    fn test_english_variants_active() {
        assert!(harper_should_be_active("en"));
        assert!(harper_should_be_active("en-GB"));
        assert!(harper_should_be_active("en-US"));
        assert!(harper_should_be_active("en-AU"));
        assert!(harper_should_be_active("en-CA"));
        assert!(harper_should_be_active("en-IN"));
    }

    #[test]
    fn test_underscore_locale_active() {
        assert!(harper_should_be_active("en_US"));
        assert!(harper_should_be_active("en_GB"));
        assert!(harper_should_be_active("en_AU"));
    }

    #[test]
    fn test_non_english_inactive() {
        assert!(!harper_should_be_active("de-DE"));
        assert!(!harper_should_be_active("fr"));
        assert!(!harper_should_be_active(""));
        assert!(!harper_should_be_active("enhance"));
        assert!(harper_should_be_active("en_th"));
    }
}

#[cfg(test)]
mod dialect_tests {
    use super::*;

    #[test]
    fn test_known_regions() {
        assert_eq!(dialect_from_bcp47("en-GB"), Dialect::British);
        assert_eq!(dialect_from_bcp47("en-AU"), Dialect::Australian);
        assert_eq!(dialect_from_bcp47("en-CA"), Dialect::Canadian);
        assert_eq!(dialect_from_bcp47("en-IN"), Dialect::Indian);
        assert_eq!(dialect_from_bcp47("en-US"), Dialect::American);
    }

    #[test]
    fn test_case_insensitive() {
        assert_eq!(dialect_from_bcp47("en-gb"), Dialect::British);
        assert_eq!(dialect_from_bcp47("en-Gb"), Dialect::British);
        assert_eq!(dialect_from_bcp47("en-au"), Dialect::Australian);
    }

    #[test]
    fn test_no_region_subtag() {
        assert_eq!(dialect_from_bcp47("en"), Dialect::American);
    }

    #[test]
    fn test_non_english_falls_back() {
        assert_eq!(dialect_from_bcp47("de-DE"), Dialect::American);
        assert_eq!(dialect_from_bcp47("fr-FR"), Dialect::American);
        assert_eq!(dialect_from_bcp47(""), Dialect::American);
    }

    #[test]
    fn test_extended_subtags_ignored() {
        assert_eq!(dialect_from_bcp47("en-GB-x-whatever"), Dialect::American);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// § 2  Core data types
// ─────────────────────────────────────────────────────────────────────────────

/// A suggested fix for a Harper lint.
///
/// Preserves the distinction between replacement, insertion, and removal
/// so that `apply_suggestion` can perform the correct buffer edit for each.
#[derive(Debug, Clone)]
pub(crate) enum LintSuggestion {
    /// Replace the lint span with this string.
    ReplaceWith(String),
    /// Insert this string after the lint span (the span itself is kept).
    InsertAfter(String),
    /// Delete the lint span entirely.
    Remove,
}

impl LintSuggestion {
    /// Short human-readable label for display in the context menu.
    ///
    /// Returns a borrowed string where possible to avoid allocation.
    /// `InsertAfter` cannot borrow because the label is constructed from
    /// the inner string alone; use `label_with_context` in that case for
    /// a richer display that includes the existing span text.
    pub(crate) fn label(&self) -> &str {
        match self {
            LintSuggestion::ReplaceWith(s) => s.as_str(),
            LintSuggestion::InsertAfter(_) => "Insert",
            LintSuggestion::Remove => "Remove",
        }
    }

    /// Context-aware label that incorporates the current span text.
    ///
    /// For `InsertAfter`, concatenates the existing span text with the
    /// string to be inserted, showing the user the final result rather
    /// than just the inserted fragment. For all other variants, falls
    /// back to `label`.
    pub(crate) fn label_with_context(&self, span_text: &str) -> String {
        match self {
            LintSuggestion::InsertAfter(s) => format!("{}{}", span_text, s),
            other => other.label().to_owned(),
        }
    }
}

/// The output of one Harper lint pass over a single paragraph.
///
/// `char_start` and `char_end` are character indices into the paragraph
/// string as Harper uses them internally — not byte offsets, not
/// document-level GTK offsets. The caller adds `para_gtk_start` to
/// convert to document-level GTK character offsets.
#[derive(Debug, Clone)]
pub(crate) struct LintResult {
    /// Character index of the lint span start within the paragraph string.
    pub char_start: usize,
    /// Character index of the lint span end within the paragraph string.
    pub char_end: usize,
    /// Human-readable lint message.
    pub message: String,
    /// Typed suggestions preserving the distinction between replace,
    /// insert-after, and remove operations.
    pub suggestions: Vec<LintSuggestion>,
}

/// A single dirty paragraph range, expressed as GTK character offsets.
///
/// `gtk_start` and `gtk_end` are character offsets (codepoint counts,
/// not byte offsets) as used by `gtk::TextBuffer` / `gtk::TextIter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirtyRange {
    /// Inclusive start offset of the dirty region, in GTK codepoint units.
    pub gtk_start: i32,
    /// Inclusive end offset of the dirty region, in GTK codepoint units.
    pub gtk_end: i32,
}

/// Tracks which regions of the buffer have been modified since the
/// last Harper lint pass and need to be re-linted.
///
/// Ranges are accumulated cheaply on every keystroke via `mark_dirty`
/// and only sorted and merged once, in `take_dirty`, when the lint
/// runner is ready to consume them.
#[derive(Debug, Default)]
pub(crate) struct DirtyRanges {
    ranges: Vec<DirtyRange>,
}

impl DirtyRanges {
    /// Create an empty tracker.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Append a character-offset range that needs re-linting.
    ///
    /// Intentionally cheap — just pushes to the vec. Merging is
    /// deferred to `take_dirty` so that rapid successive edits (e.g.
    /// the user typing) do not pay the sort-and-merge cost on every
    /// keystroke.
    pub(crate) fn mark_dirty(&mut self, start: i32, end: i32) {
        debug_assert!(start <= end, "mark_dirty: start ({start}) > end ({end})");
        self.ranges.push(DirtyRange {
            gtk_start: start,
            gtk_end: end,
        });
    }

    /// Sort all pending ranges by start offset and merge any that
    /// overlap or are directly adjacent into a single span.
    ///
    /// Called once inside `take_dirty`; not exposed publicly because
    /// callers should never need to trigger a merge without consuming
    /// the result.
    fn merge_overlapping(&mut self) {
        if self.ranges.len() < 2 {
            return;
        }

        self.ranges.sort_by_key(|r| r.gtk_start);

        let mut merged: Vec<DirtyRange> = Vec::with_capacity(self.ranges.len());
        for range in self.ranges.drain(..) {
            if let Some(last) = merged.last_mut()
                && range.gtk_start <= last.gtk_end + 1
            {
                last.gtk_end = last.gtk_end.max(range.gtk_end);
                continue;
            }
            merged.push(range);
        }

        self.ranges = merged;
    }

    /// Merge all pending ranges and return them, leaving the tracker empty.
    ///
    /// This is the only point at which the sort-and-merge runs, so the
    /// cost is paid once per lint pass rather than once per keystroke.
    pub(crate) fn take_dirty(&mut self) -> Vec<DirtyRange> {
        self.merge_overlapping();
        std::mem::take(&mut self.ranges)
    }

    /// Discard all pending dirty ranges without running Harper.
    ///
    /// Use this when the buffer is reset or Harper is disabled mid-session,
    /// and any accumulated dirty state should be thrown away.
    pub(crate) fn clear(&mut self) {
        self.ranges.clear();
    }

    /// Returns `true` if there are no pending dirty ranges.
    ///
    /// Does not trigger a merge — the raw (un-merged) count is sufficient
    /// for the emptiness check.
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Returns the number of pending dirty ranges before merging.
    ///
    /// Intended for debug logging only; the post-merge count may be
    /// lower. Does not trigger a merge.
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.ranges.len()
    }
}

#[cfg(test)]
mod dirty_ranges_tests {
    use super::*;

    #[test]
    fn test_single_range() {
        let mut dr = DirtyRanges::new();
        dr.mark_dirty(0, 10);
        let taken = dr.take_dirty();
        assert_eq!(
            taken,
            vec![DirtyRange {
                gtk_start: 0,
                gtk_end: 10
            }]
        );
        assert!(dr.take_dirty().is_empty());
    }

    #[test]
    fn test_non_overlapping_ranges_stay_separate() {
        let mut dr = DirtyRanges::new();
        dr.mark_dirty(0, 5);
        dr.mark_dirty(10, 20);
        let taken = dr.take_dirty();
        assert_eq!(
            taken,
            vec![
                DirtyRange {
                    gtk_start: 0,
                    gtk_end: 5
                },
                DirtyRange {
                    gtk_start: 10,
                    gtk_end: 20
                },
            ]
        );
    }

    #[test]
    fn test_overlapping_ranges_merge() {
        let mut dr = DirtyRanges::new();
        dr.mark_dirty(0, 15);
        dr.mark_dirty(10, 25);
        let taken = dr.take_dirty();
        assert_eq!(
            taken,
            vec![DirtyRange {
                gtk_start: 0,
                gtk_end: 25
            }]
        );
    }

    #[test]
    fn test_adjacent_ranges_merge() {
        let mut dr = DirtyRanges::new();
        dr.mark_dirty(0, 10);
        dr.mark_dirty(11, 20);
        let taken = dr.take_dirty();
        assert_eq!(
            taken,
            vec![DirtyRange {
                gtk_start: 0,
                gtk_end: 20
            }]
        );
    }

    #[test]
    fn test_out_of_order_ranges_sorted_then_merged() {
        let mut dr = DirtyRanges::new();
        dr.mark_dirty(50, 60);
        dr.mark_dirty(0, 10);
        dr.mark_dirty(8, 55);
        let taken = dr.take_dirty();
        assert_eq!(
            taken,
            vec![DirtyRange {
                gtk_start: 0,
                gtk_end: 60
            }]
        );
    }

    #[test]
    fn test_clear_discards_ranges() {
        let mut dr = DirtyRanges::new();
        dr.mark_dirty(0, 100);
        dr.clear();
        assert!(dr.take_dirty().is_empty());
    }

    #[test]
    fn test_take_dirty_resets_state() {
        let mut dr = DirtyRanges::new();
        dr.mark_dirty(0, 10);
        let _ = dr.take_dirty();
        assert!(dr.take_dirty().is_empty());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// § 3  Buffer integration
// ─────────────────────────────────────────────────────────────────────────────

/// Name of the `gtk::TextTag` used to mark Harper lint spans.
pub(crate) const HARPER_LINT_TAG: &str = "harper-lint";

/// Create the `"harper-lint"` text tag on `buffer` if it does not
/// already exist.
///
/// The tag renders a blue wavy underline, visually distinct from:
/// - the red wavy underline used by libspelling for spelling errors
/// - the red/yellow solid underlines used for Typst diagnostics
///
/// Safe to call multiple times — returns early if the tag is already
/// registered in the buffer's tag table.
pub(crate) fn ensure_harper_tag(buffer: &sourceview::Buffer) {
    let tag_table = buffer.tag_table();

    if tag_table.lookup(HARPER_LINT_TAG).is_some() {
        return;
    }

    let tag = gtk::TextTag::new(Some(HARPER_LINT_TAG));

    tag.set_property("underline", pango::Underline::Error);

    let blue = gdk::RGBA::parse("#62a0ea").expect("Invalid colour string");
    tag.set_property("underline-rgba", blue);

    tag_table.add(&tag);
}

/// Return the GTK character offsets of the paragraph boundaries
/// containing `iter`.
///
/// Returns `(start, end)` as GTK character offsets where `start` is
/// the first character of the paragraph and `end` is one past the
/// trailing `\n` (or the buffer end offset).
pub(crate) fn paragraph_bounds(iter: &gtk::TextIter) -> (i32, i32) {
    let mut start = *iter;
    // Walk backward to find a blank line (or buffer start)
    loop {
        if start.line() == 0 {
            start.set_line_offset(0);
            break;
        }
        start.backward_line();
        if start.ends_line() {
            // This line is blank — block starts on the next line
            start.forward_line();
            break;
        }
    }

    let mut end = *iter;
    // Walk forward to find a blank line (or buffer end)
    loop {
        end.forward_to_line_end();
        if end.is_end() {
            break;
        }
        end.forward_char(); // move past the \n
        if end.ends_line() {
            // This line is blank — block ends here
            break;
        }
    }

    (start.offset(), end.offset())
}

/// A single Harper lint anchored into the `sourceview::Buffer` via
/// `GtkTextMark` pairs.
///
/// Using marks instead of raw character offsets means GTK automatically
/// tracks the lint's position as text is inserted or deleted before it,
/// so the squiggle stays under the right word even after edits elsewhere
/// in the document.
#[derive(Debug)]
pub(crate) struct HarperLint {
    /// Mark anchoring the start of the lint span.
    /// Left-gravity so insertions *at* the start position do not push
    /// the mark rightward into the span.
    pub start_mark: gtk::TextMark,

    /// Mark anchoring the end of the lint span.
    /// Right-gravity (the GTK default) so insertions *at* the end
    /// position extend the span naturally.
    pub end_mark: gtk::TextMark,

    /// Human-readable lint message shown in the hover tooltip.
    pub message: String,

    /// Typed suggestions preserving the distinction between replace,
    /// insert-after, and remove operations.
    pub suggestions: Vec<LintSuggestion>,
}

impl HarperLint {
    /// Create a new lint and insert its marks into `buffer`.
    ///
    /// `start` and `end` are GTK character offsets (codepoints, not bytes).
    pub(crate) fn new(
        buffer: &sourceview::Buffer,
        start: i32,
        end: i32,
        message: impl Into<String>,
        suggestions: Vec<LintSuggestion>,
    ) -> Self {
        let start_iter = buffer.iter_at_offset(start);
        let end_iter = buffer.iter_at_offset(end);

        // Left-gravity: the start mark stays to the left of any text
        // inserted at its position, keeping the span from growing leftward.
        let start_mark = buffer.create_mark(None, &start_iter, true);

        // Right-gravity (false = right): the end mark moves right when
        // text is inserted at its position, keeping the span from shrinking.
        let end_mark = buffer.create_mark(None, &end_iter, false);

        Self {
            start_mark,
            end_mark,
            message: message.into(),
            suggestions,
        }
    }

    /// Resolve the current GTK character offset of the start mark.
    ///
    /// Returns `None` if the mark has already been deleted from the buffer.
    pub(crate) fn current_start(&self, buffer: &sourceview::Buffer) -> Option<i32> {
        if self.start_mark.is_deleted() {
            return None;
        }
        Some(buffer.iter_at_mark(&self.start_mark).offset())
    }

    /// Resolve the current GTK character offset of the end mark.
    ///
    /// Returns `None` if the mark has already been deleted from the buffer.
    pub(crate) fn current_end(&self, buffer: &sourceview::Buffer) -> Option<i32> {
        if self.end_mark.is_deleted() {
            return None;
        }
        Some(buffer.iter_at_mark(&self.end_mark).offset())
    }

    /// Returns `true` if the lint's current span overlaps `[range_start, range_end)`.
    pub(crate) fn overlaps_range(
        &self,
        buffer: &sourceview::Buffer,
        range_start: i32,
        range_end: i32,
    ) -> bool {
        match (self.current_start(buffer), self.current_end(buffer)) {
            (Some(s), Some(e)) => s < range_end && e > range_start,
            _ => false,
        }
    }

    /// Delete both marks from the buffer, freeing GTK's internal resources.
    ///
    /// Must be called whenever a lint is removed from the lint list.
    /// Failing to call this leaks marks into the buffer indefinitely.
    pub(crate) fn delete_marks(&self, buffer: &sourceview::Buffer) {
        if !self.start_mark.is_deleted() {
            buffer.delete_mark(&self.start_mark);
        }
        if !self.end_mark.is_deleted() {
            buffer.delete_mark(&self.end_mark);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// § 4  Lint lifecycle
// ─────────────────────────────────────────────────────────────────────────────

/// Remove all Harper lint tags and delete all mark pairs from `buffer`,
/// then clear the lint list.
///
/// Call this when Harper is disabled (language changed to non-English)
/// or when re-linting a range that invalidates existing lints.
pub(crate) fn clear_all_lints(buffer: &sourceview::Buffer, lints: &mut Vec<HarperLint>) {
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    buffer.remove_tag_by_name(HARPER_LINT_TAG, &start, &end);

    // Delete every mark pair explicitly — GTK leaks marks that are
    // never deleted.
    for lint in lints.drain(..) {
        lint.delete_marks(buffer);
    }
}

/// Apply a set of `LintResult`s to `buffer` within a paragraph range.
///
/// Before inserting new lints, removes all existing Harper lints and
/// their marks that overlap `[para_gtk_start, para_gtk_end)`.
pub(crate) fn apply_lint_results(
    buffer: &sourceview::Buffer,
    lints_store: &mut Vec<HarperLint>,
    results: Vec<LintResult>,
    para_gtk_start: i32,
    para_gtk_end: i32,
) {
    // ── 1. Remove stale lints that overlap this paragraph range ───────
    // swap_remove is O(1) and order within lints_store does not matter
    // for correctness, so no allocation is needed here.
    let mut i = 0;
    while i < lints_store.len() {
        if lints_store[i].overlaps_range(buffer, para_gtk_start, para_gtk_end) {
            let lint = lints_store.swap_remove(i);
            lint.delete_marks(buffer);
            // Do not increment i: the element swapped into position i
            // has not been checked yet.
        } else {
            i += 1;
        }
    }

    // ── 2. Remove the tag from this paragraph range ───────────────────
    let start_iter = buffer.iter_at_offset(para_gtk_start);
    let end_iter = buffer.iter_at_offset(para_gtk_end);
    buffer.remove_tag_by_name(HARPER_LINT_TAG, &start_iter, &end_iter);

    // ── 3. Insert new lints ───────────────────────────────────────────
    for result in results {
        let gtk_start =
            (para_gtk_start + result.char_start as i32).clamp(para_gtk_start, para_gtk_end);
        let gtk_end = (para_gtk_start + result.char_end as i32).clamp(para_gtk_start, para_gtk_end);

        // Skip zero-length spans.
        if gtk_start >= gtk_end {
            continue;
        }

        let start_iter = buffer.iter_at_offset(gtk_start);
        let end_iter = buffer.iter_at_offset(gtk_end);
        buffer.apply_tag_by_name(HARPER_LINT_TAG, &start_iter, &end_iter);

        let lint = HarperLint::new(
            buffer,
            gtk_start,
            gtk_end,
            result.message,
            result.suggestions,
        );
        lints_store.push(lint);
    }
}

/// Apply a fix suggestion to the buffer, removing the corresponding lint.
///
/// Resolves the marks to their current positions and performs the
/// appropriate edit depending on the suggestion type:
/// - `ReplaceWith` — deletes the span and inserts the replacement
/// - `InsertAfter` — inserts after the span without deleting it
/// - `Remove` — deletes the span
///
/// The lint whose marks match is removed and its marks deleted before
/// the buffer edit to avoid stale mark access.
pub(crate) fn apply_suggestion(
    buffer: &sourceview::Buffer,
    lints_store: &mut Vec<HarperLint>,
    start_mark: &gtk::TextMark,
    end_mark: &gtk::TextMark,
    suggestion: &LintSuggestion,
) {
    if start_mark.is_deleted() || end_mark.is_deleted() {
        return;
    }

    let mut start_iter = buffer.iter_at_mark(start_mark);
    let mut end_iter = buffer.iter_at_mark(end_mark);

    // Remove the lint entry before editing to avoid stale mark access.
    if let Some(pos) = lints_store
        .iter()
        .position(|l| l.start_mark == *start_mark && l.end_mark == *end_mark)
    {
        let lint = lints_store.remove(pos);
        lint.delete_marks(buffer);
    }

    buffer.remove_tag_by_name(HARPER_LINT_TAG, &start_iter, &end_iter);

    buffer.begin_user_action();
    match suggestion {
        LintSuggestion::ReplaceWith(text) => {
            buffer.delete(&mut start_iter, &mut end_iter);
            buffer.insert(&mut start_iter, text);
        }
        LintSuggestion::InsertAfter(text) => {
            // Don't touch the span — insert after it.
            buffer.insert(&mut end_iter, text);
        }
        LintSuggestion::Remove => {
            buffer.delete(&mut start_iter, &mut end_iter);
        }
    }
    buffer.end_user_action();
}

// ─────────────────────────────────────────────────────────────────────────────
// § 5  Async worker
// ─────────────────────────────────────────────────────────────────────────────

/// A batch of paragraphs to lint in a single worker request.
/// Each entry is `(paragraph_text, para_gtk_start, para_gtk_end)`.
pub(crate) type LintBatch = Vec<(String, i32, i32)>;

/// The result of linting one paragraph.
#[derive(Debug)]
pub(crate) struct ParagraphLintResult {
    /// GTK character offset of the paragraph's first character.
    pub para_gtk_start: i32,
    /// GTK character offset one past the paragraph's last character.
    pub para_gtk_end: i32,
    /// All lints found within this paragraph, with offsets relative to
    /// `para_gtk_start` (as returned by Harper's span indices).
    pub lints: Vec<LintResult>,
}

/// Message sent to the worker.
enum HarperMessage {
    Lint {
        batch: LintBatch,
        dialect: Dialect,
        response: oneshot::Sender<Vec<ParagraphLintResult>>,
    },
}

/// Async worker that runs Harper linting off the main thread.
///
/// The worker owns the `FstDictionary` so it is loaded only once
/// for the lifetime of the editor instance.
/// The `ready` watch channel starts as `false` and is set to `true`
/// once the `FstDictionary` has finished loading. Callers that need to
/// wait for the dictionary (e.g. `lint_full_document` on first load)
/// should await `lint_batch`, which parks until `ready` is `true`
/// before sending. This prevents the initial lint request from racing
/// against dictionary loading and silently dropping.
#[derive(Clone, Debug)]
pub(crate) struct HarperWorker {
    cancellation_token: CancellationToken,
    sender: mpsc::Sender<HarperMessage>,
    /// Receives `true` once the `FstDictionary` has been loaded and the
    /// worker message loop is ready to accept lint requests.
    ready: watch::Receiver<bool>,
}

impl HarperWorker {
    /// Spawn the worker task and return a handle.
    ///
    /// Loads the `FstDictionary` once on a blocking thread during
    /// initialisation, signals `ready`, then enters the message loop.
    /// The dictionary is kept alive for the worker's entire lifetime
    /// via an `Arc`.
    pub(crate) fn new() -> Self {
        let cancellation_token = CancellationToken::new();
        let (sender, mut receiver) = mpsc::channel::<HarperMessage>(4);
        let (ready_tx, ready_rx) = watch::channel(false);

        let worker = Self {
            cancellation_token: cancellation_token.clone(),
            sender,
            ready: ready_rx,
        };

        // Load the dictionary once on the blocking pool at startup,
        // then keep it alive for the worker's lifetime.
        let runtime = crate::application::TypesetterApplication::tokio_runtime();
        runtime.spawn(async move {
            // Load dictionary once, up front, on a blocking thread so we
            // don't block the async executor during FST parsing.
            let dictionary = match tokio::task::spawn_blocking(FstDictionary::curated).await {
                Ok(dict) => dict,
                Err(e) => {
                    eprintln!("HARPER: Failed to load dictionary: {}", e);
                    return;
                }
            };

            // Signal that the dictionary is loaded and the worker is ready
            // to accept lint requests. Any callers awaiting `ready` will
            // now be unparked.
            let _ = ready_tx.send(true);

            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        #[cfg(debug_assertions)]
                        println!("HarperWorker: Cancellation requested, stopping");
                        break;
                    }

                    msg = receiver.recv() => {
                        match msg {
                            Some(HarperMessage::Lint { batch, dialect, response }) => {
                                let dict = Arc::clone(&dictionary);
                                tokio::task::spawn_blocking(move || {
                                    let results = Self::handle_lint_batch(batch, dialect, dict);
                                    let _ = response.send(results);
                                });
                            }
                            None => {
                                #[cfg(debug_assertions)]
                                println!("HarperWorker: Channel closed, stopping");
                                break;
                            }
                        }
                    }
                }
            }
        });

        worker
    }

    /// Send a batch of paragraphs to the worker for linting and await the results.
    ///
    /// If the `FstDictionary` has not yet finished loading, this method
    /// parks the calling async task (without blocking the main thread)
    /// until the worker signals readiness, then sends the batch.
    ///
    /// Returns `Err` if the worker channel is closed (i.e. the worker task
    /// has already been cancelled or panicked) or if the oneshot reply
    /// channel is dropped before a result is sent.
    pub(crate) async fn lint_batch(
        &self,
        batch: LintBatch,
        dialect: Dialect,
    ) -> Result<Vec<ParagraphLintResult>, String> {
        // Park this async task (not the main thread) until the dictionary
        // is loaded. In the common case after first load, `ready` is
        // already `true` and this returns immediately without yielding.
        if !*self.ready.borrow() {
            let mut ready = self.ready.clone();
            ready.wait_for(|r| *r).await.map_err(|e| e.to_string())?;
        }

        let (tx, rx) = oneshot::channel();
        self.sender
            .send(HarperMessage::Lint {
                batch,
                dialect,
                response: tx,
            })
            .await
            .map_err(|e| e.to_string())?;
        rx.await.map_err(|e| e.to_string())
    }

    /// Signal the worker task to shut down.
    ///
    /// The cancellation is cooperative — the task exits cleanly at its
    /// next `tokio::select!` poll. Outstanding in-flight lint jobs may
    /// still complete before the task stops.
    pub(crate) fn cancel(&self) {
        self.cancellation_token.cancel();
    }

    /// Run Harper over a batch of paragraphs on the current blocking thread.
    ///
    /// Creates a single `LintGroup` for the whole batch so that per-batch
    /// setup cost is paid once. Both `LintGroup` and `Document` are `!Send`
    /// and therefore must be constructed, used, and dropped entirely within
    /// this function — they must never cross a thread boundary.
    ///
    /// Empty or whitespace-only paragraphs are skipped; they would produce
    /// no lints and would waste a Harper parse round-trip.
    fn handle_lint_batch(
        batch: LintBatch,
        dialect: Dialect,
        dictionary: Arc<FstDictionary>,
    ) -> Vec<ParagraphLintResult> {
        // Construct a fresh LintGroup for this batch.
        // LintGroup is !Send so it lives entirely within this closure.
        let mut linter = LintGroup::new_curated(Arc::clone(&dictionary), dialect);
        linter.config.set_rule_enabled("LongSentences", false);
        linter.config.set_rule_enabled("NoFrenchSpaces", false);
        linter.config.set_rule_enabled("SpellCheck", false);
        linter.config.set_rule_enabled("Spaces", false);
        linter.config.set_rule_enabled("UnclosedQuotes", false);

        batch
            .into_iter()
            .filter_map(|(text, para_gtk_start, para_gtk_end)| {
                if text.trim().is_empty() {
                    return None;
                }

                // Document is !Send so it also lives entirely within
                // this closure — created, used, and dropped here.
                let document = Document::new_curated(&text, &Typst);
                let lints = linter.lint(&document);

                #[cfg(debug_assertions)]
                for lint in &lints {
                    println!("HARPER: kind={:?} msg={}", lint.lint_kind, lint.message);
                }

                let results = lints
                    .into_iter()
                    .map(|lint| {
                        let char_start = lint.span.start;
                        let char_end = lint.span.end;
                        let message = lint.message;

                        let suggestions = lint
                            .suggestions
                            .iter()
                            .map(|s| match s {
                                Suggestion::ReplaceWith(chars) => {
                                    LintSuggestion::ReplaceWith(chars.iter().collect::<String>())
                                }
                                Suggestion::InsertAfter(chars) => {
                                    LintSuggestion::InsertAfter(chars.iter().collect::<String>())
                                }
                                Suggestion::Remove => LintSuggestion::Remove,
                            })
                            .collect();

                        LintResult {
                            char_start,
                            char_end,
                            message,
                            suggestions,
                        }
                    })
                    .collect();

                Some(ParagraphLintResult {
                    para_gtk_start,
                    para_gtk_end,
                    lints: results,
                })
            })
            .collect()
    }
}
