// Copyright 2026 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use gettextrs::gettext;
use gtk::{gio, prelude::*};
use std::path::Path;

use crate::file_io::packages::PackageInfo;

/// Errors that can occur during package import.
#[derive(Debug)]
pub enum ImportError {
    /// The selected directory does not contain a `typst.toml`.
    NoManifest,
    /// The `typst.toml` could not be read or parsed.
    InvalidManifest(String),
    /// The manifest is missing required fields.
    MissingFields(String),
    /// The package already exists at the destination.
    AlreadyInstalled,
    /// A filesystem error occurred while copying.
    CopyFailed(String),
    /// The archive could not be extracted.
    ExtractionFailed(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::NoManifest => write!(f, "No typst.toml found"),
            ImportError::InvalidManifest(e) => write!(f, "Could not parse typst.toml: {e}"),
            ImportError::MissingFields(e) => {
                write!(f, "typst.toml is missing required fields: {e}")
            }
            ImportError::AlreadyInstalled => write!(f, "This package version is already installed"),
            ImportError::CopyFailed(e) => write!(f, "Failed to copy package: {e}"),
            ImportError::ExtractionFailed(e) => write!(f, "Failed to extract archive: {e}"),
            // ImportError::(e) => write!(f, "Failed to clone repository: {e}"),
        }
    }
}

// ============================================================================
// Public entry points
// ============================================================================

/// Shows a folder picker and imports the selected directory as a package.
/// On success, returns the installed [`PackageInfo`]. On failure, returns an
/// [`ImportError`] describing what went wrong. Returns `None` if the user
/// cancelled.
pub async fn prompt_and_import_folder(
    parent: &impl IsA<gtk::Window>,
) -> Option<Result<PackageInfo, ImportError>> {
    let dialog = gtk::FileDialog::builder()
        .title(gettext("Select Folder"))
        .modal(true)
        .accept_label(gettext("Install"))
        .build();

    let folder = match dialog.select_folder_future(Some(parent)).await {
        Ok(file) => file.path()?,
        Err(_) => return None, // user cancelled
    };

    Some(import_package(&folder))
}

/// Shows a file picker filtered to `.zip` and `.tar.gz` archives, extracts
/// the selected archive to a temporary directory, and imports it as a package.
/// On success, returns the installed [`PackageInfo`]. On failure, returns an
/// [`ImportError`] describing what went wrong. Returns `None` if the user
/// cancelled.
pub async fn prompt_and_import_archive(
    parent: &impl IsA<gtk::Window>,
) -> Option<Result<PackageInfo, ImportError>> {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(&gettext("Archives")));
    filter.add_pattern("*.zip");
    filter.add_pattern("*.tar.gz");
    filter.add_pattern("*.tgz");

    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);

    let dialog = gtk::FileDialog::builder()
        .title(gettext("Select Archive"))
        .modal(true)
        .accept_label(gettext("Install"))
        .filters(&filters)
        .build();

    let file = match dialog.open_future(Some(parent)).await {
        Ok(f) => f.path()?,
        Err(_) => return None, // user cancelled
    };

    Some(extract_and_import(&file))
}

// ============================================================================
// Core import logic
// ============================================================================

/// Validates and copies a package directory into the user packages location.
pub(super) fn import_package(source: &Path) -> Result<PackageInfo, ImportError> {
    // Validate manifest exists
    let toml_path = source.join("typst.toml");
    if !toml_path.exists() {
        return Err(ImportError::NoManifest);
    }

    // Parse manifest
    let contents = std::fs::read_to_string(&toml_path)
        .map_err(|e| ImportError::InvalidManifest(e.to_string()))?;
    let parsed: toml::Value =
        toml::from_str(&contents).map_err(|e| ImportError::InvalidManifest(e.to_string()))?;

    // Extract required fields
    let package = parsed
        .get("package")
        .ok_or_else(|| ImportError::MissingFields("[package] section".to_string()))?;

    let name = package
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ImportError::MissingFields("name".to_string()))?
        .to_string();

    let version = package
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ImportError::MissingFields("version".to_string()))?
        .to_string();

    // Detect namespace by scanning the template entrypoint for the
    // #import "@namespace/name:version" self-import. Falls back to "local"
    // if there is no [template] section, or path/entrypoint fields are absent.
    let namespace = parsed
        .get("template")
        .and_then(|t| {
            let path = t.get("path").and_then(|v| v.as_str())?;
            let ep = t.get("entrypoint").and_then(|v| v.as_str())?;
            Some(source.join(format!("{path}/{ep}")))
        })
        .map(|entrypoint_path| detect_namespace(&entrypoint_path, &name, &version))
        .unwrap_or_else(|| "local".to_string());

    // Determine destination
    let dest = crate::utils::xdg_data_dir()
        .join("packages")
        .join(&namespace)
        .join(&name)
        .join(&version);

    if dest.exists() {
        return Err(ImportError::AlreadyInstalled);
    }

    // Copy the directory, cleaning up on partial failure
    copy_dir(source, &dest).map_err(|e| {
        let _ = std::fs::remove_dir_all(&dest);
        ImportError::CopyFailed(e.to_string())
    })?;

    Ok(PackageInfo {
        name: name.clone(),
        path: name,
        display_name: package
            .get("display-name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        version,
        description: package
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        authors: package
            .get("authors")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default(),
        namespace,
        package_dir: dest,
        repository: package
            .get("repository")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

// ============================================================================
// Archive extraction
// ============================================================================

/// Extracts the archive at `source` to a temporary directory and imports it.
/// The temporary directory is always cleaned up, even on failure.
fn extract_and_import(source: &Path) -> Result<PackageInfo, ImportError> {
    let temp_dir = tempfile::tempdir().map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;

    extract(source, temp_dir.path())?;

    // After extraction, find the actual package root. Archives may be
    // wrapped in a single top-level directory (e.g. `package-1.0.0/`).
    // If the extracted root contains exactly one subdirectory and no
    // typst.toml of its own, treat that subdirectory as the package root.
    let package_root = find_package_root(temp_dir.path())?;

    import_package(&package_root)

    // temp_dir is dropped here, cleaning up regardless of outcome
}

/// Finds the directory containing `typst.toml`, unwrapping a single wrapper
/// directory if present. Returns an error if no `typst.toml` can be located.
pub(super) fn find_package_root(extracted: &Path) -> Result<std::path::PathBuf, ImportError> {
    // If there's a typst.toml directly in the extracted root, we're done.
    if extracted.join("typst.toml").exists() {
        return Ok(extracted.to_path_buf());
    }

    // Otherwise, look for a single subdirectory that contains a typst.toml.
    let mut entries = std::fs::read_dir(extracted)
        .map_err(|e| ImportError::ExtractionFailed(e.to_string()))?
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect::<Vec<_>>();

    if entries.len() == 1 {
        let candidate = entries.remove(0).path();
        if candidate.join("typst.toml").exists() {
            return Ok(candidate);
        }
    }

    Err(ImportError::NoManifest)
}

/// Dispatches to the correct extractor based on the file extension.
fn extract(source: &Path, dest: &Path) -> Result<(), ImportError> {
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_lowercase();

    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(source, dest)
    } else if name.ends_with(".zip") {
        extract_zip(source, dest)
    } else {
        Err(ImportError::ExtractionFailed(
            "Unsupported archive format".to_string(),
        ))
    }
}

fn extract_tar_gz(source: &Path, dest: &Path) -> Result<(), ImportError> {
    let file =
        std::fs::File::open(source).map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    for entry in archive
        .entries()
        .map_err(|e| ImportError::ExtractionFailed(e.to_string()))?
    {
        let mut entry = entry.map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;
        let entry_path = entry
            .path()
            .map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;

        let safe_path = safe_join(dest, entry_path.as_ref())?;

        if let Some(parent) = safe_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;
        }

        if entry.header().entry_type().is_file() {
            entry
                .unpack(&safe_path)
                .map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;
        }
    }

    Ok(())
}

fn extract_zip(source: &Path, dest: &Path) -> Result<(), ImportError> {
    let file =
        std::fs::File::open(source).map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;

        let entry_path = match entry.enclosed_name() {
            Some(p) => p,
            None => {
                return Err(ImportError::ExtractionFailed(
                    "Archive contains unsafe path".to_string(),
                ));
            }
        };

        let safe_path = safe_join(dest, &entry_path)?;

        if entry.is_dir() {
            std::fs::create_dir_all(&safe_path)
                .map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;
        } else {
            if let Some(parent) = safe_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;
            }
            let mut out = std::fs::File::create(&safe_path)
                .map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;
        }
    }

    Ok(())
}

// ============================================================================
// Path utilities
// ============================================================================

/// Attempts to detect the package namespace by scanning the entrypoint file
/// for an import statement referencing the given package name and version.
/// Falls back to `"local"` if no match is found.
fn detect_namespace(entrypoint: &Path, name: &str, version: &str) -> String {
    let Ok(content) = std::fs::read_to_string(entrypoint) else {
        return "local".to_string();
    };

    let needle = format!("/{name}:{version}\"");

    for line in content.lines().take(20) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#import \"@")
            && rest.contains(&needle)
            && let Some(slash) = rest.find('/')
        {
            return rest[..slash].to_string();
        }
    }

    "local".to_string()
}

/// Recursively copies a directory from `src` to `dst`.
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Joins `base` and `relative`, returning an error if the result escapes `base`.
fn safe_join(base: &Path, relative: &Path) -> Result<std::path::PathBuf, ImportError> {
    let joined = base.join(relative);
    let canonical_base = base
        .canonicalize()
        .map_err(|e| ImportError::ExtractionFailed(e.to_string()))?;
    // We can't canonicalize the joined path before it exists, so normalise
    // manually by checking that it starts with the base after resolving `..`.
    let normalized = normalize_path(&joined);
    if normalized.starts_with(&canonical_base) {
        Ok(joined)
    } else {
        Err(ImportError::ExtractionFailed(
            "Archive contains path traversal".to_string(),
        ))
    }
}

/// Lexically normalises a path by resolving `.` and `..` components without
/// hitting the filesystem.
fn normalize_path(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}
