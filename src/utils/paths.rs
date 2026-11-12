// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Utilities for working with file paths in Flatpak environments.
//!
//! When running in a Flatpak sandbox, files are accessed through the document portal.
//! This module provides functions to retrieve real host paths and display them in a
//! user-friendly format.

use adw::prelude::FileExt;
use gtk::gio;
use std::path::Path;

const FILE_ATTRIBUTE_HOST_PATH: &str = "xattr::document-portal.host-path";

/// Get the host filesystem path for a file accessed through the Flatpak document portal.
///
/// When running in a Flatpak sandbox, files are accessed through the document portal,
/// which stores the actual host path in an extended attribute. This function retrieves
/// that path to display the real location to the user.
pub fn get_portal_host_path(file: &gio::File) -> Option<String> {
    let info = file
        .query_info(
            FILE_ATTRIBUTE_HOST_PATH,
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()?;
    let host_path = info.attribute_string(FILE_ATTRIBUTE_HOST_PATH)?;
    let host_path_str = host_path.as_str();
    // Early portal versions added a "\x00" suffix, trim it if present
    let cleaned = if let Some(stripped) = host_path_str.strip_suffix("\\x00") {
        stripped
    } else {
        host_path_str
    };
    Some(collapse_path(cleaned))
}

/// Collapse a path by replacing the home directory with ~
///
/// This provides a more user-friendly display of paths by showing ~/Documents
/// instead of /home/username/Documents.
pub fn collapse_path(path: &str) -> String {
    let home_dir = glib::home_dir();
    let home_str = home_dir.to_string_lossy();

    // Special case: if the path equals home directory, return "~/"
    if path == home_str {
        return "~/".to_string();
    }

    // If path starts with home directory, replace with ~
    if let Some(suffix) = path.strip_prefix(home_str.as_ref()) {
        format!("~{}", suffix)
    } else {
        path.to_string()
    }
}

/// Get the directory path for display, either from the portal or from the regular path.
///
/// This function tries to get the real host path when running in Flatpak, and falls
/// back to using the sandboxed path if that's not available.
pub fn get_display_directory(path: &Path) -> Option<String> {
    // Try to get the host path from the portal
    let file = gio::File::for_path(path);
    if let Some(host_path) = get_portal_host_path(&file) {
        // The host_path is the full file path, we need to get its parent
        if let Some(parent) = Path::new(&host_path).parent() {
            return Some(parent.display().to_string());
        }
    }

    // Fallback: use the regular parent directory and collapse it
    path.parent().and_then(|p| p.to_str()).map(collapse_path)
}

/// Get a display-friendly file name from a path.
///
/// Returns the file name component of the path, or a fallback string if unavailable.
/// This is useful for showing file names in UI elements like window titles.
pub fn file_display_name(path: Option<&std::path::PathBuf>, fallback: &str) -> String {
    path.and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or(fallback)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collapse_path() {
        let home = glib::home_dir();
        let home_str = home.to_string_lossy();

        // Test exact home directory
        assert_eq!(collapse_path(&home_str), "~/");

        // Test subdirectory
        let subdir = format!("{}/Documents/test.txt", home_str);
        assert_eq!(collapse_path(&subdir), "~/Documents/test.txt");

        // Test non-home path
        assert_eq!(collapse_path("/usr/bin"), "/usr/bin");
    }
}
