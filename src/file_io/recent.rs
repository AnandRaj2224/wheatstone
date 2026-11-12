// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::FileExt;
use gtk::gio;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::fs;
use tokio::sync::Mutex;

const MAX_RECENT_FILES: usize = 20;
const RECENT_FILES_VERSION: u32 = 3;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct RecentFile {
    pub uri: String,
    pub display_name: String,
    pub display_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_root_uri: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RecentFilesData {
    version: u32,
    files: Vec<RecentFile>,
}

impl Default for RecentFilesData {
    fn default() -> Self {
        Self {
            version: RECENT_FILES_VERSION,
            files: Vec::new(),
        }
    }
}

// ============================================================================
// RecentFile Implementation
// ============================================================================

impl RecentFile {
    /// Create a new RecentFile from a gio::File (preserves portal URI)
    pub fn new(file: &gio::File, project_root: Option<&gio::File>) -> Self {
        let uri = file.uri().to_string();

        let display_name = file
            .basename()
            .and_then(|b| b.to_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "unknown".to_string());

        let display_directory = file
            .path()
            .as_deref()
            .and_then(crate::utils::get_display_directory);

        Self {
            uri,
            display_name,
            display_directory,
            project_root_uri: project_root.map(|f| f.uri().to_string()),
        }
    }

    /// Get the gio::File for this entry
    pub fn gio_file(&self) -> gio::File {
        gio::File::for_uri(&self.uri)
    }

    /// Get the file path as a PathBuf, resolved from the URI
    pub fn path_buf(&self) -> Option<PathBuf> {
        self.gio_file().path()
    }

    /// Get the display name
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Get the display directory, with fallback to the URI
    pub fn display_directory(&self) -> String {
        self.display_directory
            .clone()
            .unwrap_or_else(|| self.uri.clone())
    }

    /// Get the project root as a gio::File
    pub fn project_root_file(&self) -> Option<gio::File> {
        self.project_root_uri.as_deref().map(gio::File::for_uri)
    }

    /// Get the project root as an Option<PathBuf>
    pub fn project_root_path(&self) -> Option<PathBuf> {
        self.project_root_file()?.path()
    }

    /// Check if this file has a project root set
    pub fn has_project_root(&self) -> bool {
        self.project_root_uri.is_some()
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Adds a recent file. Spawns a background task to avoid blocking.
pub fn add_recent_file(file: &gio::File, project_root: Option<&gio::File>) {
    let uri = file.uri().to_string();
    let project_root_uri = project_root.map(|f| f.uri().to_string());
    let rt = crate::application::TypesetterApplication::tokio_runtime();

    rt.spawn(async move {
        if let Err(e) = add_recent_file_impl(&uri, project_root_uri.as_deref()).await {
            eprintln!("Failed to add recent file: {}", e);
        }
    });
}

/// Loads recent files. Blocks on the Tokio runtime since this is called from UI code.
pub fn load_recent_files() -> Vec<RecentFile> {
    let rt = crate::application::TypesetterApplication::tokio_runtime();
    rt.block_on(async {
        load_recent_files_impl()
            .await
            .map(|data| data.files)
            .unwrap_or_else(|e| {
                eprintln!("Failed to load recent files: {}", e);
                Vec::new()
            })
    })
}

/// Saves a list of recent files. Spawns a background task to avoid blocking.
pub fn save_recent_files(files: &[RecentFile]) {
    let files = files.to_vec();
    let rt = crate::application::TypesetterApplication::tokio_runtime();

    rt.spawn(async move {
        if let Err(e) = save_recent_files_impl(&RecentFilesData {
            version: RECENT_FILES_VERSION,
            files,
        })
        .await
        {
            eprintln!("Failed to save recent files: {}", e);
        }
    });
}

// ============================================================================
// Internal Implementation
// ============================================================================

static RECENT_FILES_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn get_recent_files_lock() -> &'static Mutex<()> {
    RECENT_FILES_LOCK.get_or_init(|| Mutex::new(()))
}

fn recent_files_path() -> PathBuf {
    let config_dir = gtk::glib::user_config_dir();
    config_dir.join("recent-files.json")
}

async fn add_recent_file_impl(
    uri: &str,
    project_root_uri: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let _guard = get_recent_files_lock().lock().await;

    let mut data = load_recent_files_impl().await?;

    // Build the candidate entry first so we can compute its display fields
    let file = gio::File::for_uri(uri);
    let candidate_display_name = file
        .basename()
        .and_then(|b| b.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string());
    let candidate_display_dir = file
        .path()
        .as_deref()
        .and_then(crate::utils::get_display_directory);

    // Scan existing entries for:
    //   (a) exact URI match — same sandbox path, straightforward dedup
    //   (b) display-identity match — same real file, different sandbox path
    // In both cases, we want to inherit any stored project_root_uri if the
    // caller didn't supply one, but only when that root still exists on disk.
    let mut preserved_project_root_uri: Option<String> = None;

    data.files.retain(|f| {
        let uri_match = f.uri == uri;
        let display_match = f.display_name == candidate_display_name
            && f.display_directory == candidate_display_dir;

        if uri_match || display_match {
            // Inherit project root only when:
            //   - the caller didn't provide one, AND
            //   - the stored root resolves to a path that still exists
            if project_root_uri.is_none()
                && preserved_project_root_uri.is_none()
                && let Some(ref stored_root_uri) = f.project_root_uri
            {
                let root_exists = gio::File::for_uri(stored_root_uri)
                    .path()
                    .is_some_and(|p| p.exists());
                if root_exists {
                    preserved_project_root_uri = Some(stored_root_uri.clone());
                }
            }
            false
        } else {
            true
        }
    });

    let final_project_root_uri = project_root_uri
        .map(|s| s.to_string())
        .or(preserved_project_root_uri);

    let project_root_file = final_project_root_uri.as_deref().map(gio::File::for_uri);

    data.files
        .insert(0, RecentFile::new(&file, project_root_file.as_ref()));

    data.files.truncate(MAX_RECENT_FILES);

    save_recent_files_impl(&data).await?;
    Ok(())
}

async fn load_recent_files_impl() -> Result<RecentFilesData, Box<dyn std::error::Error>> {
    let path = recent_files_path();
    match fs::read(&path).await {
        Ok(contents) => {
            let mut data: RecentFilesData = match serde_json::from_slice(&contents) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Warning: could not parse recent files, resetting: {}", e);
                    fs::remove_file(&path).await?;
                    return Ok(RecentFilesData::default());
                }
            };

            if data.version != RECENT_FILES_VERSION {
                // Version mismatch, delete and start fresh
                fs::remove_file(&path).await?;
                return Ok(RecentFilesData::default());
            }

            // Deduplicate by display identity (name + directory), keeping the first
            // (most recent) occurrence of each.
            let mut seen = std::collections::HashSet::new();
            data.files
                .retain(|f| seen.insert((f.display_name.clone(), f.display_directory.clone())));

            for file in &data.files {
                let resolved = gio::File::for_uri(&file.uri).path();
                #[cfg(debug_assertions)]
                println!("[recent] uri={:?} resolves={:?}", file.uri, resolved);
                if resolved.is_none() {
                    println!("[recent] uri={:?} did not resolve", file.uri);
                }

                if let Some(ref root_uri) = file.project_root_uri {
                    let root_resolved = gio::File::for_uri(root_uri).path();
                    #[cfg(debug_assertions)]
                    println!(
                        "[recent] project_root_uri={:?} resolves={:?}",
                        root_uri, root_resolved
                    );
                    if root_resolved.is_none() {
                        println!("[recent] project_root_uri={:?} did not resolve", root_uri);
                    }
                }
            }

            Ok(data)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RecentFilesData::default()),
        Err(e) => Err(e.into()),
    }
}

async fn save_recent_files_impl(data: &RecentFilesData) -> Result<(), Box<dyn std::error::Error>> {
    let path = recent_files_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let contents = serde_json::to_vec_pretty(data)?;
    fs::write(&path, contents).await?;
    Ok(())
}
