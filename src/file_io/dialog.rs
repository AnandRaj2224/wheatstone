// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gettextrs::gettext;
use gtk::{gio, prelude::*};
use std::path::PathBuf;

use crate::window::TypesetterWindow;

enum FileDialogType {
    Open,
    Save,
}

/// Generic file dialog creator.
fn create_file_dialog<F>(
    window: &TypesetterWindow,
    dialog_type: FileDialogType,
    title: &str,
    initial_name: Option<&str>,
    filter_fn: fn() -> gio::ListStore,
    callback: F,
) where
    F: FnOnce(Option<gio::File>) + 'static,
{
    let dialog = gtk::FileDialog::new();
    dialog.set_title(title);
    if let Some(name) = initial_name {
        dialog.set_initial_name(Some(name));
    }
    dialog.set_filters(Some(&filter_fn()));
    let parent_window = Some(window);
    let cancellable: Option<&gio::Cancellable> = None;
    let handler = move |result: Result<gio::File, glib::Error>| {
        callback(result.ok());
    };
    match dialog_type {
        FileDialogType::Open => dialog.open(parent_window, cancellable, handler),
        FileDialogType::Save => dialog.save(parent_window, cancellable, handler),
    }
}

/// Shows the "Export HTML" dialog. Callback receives `Option<PathBuf>` since
/// export destinations are always real filesystem paths, not portal-issued files.
pub fn create_file_export_html_dialog<F>(
    window: &TypesetterWindow,
    suggested_name: String,
    callback: F,
) where
    F: FnOnce(Option<PathBuf>) + 'static,
{
    create_file_dialog(
        window,
        FileDialogType::Save,
        &gettext("Export File"),
        Some(format!("{}.html", suggested_name).as_str()),
        create_file_filter_html,
        move |file| callback(file.and_then(|f| f.path())),
    );
}

/// Shows the "Export PDF" dialog. Callback receives `Option<PathBuf>` since
/// export destinations are always real filesystem paths, not portal-issued files.
pub fn create_file_export_pdf_dialog<F>(
    window: &TypesetterWindow,
    suggested_name: String,
    callback: F,
) where
    F: FnOnce(Option<PathBuf>) + 'static,
{
    create_file_dialog(
        window,
        FileDialogType::Save,
        &gettext("Export File"),
        Some(format!("{}.pdf", suggested_name).as_str()),
        create_file_filter_pdf,
        move |file| callback(file.and_then(|f| f.path())),
    );
}

/// Shows the "Export Markdown" dialog. Callback receives `Option<PathBuf>` since
/// export destinations are always real filesystem paths, not portal-issued files.
pub fn create_file_export_markdown_dialog<F>(
    window: &TypesetterWindow,
    suggested_name: String,
    callback: F,
) where
    F: FnOnce(Option<PathBuf>) + 'static,
{
    create_file_dialog(
        window,
        FileDialogType::Save,
        &gettext("Export File"),
        Some(format!("{}.md", suggested_name).as_str()),
        create_file_filter_markdown,
        move |file| callback(file.and_then(|f| f.path())),
    );
}

/// Shows the "Open File" dialog.
pub fn create_file_open_dialog<F>(window: &TypesetterWindow, callback: F)
where
    F: FnOnce(Option<gio::File>) + 'static,
{
    create_file_dialog(
        window,
        FileDialogType::Open,
        &gettext("Open File"),
        None,
        create_file_filter_typst,
        callback,
    );
}

/// Shows the "Save File" dialog.
pub fn create_file_save_dialog<F>(window: &TypesetterWindow, suggested_name: String, callback: F)
where
    F: FnOnce(Option<gio::File>) + 'static,
{
    create_file_dialog(
        window,
        FileDialogType::Save,
        &gettext("Save File"),
        Some(format!("{}.typ", suggested_name).as_str()),
        create_file_filter_typst,
        callback,
    );
}

/// Creates a GTK file filter.
fn create_file_filter(name: &str, pattern: &str) -> gtk::FileFilter {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(name));
    filter.add_pattern(pattern);
    filter
}

/// Creates a list of GTK file filters.
fn create_file_filter_list(filters: &[(&str, &str)]) -> gio::ListStore {
    let store = gio::ListStore::new::<gtk::FileFilter>();
    for (name, pattern) in filters {
        store.append(&create_file_filter(name, pattern));
    }
    store
}

fn create_file_filter_html() -> gio::ListStore {
    create_file_filter_list(&[("HTML", "*.html"), (&gettext("All Files"), "*")])
}

fn create_file_filter_pdf() -> gio::ListStore {
    create_file_filter_list(&[("PDF", "*.pdf"), (&gettext("All Files"), "*")])
}

fn create_file_filter_markdown() -> gio::ListStore {
    create_file_filter_list(&[("Markdown", "*.md"), (&gettext("All Files"), "*")])
}

fn create_file_filter_typst() -> gio::ListStore {
    create_file_filter_list(&[("Typst", "*.typ"), (&gettext("All Files"), "*")])
}
