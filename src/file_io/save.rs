// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::{gio, glib};
use sourceview::{FileSaver, prelude::SourceFileExt};
use std::path::PathBuf;
use tokio::sync::oneshot;

use crate::file_io::dialog;
use crate::window::TypesetterWindow;

/// Async wrapper around `create_file_save_dialog` that returns the chosen file.
pub async fn prompt_save_file(
    window: &TypesetterWindow,
    current_file_path: Option<PathBuf>,
) -> Option<gio::File> {
    let suggested_name = current_file_path
        .as_ref()
        .and_then(|path| path.file_stem())
        .and_then(|stem| stem.to_str())
        .unwrap_or("New File")
        .to_string();

    let (tx, rx) = oneshot::channel::<Option<gio::File>>();
    dialog::create_file_save_dialog(window, suggested_name, move |file| {
        let _ = tx.send(file);
    });
    rx.await.ok().flatten()
}

/// Write buffer contents to a gio::File.
pub async fn write_file(
    buffer: &sourceview::Buffer,
    file: gio::File,
) -> Result<gio::File, glib::Error> {
    let source_file = sourceview::File::new();
    source_file.set_location(Some(&file));

    let saver = FileSaver::new(buffer, &source_file);
    let (tx, rx) = oneshot::channel::<Result<(), glib::Error>>();
    saver.save_async(
        glib::Priority::DEFAULT,
        gio::Cancellable::NONE,
        move |result| {
            let _ = tx.send(result);
        },
    );

    match rx.await.unwrap_or_else(|_| {
        Err(glib::Error::new(
            glib::FileError::Failed,
            "Save operation channel closed",
        ))
    }) {
        Ok(_) => Ok(file),
        Err(e) => Err(e),
    }
}
