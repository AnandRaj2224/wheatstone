// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    fs,
    path::{Path, PathBuf},
};
use typst::{
    diag::{FileError, FileResult},
    foundations::Bytes,
    syntax::{FileId, Source, VirtualRoot},
};
use typst_kit::packages::SystemPackages;

/// Represents a single file slot in the world — caches file contents in
/// memory, tracks access, and routes package file requests to the appropriate
/// resolver (bundled templates, git cache, or Typst Universe via
/// PackageStorage).
#[derive(Clone, Debug)]
pub struct FileSlot {
    id: FileId,
    cached_source: Option<Source>,
    cached_bytes: Option<Bytes>,
    accessed: bool,
}

impl FileSlot {
    /// Create a new slot for the given file ID.
    pub fn new(id: FileId) -> Self {
        Self {
            id,
            cached_source: None,
            cached_bytes: None,
            accessed: false,
        }
    }

    /// Mark this file as accessed (called when file is read)
    pub fn mark_accessed(&mut self) {
        self.accessed = true;
    }

    /// Mark this file as not accessed (called at start of each compilation)
    pub fn mark_not_accessed(&mut self) {
        self.accessed = false;
    }

    /// Returns `true` if this file was accessed since the last reset.
    pub fn was_accessed(&self) -> bool {
        self.accessed
    }

    /// Returns the resolved filesystem path if this file was accessed and is
    /// a local file (not a package).
    pub fn accessed_local_path(&self, project_root: &Path) -> Option<PathBuf> {
        if !self.was_accessed() {
            return None;
        }
        if matches!(self.id.root(), VirtualRoot::Package(_)) {
            return None;
        }
        self.id.vpath().realize(project_root).ok()
    }

    /// Get the parsed Typst source for this file, loading it if necessary.
    pub fn source(&mut self, project_root: &Path, packages: &SystemPackages) -> FileResult<Source> {
        self.accessed = true;
        if let Some(source) = &self.cached_source {
            return Ok(source.clone());
        }
        let bytes = self.read_file(project_root, packages)?;
        let text = Self::decode_utf8(&bytes)?;
        let source = Source::new(self.id, text.into());
        self.cached_source = Some(source.clone());
        Ok(source)
    }

    /// Get the raw file bytes, loading them if necessary.
    pub fn bytes(&mut self, project_root: &Path, packages: &SystemPackages) -> FileResult<Bytes> {
        self.accessed = true;
        if let Some(bytes) = &self.cached_bytes {
            return Ok(bytes.clone());
        }
        let content = self.read_file(project_root, packages)?;
        let bytes = Bytes::new(content);
        self.cached_bytes = Some(bytes.clone());
        Ok(bytes)
    }

    /// Clear only the bytes cache to free memory while preserving parsed source.
    pub fn clear_bytes_cache(&mut self) {
        self.cached_bytes = None;
    }

    /// Get the approximate size of cached data in bytes.
    pub fn cached_size(&self) -> usize {
        let source_size = self
            .cached_source
            .as_ref()
            .map(|s| s.text().len())
            .unwrap_or(0);
        let bytes_size = self.cached_bytes.as_ref().map(|b| b.len()).unwrap_or(0);
        source_size + bytes_size
    }

    /// Check if source is cached.
    pub fn has_source(&self) -> bool {
        self.cached_source.is_some()
    }

    /// Check if bytes are cached.
    pub fn has_bytes(&self) -> bool {
        self.cached_bytes.is_some()
    }

    /// Strips a UTF-8 BOM if present and decodes the bytes as UTF-8.
    fn decode_utf8(buf: &[u8]) -> FileResult<&str> {
        let without_bom = buf.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(buf);
        Ok(std::str::from_utf8(without_bom)?)
    }

    /// Reads the file bytes from either a project root or a package.
    fn read_file(&self, project_root: &Path, packages: &SystemPackages) -> FileResult<Vec<u8>> {
        let path = self.resolve_path(project_root, packages)?;
        let meta = fs::metadata(&path).map_err(|e| FileError::from_io(e, &path))?;
        if meta.is_dir() {
            return Err(FileError::IsDirectory);
        }
        fs::read(&path).map_err(|e| FileError::from_io(e, &path))
    }

    /// Resolves the full path to this file based on its FileId.
    ///
    /// Package files are resolved by checking locations in order:
    /// 1. The bundled templates directory
    /// 2. The user packages directory via [`SystemPackages`]
    ///
    /// See: <https://github.com/typst/typst/issues/8183>
    fn resolve_path(&self, project_root: &Path, packages: &SystemPackages) -> FileResult<PathBuf> {
        let base = match self.id.root() {
            VirtualRoot::Package(spec) => {
                let bundled = crate::utils::bundled_templates_dir()
                    .join(spec.namespace.as_str())
                    .join(spec.name.as_str())
                    .join(spec.version.to_string());

                if bundled.is_dir() {
                    bundled
                } else {
                    packages.obtain(spec)?.path().to_path_buf()
                }
            }
            VirtualRoot::Project => project_root.to_path_buf(),
        };
        self.id
            .vpath()
            .realize(&base)
            .map_err(|_| FileError::AccessDenied)
    }
}
