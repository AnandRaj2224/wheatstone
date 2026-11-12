// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Delimiter auto-closing, skip-closer, and paired-backspace handling.
//!
//! # Design
//!
//! Mode (Code/Math/Markup) and escape-sequence detection are answered by
//! parsing the buffer's current text fresh — `typst::syntax::parse` +
//! `LinkedNode::mode_after()` — rather than approximating it from
//! GtkSourceView's lexer.
//!
//! This deliberately does *not* keep a persistent, incrementally-updated
//! `Source` around. That was tried and reverted: `Source::edit`'s text/
//! line-table half is not actually incremental — it re-scans everything
//! *after* the edit point on every call, regardless of edit size — and its
//! syntax-tree half never incrementally reparses inside math at all, and
//! falls back to a full document reparse elsewhere whenever an edit
//! doesn't isolate cleanly. A persistent `Source` therefore pays a
//! document-proportional cost on every keystroke anywhere in the document.
//! A fresh parse gated to just the 8 characters in [`PAIRS`] pays a
//! similar-shaped cost only on those characters, and nothing at all on
//! ordinary typing — which is strictly less work once the "incremental"
//! option turned out not to be cheap.
//!
//! Parsing happens at most once per relevant keystroke: [`parse_at`] is
//! called once, and its result is reused for every mode/escape query
//! needed while handling that keystroke, rather than re-parsing per query.

use gtk::{glib, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;
use typst::syntax::{LinkedNode, Side, SyntaxKind, SyntaxMode, SyntaxNode, parse};

// ── Delimiter pair table ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Pair {
    open: char,
    close: char,
    contexts: &'static [SyntaxMode],
}

impl Pair {
    fn is_same(&self) -> bool {
        self.open == self.close
    }

    fn is_escapable(&self) -> bool {
        matches!(self.open, '_' | '*' | '$' | '`')
    }

    /// Check if we should auto-close this pair at `iter` (current cursor
    /// position, corresponding to byte offset `byte_at` in `root`).
    fn should_close(&self, iter: &gtk::TextIter, byte_at: usize, root: &SyntaxNode) -> bool {
        let mut should_close = Self::next_is_not_alnum(iter);
        if self.is_same() {
            should_close &= Self::prev_is_not_alnum(iter, byte_at, root);
        }
        should_close
    }

    /// `TextIter::char()` returns the character immediately to the iter's
    /// right directly off the iter — no buffer read, no allocation. It's
    /// `'\0'` at end-of-buffer, which is correctly non-alphanumeric.
    fn next_is_not_alnum(iter: &gtk::TextIter) -> bool {
        let mut next = *iter;
        next.forward_char();
        !next.char().is_alphanumeric()
    }

    fn prev_is_not_alnum(iter: &gtk::TextIter, byte_at: usize, root: &SyntaxNode) -> bool {
        if iter.offset() == 0 {
            return true;
        }
        let mut prev = *iter;
        prev.backward_char();
        let prev_char = prev.char();

        // Treat an already-parsed escape sequence as word-like — don't
        // auto-close after it. The previous character's byte offset is
        // simple arithmetic from `byte_at`, since it's the character
        // immediately before it.
        let prev_byte = byte_at.saturating_sub(prev_char.len_utf8());
        if is_escape_at(root, prev_byte) {
            return false;
        }

        !prev_char.is_alphanumeric()
    }
}

const PAIRS: &[Pair] = &[
    Pair {
        open: '(',
        close: ')',
        contexts: &[SyntaxMode::Code],
    },
    Pair {
        open: '[',
        close: ']',
        contexts: &[SyntaxMode::Code],
    },
    Pair {
        open: '{',
        close: '}',
        contexts: &[SyntaxMode::Code],
    },
    Pair {
        open: '"',
        close: '"',
        contexts: &[SyntaxMode::Code, SyntaxMode::Math],
    },
    Pair {
        open: '_',
        close: '_',
        contexts: &[SyntaxMode::Markup],
    },
    Pair {
        open: '*',
        close: '*',
        contexts: &[SyntaxMode::Markup],
    },
    Pair {
        open: '$',
        close: '$',
        contexts: &[SyntaxMode::Markup, SyntaxMode::Math],
    },
    Pair {
        open: '`',
        close: '`',
        contexts: &[SyntaxMode::Markup],
    },
];

fn find_pair(ch: char) -> Option<&'static Pair> {
    PAIRS.iter().find(|p| p.open == ch || p.close == ch)
}

/// Returns true if the character immediately before `iter` is a backslash.
///
/// This is necessarily a plain text check rather than a parser query: at
/// the moment this matters (deciding whether to auto-pair the character
/// about to be typed), the character that would complete the escape
/// sequence hasn't been typed yet, so there's no `Escape` node in any parse
/// for `is_escape_at` to find.
fn preceded_by_backslash(iter: &gtk::TextIter) -> bool {
    if iter.offset() == 0 {
        return false;
    }
    let mut prev = *iter;
    prev.backward_char();
    prev.char() == '\\'
}

// ── Parser-backed mode/escape queries ────────────────────────────────────────

/// Parse the buffer's current text and return it alongside the byte offset
/// of `iter` within it.
///
/// This is a full O(document length) parse — callers must gate calls to
/// this behind the cheap checks that establish we actually need syntax
/// context (i.e. the character involved is one of the 8 in [`PAIRS`]), not
/// call it unconditionally. See the module doc comment for why a gated
/// fresh parse is preferred here over keeping an incrementally-updated
/// `Source` around.
fn parse_at(buffer: &sourceview::Buffer, iter: &gtk::TextIter) -> (SyntaxNode, usize) {
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    let text = buffer.text(&start, &end, false);

    // Derive the byte offset from the text we already extracted, rather
    // than a second buffer.text() call for a separate prefix — that would
    // copy roughly "cursor position" bytes a second time, which for a
    // cursor near the end of the document is nearly the whole text again.
    // `iter.line_index()` already gives the byte offset *within* the
    // line for free (O(1), GTK-side); only the byte offset of the line's
    // own start needs deriving here, via a plain byte scan for newlines
    // (cheap — no UTF-8 decoding needed, since '\n' is always a single
    // byte and never appears mid-codepoint).
    let target_line = iter.line();
    let mut line_start = 0usize;
    if target_line > 0 {
        let mut lines_seen = 0;
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                lines_seen += 1;
                if lines_seen == target_line {
                    line_start = i + 1;
                    break;
                }
            }
        }
    }
    let byte_offset = line_start + iter.line_index() as usize;

    (parse(text.as_str()), byte_offset)
}

/// Determine the [`SyntaxMode`] in effect at `byte_offset` — e.g. the
/// cursor — by asking what mode holds immediately after the leaf ending
/// there.
fn mode_at(root: &SyntaxNode, byte_offset: usize) -> Option<SyntaxMode> {
    // The very start of a document is always Markup — there's no leaf
    // "before" byte offset 0 to ask, by definition of the grammar.
    if byte_offset == 0 {
        return Some(SyntaxMode::Markup);
    }
    let node = LinkedNode::new(root);
    node.leaf_at(byte_offset, Side::Before)?.mode_after()
}

/// Returns `true` if the character starting at `byte_offset` is part of an
/// escape sequence (e.g. the `_` in `\_`), per this parse.
fn is_escape_at(root: &SyntaxNode, byte_offset: usize) -> bool {
    let node = LinkedNode::new(root);
    match node.leaf_at(byte_offset, Side::After) {
        Some(leaf) => leaf.kind() == SyntaxKind::Escape,
        None => false,
    }
}

// ── Recursion guard ─────────────────────────────────────────────────────────

struct DelimiterState {
    is_handling: bool,
}

impl DelimiterState {
    fn new() -> Self {
        Self { is_handling: false }
    }
}

// ── Paired-backspace detection ───────────────────────────────────────────────

/// If deleting the single character in `[start, end)` should also delete
/// its auto-inserted matching closer, returns the GTK iter just past that
/// closer (so the caller can widen the deletion to cover both).
///
/// The cheap checks happen first and bail out before any parsing, so a
/// parse only ever happens for a deletion that's actually a candidate —
/// i.e. removing exactly one character that's a pair opener. Most
/// deletions (multi-character selections, deleting ordinary text) never
/// reach the parse at all.
fn paired_backspace_end(
    buffer: &sourceview::Buffer,
    start: &gtk::TextIter,
    end: &gtk::TextIter,
) -> Option<gtk::TextIter> {
    if end.offset() - start.offset() != 1 {
        return None;
    }
    // `'\0'` is GTK's end-of-buffer sentinel; treat it as "no character".
    let deleted = start.char();
    if deleted == '\0' {
        return None;
    }
    let pair = find_pair(deleted)?;
    if pair.open != deleted {
        return None;
    }

    let (root, byte_start) = parse_at(buffer, start);

    // Don't treat an escaped delimiter as an opener — it's not a pair we placed.
    if is_escape_at(&root, byte_start) {
        return None;
    }

    // Only double-delete if the opener is in a valid context for this pair,
    // e.g. `"` is only paired in Code/Math — don't double-delete in Markup.
    let mode = mode_at(&root, byte_start)?;
    if !pair.contexts.contains(&mode) {
        return None;
    }

    if end.char() != pair.close {
        return None;
    }

    // For same-character pairs (e.g. `_`), also verify the closer isn't
    // itself an escaped character — we don't want to delete a `_` that the
    // user typed as `\_` just because it happens to follow an opener.
    let mut after_end = *end;
    after_end.forward_char();
    if pair.is_same() {
        let byte_end = byte_start + deleted.len_utf8();
        if is_escape_at(&root, byte_end) {
            return None;
        }
    }

    Some(after_end)
}

// ── Public setup ─────────────────────────────────────────────────────────────

pub fn setup_delimiter_handler<V: IsA<gtk::TextView>>(
    view: &V,
) -> Vec<(glib::Object, glib::SignalHandlerId)> {
    let buffer = match view.as_ref().buffer().downcast::<sourceview::Buffer>() {
        Ok(buf) => buf,
        Err(_) => {
            eprintln!("delimiter: buffer is not a GtkSourceBuffer, delimiter handling disabled");
            return Vec::new();
        }
    };

    let state = Rc::new(RefCell::new(DelimiterState::new()));
    let mut handlers = Vec::new();

    // ── 1. Delete-range handler: paired-backspace only ───────────────────
    {
        let state_ref = Rc::clone(&state);
        let id = buffer.connect_delete_range(move |buf, start, end| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if state_ref.borrow().is_handling {
                    // Re-entrant call from our own buf.delete() below.
                    return;
                }

                let Some(after_end) = paired_backspace_end(buf, start, end) else {
                    return;
                };

                let Ok(mut s) = state_ref.try_borrow_mut() else {
                    return;
                };
                s.is_handling = true;
                drop(s);

                glib::signal::signal_stop_emission_by_name(buf, "delete-range");
                buf.begin_user_action();
                let mut ds = *start;
                let mut de = after_end;
                buf.delete(&mut ds, &mut de);
                buf.end_user_action();

                if let Ok(mut s) = state_ref.try_borrow_mut() {
                    s.is_handling = false;
                }
            }));

            if let Err(e) = result {
                eprintln!("delimiter: panic in delete_range: {:?}", e);
                if let Ok(mut s) = state_ref.try_borrow_mut() {
                    s.is_handling = false;
                }
            }
        });
        handlers.push((buffer.clone().upcast(), id));
    }

    // ── 2. Key-press handler ─────────────────────────────────────────────
    let key_controller = gtk::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    key_controller.connect_key_pressed(glib::clone! {
        #[strong] state,
        #[strong] buffer,
        move |_controller, key, _code, modifier| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if state.borrow().is_handling {
                    return glib::Propagation::Proceed;
                }
                if modifier.intersects(
                    gtk::gdk::ModifierType::CONTROL_MASK
                        | gtk::gdk::ModifierType::ALT_MASK
                        | gtk::gdk::ModifierType::META_MASK,
                ) {
                    return glib::Propagation::Proceed;
                }

                let Some(ch) = key.to_unicode() else {
                    return glib::Propagation::Proceed;
                };
                let Some(pair) = find_pair(ch) else {
                    return glib::Propagation::Proceed;
                };
                if buffer.has_selection() {
                    return glib::Propagation::Proceed;
                }

                let insert_iter = buffer.iter_at_mark(&buffer.get_insert());

                // Skip over an existing closer for same-character pairs (e.g. `"`).
                // No parsing needed for this branch — skipping over an existing
                // character doesn't depend on what mode we're in.
                if pair.is_same() && insert_iter.char() == pair.close {
                    let mut skip_to = insert_iter;
                    skip_to.forward_char();
                    let Ok(mut s) = state.try_borrow_mut() else {
                        return glib::Propagation::Proceed;
                    };
                    s.is_handling = true;
                    drop(s);
                    buffer.begin_user_action();
                    buffer.place_cursor(&skip_to);
                    buffer.end_user_action();
                    if let Ok(mut s) = state.try_borrow_mut() {
                        s.is_handling = false;
                    }
                    return glib::Propagation::Stop;
                }

                // From here on we actually need syntax context — parse now,
                // gated to just this keystroke, and reuse the result for
                // every query needed below.
                let (root, byte_at) = parse_at(&buffer, &insert_iter);

                let Some(mode) = mode_at(&root, byte_at) else {
                    return glib::Propagation::Proceed;
                };
                if !pair.contexts.contains(&mode) {
                    return glib::Propagation::Proceed;
                }

                // Don't auto-pair escaped markup characters (\_, \*, \$, \`).
                // See preceded_by_backslash's doc comment for why this is a
                // plain text check rather than a parser query.
                if pair.is_escapable() && preceded_by_backslash(&insert_iter) {
                    return glib::Propagation::Proceed;
                }

                // Determine action. The same-character skip case was already
                // handled above, so `pair.is_same()` only reaches here for the
                // insert decision.
                let action: Option<char> = if pair.open == ch {
                    if pair.should_close(&insert_iter, byte_at, &root) {
                        Some('i')
                    } else {
                        None
                    }
                } else if pair.close == ch && insert_iter.char() == pair.close {
                    Some('s')
                } else {
                    None
                };

                let Some(action) = action else {
                    return glib::Propagation::Proceed;
                };

                let Ok(mut s) = state.try_borrow_mut() else {
                    return glib::Propagation::Proceed;
                };
                s.is_handling = true;
                drop(s);

                if action == 's' {
                    let mut skip_to = insert_iter;
                    skip_to.forward_char();
                    buffer.begin_user_action();
                    buffer.place_cursor(&skip_to);
                    buffer.end_user_action();
                } else {
                    buffer.begin_user_action();
                    // GTK advances `ins` in place past the inserted text — but
                    // only `ins` itself; any other iterator is invalidated by
                    // this mutation and must not be touched afterward.
                    let mut ins = insert_iter;
                    let mut ch_buf = [0u8; 4];
                    buffer.insert(&mut ins, ch.encode_utf8(&mut ch_buf));

                    // Remember the cursor's target position (between open and
                    // close) as a plain integer offset — offsets survive the
                    // next buffer mutation untouched, unlike iterators.
                    let close_offset = ins.offset();

                    let mut cl_buf = [0u8; 4];
                    buffer.insert(&mut ins, pair.close.encode_utf8(&mut cl_buf));

                    let close_pos = buffer.iter_at_offset(close_offset);
                    buffer.place_cursor(&close_pos);
                    buffer.end_user_action();
                }

                if let Ok(mut s) = state.try_borrow_mut() {
                    s.is_handling = false;
                }
                glib::Propagation::Stop
            }));

            match result {
                Ok(prop) => prop,
                Err(e) => {
                    eprintln!("delimiter: panic in key_pressed: {:?}", e);
                    if let Ok(mut s) = state.try_borrow_mut() {
                        s.is_handling = false;
                    }
                    glib::Propagation::Proceed
                }
            }
        }
    });
    view.as_ref().add_controller(key_controller);

    handlers
}
