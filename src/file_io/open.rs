// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::{gio, glib, prelude::*};
use sourceview::{FileLoader, prelude::SourceFileExt};
use tokio::sync::oneshot;

use crate::file_io::dialog;
use crate::window::TypesetterWindow;

/// Find an existing window that has the given file open, comparing by URI.
pub fn find_window_with_file(app: &gtk::Application, file: &gio::File) -> Option<TypesetterWindow> {
    for window in app.windows() {
        if let Ok(typesetter_window) = window.downcast::<TypesetterWindow>()
            && let Some(current_path) = typesetter_window.current_file_path()
        {
            let current_file = gio::File::for_path(&current_path);
            if current_file.equal(file) || current_file.uri() == file.uri() {
                return Some(typesetter_window);
            }
        }
    }
    None
}

/// Async wrapper around `create_file_open_dialog` that returns the chosen file.
pub async fn prompt_open_file(window: &TypesetterWindow) -> Option<gio::File> {
    let (tx, rx) = oneshot::channel::<Option<gio::File>>();
    dialog::create_file_open_dialog(window, move |file| {
        let _ = tx.send(file);
    });
    rx.await.ok().flatten()
}

/// Async load file contents into the editor buffer via URI.
pub async fn load_file(buffer: &sourceview::Buffer, file: &gio::File) -> Result<(), glib::Error> {
    #[cfg(debug_assertions)]
    println!(
        "[load_file] uri={:?} path={:?} exists={:?}",
        file.uri(),
        file.path(),
        file.path().map(|p| p.exists())
    );

    let source_file = sourceview::File::new();
    source_file.set_location(Some(file));

    let loader = FileLoader::new(buffer, &source_file);
    let (tx, rx) = oneshot::channel::<Result<(), glib::Error>>();
    loader.load_async(
        glib::Priority::DEFAULT,
        gio::Cancellable::NONE,
        move |result| {
            let _ = tx.send(result);
        },
    );

    rx.await
        .unwrap_or_else(|_| Err(glib::Error::new(glib::FileError::Failed, "Channel closed")))
}
