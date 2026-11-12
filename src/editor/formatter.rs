// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gettextrs::gettext;
use gtk::prelude::*;
use similar::{ChangeTag, TextDiff};
use typstyle_core::{Config, Typstyle};

use super::TypesetterEditor;
use crate::window::TypesetterWindow;

fn count_whole_word_matches_before(text: &str, word: &str) -> usize {
    if word.is_empty() {
        return 0;
    }
    text.match_indices(word)
        .filter(|(byte_pos, matched)| {
            let before_ok = *byte_pos == 0
                || !text[..*byte_pos]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
            let after = byte_pos + matched.len();
            let after_ok = after >= text.len()
                || !text[after..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
            before_ok && after_ok
        })
        .count()
}

fn find_nth_whole_word(text: &str, word: &str, n: usize) -> Option<usize> {
    if word.is_empty() {
        return None;
    }
    text.match_indices(word)
        .filter(|(byte_pos, matched)| {
            let before_ok = *byte_pos == 0
                || !text[..*byte_pos]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
            let after = byte_pos + matched.len();
            let after_ok = after >= text.len()
                || !text[after..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
            before_ok && after_ok
        })
        .nth(n)
        .map(|(byte_pos, _)| byte_pos)
}

/// Applies the diff between `old_text` and `new_text` to the buffer by replacing
/// only the lines that changed. Hunks are applied in reverse order so that
/// byte offsets of earlier hunks remain valid after each edit.
///
/// Using line-level diffing keeps the unchanged regions of the buffer, and
/// therefore the viewport, untouched by GTK, minimizing a scroll jump.
fn apply_formatted_diff(buffer: &sourceview::Buffer, old_text: &str, new_text: &str) {
    let diff = TextDiff::from_lines(old_text, new_text);

    // Collect contiguous changed regions as (old_byte_start, old_byte_end,
    // replacement_text) hunks. Adjacent delete/insert changes are merged into
    // a single hunk so we make as few buffer operations as possible.
    let mut hunks: Vec<(usize, usize, String)> = Vec::new();
    let mut old_pos: usize = 0;
    let mut current_delete_start: Option<usize> = None;
    let mut current_delete_end: usize = 0;
    let mut current_insert = String::new();

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                if let Some(start) = current_delete_start.take() {
                    hunks.push((
                        start,
                        current_delete_end,
                        std::mem::take(&mut current_insert),
                    ));
                }
                old_pos += change.value().len();
            }
            ChangeTag::Delete => {
                if current_delete_start.is_none() {
                    current_delete_start = Some(old_pos);
                }
                current_delete_end = old_pos + change.value().len();
                old_pos += change.value().len();
            }
            ChangeTag::Insert => {
                if current_delete_start.is_none() {
                    // Pure insertion with no preceding delete — anchor at
                    // current old_pos with a zero-length delete range.
                    current_delete_start = Some(old_pos);
                    current_delete_end = old_pos;
                }
                current_insert.push_str(change.value());
            }
        }
    }
    // Flush any trailing hunk
    if let Some(start) = current_delete_start {
        hunks.push((start, current_delete_end, current_insert));
    }

    if hunks.is_empty() {
        return;
    }

    buffer.begin_user_action();

    // Apply in reverse order so that byte offsets of earlier hunks are not
    // shifted by edits made to later (higher-offset) regions.
    for (byte_start, byte_end, replacement) in hunks.into_iter().rev() {
        let char_start = old_text[..byte_start].chars().count() as i32;
        let char_end = old_text[..byte_end].chars().count() as i32;

        let mut start_iter = buffer.iter_at_offset(char_start);
        let mut end_iter = buffer.iter_at_offset(char_end);

        buffer.delete(&mut start_iter, &mut end_iter);
        if !replacement.is_empty() {
            let mut insert_iter = buffer.iter_at_offset(char_start);
            buffer.insert(&mut insert_iter, &replacement);
        }
    }

    buffer.end_user_action();
}

pub fn format_document(editor: &TypesetterEditor) {
    let buffer = editor.buffer();

    let insert_mark = buffer.get_insert();
    let cursor_iter = buffer.iter_at_mark(&insert_mark);

    let mut word_start = cursor_iter;
    let mut word_end = cursor_iter;
    if !cursor_iter.starts_word() {
        word_start.backward_word_start();
    }
    if !cursor_iter.ends_word() {
        word_end.forward_word_end();
    }

    let anchor_word = buffer.text(&word_start, &word_end, false).to_string();
    // Clamped to 0 defensively.
    let offset_in_word = (cursor_iter.offset() - word_start.offset()).max(0);
    // Save raw offset before iters are invalidated by buffer mutation.
    let cursor_offset_fallback = cursor_iter.offset();
    // Count whole-word occurrences before the cursor so we can find the same
    // occurrence in the formatted text. Whole-word matching avoids e.g. "in"
    // matching inside "index" or "inline" and inflating the count.
    let occurrence_before_cursor = {
        let full_text = buffer
            .text(&buffer.start_iter(), &word_start, false)
            .to_string();
        count_whole_word_matches_before(&full_text, &anchor_word)
    };

    let text = editor.contents();

    let config = Config {
        tab_spaces: 2,
        max_width: 65,
        blank_lines_upper_bound: 1,
        collapse_markup_spaces: true,
        reorder_import_items: true,
        wrap_text: false,
    };

    match Typstyle::new(config).format_text(&text).render() {
        Ok(mut formatted) => {
            if formatted.ends_with('\n') && !text.ends_with('\n') {
                formatted.pop();
            }

            if text != formatted {
                apply_formatted_diff(buffer, &text, &formatted);

                // Restore cursor. Though not optimal, if the cursor was on whitespace or
                // punctuation, anchor_word will be empty and we fall through
                // to the offset fallback.
                let new_offset = if !anchor_word.is_empty() {
                    find_nth_whole_word(&formatted, &anchor_word, occurrence_before_cursor)
                        .map(|byte_pos| {
                            let char_pos = formatted[..byte_pos].chars().count() as i32;
                            (char_pos + offset_in_word).min(buffer.char_count())
                        })
                        .unwrap_or_else(|| cursor_offset_fallback.min(buffer.char_count()))
                } else {
                    cursor_offset_fallback.min(buffer.char_count())
                };

                buffer.place_cursor(&buffer.iter_at_offset(new_offset));
            }
        }
        Err(_) => {
            if let Some(window) = editor
                .root()
                .and_then(|r| r.downcast::<TypesetterWindow>().ok())
            {
                window.show_toast(&gettext("Unable to clean up formatting"), None);
            }
        }
    }
}
