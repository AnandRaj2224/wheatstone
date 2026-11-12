// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gtk::glib;
use std::path::{Path, PathBuf};
use tokio::sync::oneshot;

use crate::file_io::dialog;
use crate::window::TypesetterWindow;

pub async fn export_html(
    window: &TypesetterWindow,
    content: String,
    source_path: Option<PathBuf>,
    project_root: Option<PathBuf>,
    export_path: PathBuf,
) -> Result<PathBuf, glib::Error> {
    validate_export_path(&export_path)?;
    let compiler = window.compiler_system().await.map_err(|e| {
        glib::Error::new(
            glib::FileError::Failed,
            &format!("Failed to initialize compiler: {}", e),
        )
    })?;
    let html_bytes = compiler
        .request_html(content, source_path, project_root)
        .await
        .map_err(|e| glib::Error::new(glib::FileError::Failed, &e.to_string()))?
        .ok_or_else(|| {
            glib::Error::new(
                glib::FileError::Failed,
                "Check the errors indicated in the preview pane for details",
            )
        })?;
    let export_path_clone = export_path.clone();
    let rt = crate::application::TypesetterApplication::tokio_runtime();
    rt.spawn(async move { tokio::fs::write(&export_path_clone, html_bytes.as_ref()).await })
        .await
        .map_err(|e| {
            glib::Error::new(
                glib::FileError::Failed,
                &format!("Write task panicked: {}", e),
            )
        })?
        .map_err(|e| {
            glib::Error::new(
                glib::FileError::Failed,
                &format!(
                    "Failed to write HTML file to {}: {}",
                    export_path.display(),
                    e
                ),
            )
        })?;
    Ok(export_path)
}

pub async fn export_pdf(
    window: &TypesetterWindow,
    content: String,
    source_path: Option<PathBuf>,
    project_root: Option<PathBuf>,
    export_path: PathBuf,
    pdf_standard: String,
) -> Result<PathBuf, glib::Error> {
    validate_export_path(&export_path)?;
    let compiler = window.compiler_system().await.map_err(|e| {
        glib::Error::new(
            glib::FileError::Failed,
            &format!("Failed to initialize compiler: {}", e),
        )
    })?;
    let pdf_bytes = compiler
        .request_pdf(content, source_path, project_root, pdf_standard)
        .await
        .map_err(|e| glib::Error::new(glib::FileError::Failed, &e.to_string()))?
        .ok_or_else(|| {
            glib::Error::new(
                glib::FileError::Failed,
                "Check the errors indicated in the preview pane for details",
            )
        })?;
    let export_path_clone = export_path.clone();
    let rt = crate::application::TypesetterApplication::tokio_runtime();
    rt.spawn(async move { tokio::fs::write(&export_path_clone, pdf_bytes.as_ref()).await })
        .await
        .map_err(|e| {
            glib::Error::new(
                glib::FileError::Failed,
                &format!("Write task panicked: {}", e),
            )
        })?
        .map_err(|e| {
            glib::Error::new(
                glib::FileError::Failed,
                &format!(
                    "Failed to write PDF file to {}: {}",
                    export_path.display(),
                    e
                ),
            )
        })?;
    Ok(export_path)
}

pub async fn export_markdown(
    window: &TypesetterWindow,
    content: String,
    source_path: Option<PathBuf>,
    project_root: Option<PathBuf>,
    export_path: PathBuf,
) -> Result<PathBuf, glib::Error> {
    validate_export_path(&export_path)?;
    let compiler = window.compiler_system().await.map_err(|e| {
        glib::Error::new(
            glib::FileError::Failed,
            &format!("Failed to initialize compiler: {}", e),
        )
    })?;
    let markdown = compiler
        .request_markdown(content, source_path, project_root)
        .await
        .map_err(|e| glib::Error::new(glib::FileError::Failed, &e.to_string()))?
        .ok_or_else(|| {
            glib::Error::new(
                glib::FileError::Failed,
                "Check the errors indicated in the preview pane for details",
            )
        })?;
    let export_path_clone = export_path.clone();
    let rt = crate::application::TypesetterApplication::tokio_runtime();
    rt.spawn(async move { tokio::fs::write(&export_path_clone, markdown.as_bytes()).await })
        .await
        .map_err(|e| {
            glib::Error::new(
                glib::FileError::Failed,
                &format!("Write task panicked: {}", e),
            )
        })?
        .map_err(|e| {
            glib::Error::new(
                glib::FileError::Failed,
                &format!(
                    "Failed to write Markdown file to {}: {}",
                    export_path.display(),
                    e
                ),
            )
        })?;
    Ok(export_path)
}

fn validate_export_path(path: &Path) -> Result<(), glib::Error> {
    if !path.is_absolute() {
        return Err(glib::Error::new(
            glib::FileError::Failed,
            "Export path must be absolute",
        ));
    }
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        return Err(glib::Error::new(
            glib::FileError::Noent,
            &format!("Parent directory does not exist: {}", parent.display()),
        ));
    }
    Ok(())
}

/// Async wrapper around `create_file_export_html_dialog` that returns the chosen path.
pub async fn prompt_export_html_path(
    window: &TypesetterWindow,
    current_file_path: Option<PathBuf>,
) -> Option<PathBuf> {
    let suggested_name = get_file_stem_or_default(current_file_path.as_ref());
    let (tx, rx) = oneshot::channel::<Option<PathBuf>>();
    dialog::create_file_export_html_dialog(window, suggested_name, move |path| {
        let _ = tx.send(path);
    });
    rx.await.ok().flatten()
}

/// Async wrapper around `create_file_export_pdf_dialog` that returns the chosen path.
pub async fn prompt_export_pdf_path(
    window: &TypesetterWindow,
    current_file_path: Option<PathBuf>,
) -> Option<PathBuf> {
    let suggested_name = get_file_stem_or_default(current_file_path.as_ref());
    let (tx, rx) = oneshot::channel::<Option<PathBuf>>();
    dialog::create_file_export_pdf_dialog(window, suggested_name, move |path| {
        let _ = tx.send(path);
    });
    rx.await.ok().flatten()
}

/// Async wrapper around `create_file_export_markdown_dialog` that returns the chosen path.
pub async fn prompt_export_markdown_path(
    window: &TypesetterWindow,
    current_file_path: Option<PathBuf>,
) -> Option<PathBuf> {
    let suggested_name = get_file_stem_or_default(current_file_path.as_ref());
    let (tx, rx) = oneshot::channel::<Option<PathBuf>>();
    dialog::create_file_export_markdown_dialog(window, suggested_name, move |path| {
        let _ = tx.send(path);
    });
    rx.await.ok().flatten()
}

fn get_file_stem_or_default(path: Option<&PathBuf>) -> String {
    path.and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("New File")
        .to_string()
}
