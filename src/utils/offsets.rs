// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Utilities for converting between UTF-8 byte offsets and character offsets.
//!
//! Typst uses byte offsets (positions in the UTF-8 byte stream), while GTK uses
//! character offsets (positions counting Unicode scalar values). This module provides
//! conversion functions between the two representations.

/// Convert a byte offset to a character offset in UTF-8 text.
///
/// Returns the character position where a cursor should be placed for the given byte offset.
/// If the byte offset is in the middle of a multi-byte character, returns the position
/// at the start of that character.
///
/// # Examples
///
/// ```
/// use typesetter::utils;
///
/// let text = "hello 世界";
/// assert_eq!(utils::byte_to_char_offset(text, 6), 6);  // After "hello "
/// assert_eq!(utils::byte_to_char_offset(text, 9), 7);  // After "世"
/// assert_eq!(utils::byte_to_char_offset(text, 7), 6);  // Mid "世" -> start of "世"
/// ```
pub fn byte_to_char_offset(text: &str, byte_offset: usize) -> usize {
    text.char_indices()
        .take_while(|(pos, ch)| pos + ch.len_utf8() <= byte_offset)
        .count()
}

/// Convert a character offset to a byte offset in UTF-8 text.
///
/// Returns the byte position in the UTF-8 string corresponding to the given
/// character position.
///
/// # Examples
///
/// ```
/// use typesetter::utils;
///
/// let text = "hello 世界";
/// assert_eq!(utils::char_to_byte_offset(text, 6), 6);  // After "hello "
/// assert_eq!(utils::char_to_byte_offset(text, 7), 9);  // After "世"
/// ```
#[allow(dead_code)]
pub fn char_to_byte_offset(text: &str, char_offset: usize) -> usize {
    text.chars().take(char_offset).map(|c| c.len_utf8()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_to_char_conversion_ascii() {
        let text = "hello world";

        // ASCII: byte offset == character offset
        assert_eq!(byte_to_char_offset(text, 0), 0);
        assert_eq!(byte_to_char_offset(text, 5), 5);
        assert_eq!(byte_to_char_offset(text, 11), 11);

        // Round trip
        let char_offset = 7;
        let byte_offset = char_to_byte_offset(text, char_offset);
        assert_eq!(byte_offset, 7);
        assert_eq!(byte_to_char_offset(text, byte_offset), char_offset);
    }

    #[test]
    fn test_byte_to_char_conversion_unicode() {
        let text = "hello 世界";

        assert_eq!(byte_to_char_offset(text, 0), 0);
        assert_eq!(byte_to_char_offset(text, 6), 6);
        assert_eq!(byte_to_char_offset(text, 9), 7);
        assert_eq!(byte_to_char_offset(text, 12), 8);

        // Round trip
        let char_offset = 7;
        let byte_offset = char_to_byte_offset(text, char_offset);
        assert_eq!(byte_offset, 9);
        assert_eq!(byte_to_char_offset(text, byte_offset), char_offset);
    }

    #[test]
    fn test_byte_to_char_conversion_emoji() {
        let text = "hello 👋";

        assert_eq!(byte_to_char_offset(text, 6), 6);
        assert_eq!(byte_to_char_offset(text, 10), 7);
    }

    #[test]
    fn test_byte_offset_at_char_boundary() {
        let text = "a世b";

        assert_eq!(byte_to_char_offset(text, 0), 0);
        assert_eq!(byte_to_char_offset(text, 1), 1);

        // Mid-way through "世" should return start of character
        assert_eq!(byte_to_char_offset(text, 2), 1);
        assert_eq!(byte_to_char_offset(text, 3), 1);

        assert_eq!(byte_to_char_offset(text, 4), 2);
    }

    #[test]
    fn test_char_to_byte_conversion() {
        let text = "hello 世界";

        assert_eq!(char_to_byte_offset(text, 0), 0);
        assert_eq!(char_to_byte_offset(text, 6), 6);
        assert_eq!(char_to_byte_offset(text, 7), 9);
        assert_eq!(char_to_byte_offset(text, 8), 12);
    }

    #[test]
    fn test_mixed_unicode_content() {
        let text = "Hi 👋 世界!";

        let char_offsets = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let byte_offsets = vec![0, 1, 2, 3, 7, 8, 11, 14, 15];

        for (&char_idx, &expected_byte) in char_offsets.iter().zip(byte_offsets.iter()) {
            assert_eq!(char_to_byte_offset(text, char_idx), expected_byte);
        }

        for (&byte_idx, &char_idx) in byte_offsets.iter().zip(char_offsets.iter()) {
            assert_eq!(byte_to_char_offset(text, byte_idx), char_idx);
        }
    }

    #[test]
    fn test_empty_string() {
        let text = "";
        assert_eq!(byte_to_char_offset(text, 0), 0);
        assert_eq!(char_to_byte_offset(text, 0), 0);
    }

    #[test]
    fn test_offset_at_end() {
        let text = "hello";
        assert_eq!(byte_to_char_offset(text, 5), 5);
        assert_eq!(char_to_byte_offset(text, 5), 5);
    }
}
