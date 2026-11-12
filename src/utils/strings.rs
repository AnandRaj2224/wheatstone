// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

/// Capitalize the first Unicode character of a string, leaving the rest unchanged.
pub fn capitalize_first_letter(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut result = String::with_capacity(s.len());
            result.extend(first.to_uppercase());
            result.push_str(chars.as_str());
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_string() {
        assert_eq!(capitalize_first_letter(""), "");
    }

    #[test]
    fn test_already_capitalized() {
        assert_eq!(capitalize_first_letter("Hello"), "Hello");
    }

    #[test]
    fn test_lowercase_first() {
        assert_eq!(capitalize_first_letter("hello world"), "Hello world");
    }

    #[test]
    fn test_single_char() {
        assert_eq!(capitalize_first_letter("a"), "A");
        assert_eq!(capitalize_first_letter("A"), "A");
    }

    #[test]
    fn test_unicode_first_char() {
        assert_eq!(capitalize_first_letter("üniform"), "Üniform");
    }

    #[test]
    fn test_non_alpha_first_char() {
        assert_eq!(capitalize_first_letter("123abc"), "123abc");
        assert_eq!(capitalize_first_letter("'quoted'"), "'quoted'");
    }
}
