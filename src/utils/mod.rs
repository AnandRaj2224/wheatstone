// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Utilities for the Typesetter application.
//!
//! This module provides utilities for:
//! - Converting between UTF-8 byte offsets and character offsets
//! - Working with file paths in Flatpak environments

mod colors;
mod offsets;
mod paths;
mod strings;
mod xdg;

// Re-export public functions
pub use colors::{Matrix, Offset, combined_transform};
pub use offsets::{byte_to_char_offset, char_to_byte_offset};
pub use paths::{file_display_name, get_display_directory};
pub use strings::capitalize_first_letter;
pub use xdg::{bundled_templates_dir, xdg_cache_dir, xdg_data_dir, xdg_state_dir};
