// Copyright 2026 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;

/// Returns the XDG data directory for this application, falling back to
/// `~/.local/share/dev.whetstone.editor` if `XDG_DATA_HOME` is not set.
/// On Flatpak, `XDG_DATA_HOME` is already scoped to the app.
pub fn xdg_data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".local/share")
                .join(crate::config::APP_ID)
        })
}

/// Returns the XDG cache directory for this application, falling back to
/// `~/.cache/dev.whetstone.editor` if `XDG_CACHE_HOME` is not set.
/// On Flatpak, `XDG_CACHE_HOME` is already scoped to the app.
pub fn xdg_cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".cache")
                .join(crate::config::APP_ID)
        })
}

/// Returns the XDG state directory for this application, falling back to
/// `~/.local/state/dev.whetstone.editor` if `XDG_STATE_HOME` is not set.
/// On Flatpak, `XDG_STATE_HOME` is already scoped to the app.
pub fn xdg_state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".local/state")
                .join(crate::config::APP_ID)
        })
}

/// Returns the path to the bundled templates directory shipped with the
/// application. On Flatpak this resolves to `/app/share/typesetter/templates`.
/// These templates are read-only; user-installed templates live under
/// [`xdg_data_dir`] instead.
pub fn bundled_templates_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(crate::config::PKGDATADIR).join("templates")
}
