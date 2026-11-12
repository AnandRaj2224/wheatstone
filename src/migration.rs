// Copyright 2026 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::{Path, PathBuf};
use gtk::gio;
use gtk::prelude::*;

pub const OLD_APP_ID: &str = "net.trowell.typesetter";
pub const CACHE_SENTINEL_NAME: &str = ".typesetter_cache_migrated";
pub const DATA_SENTINEL_NAME: &str = ".typesetter_data_migrated";

/// Result of an individual migration step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    /// Files were migrated from legacy directory.
    Migrated,
    /// Already migrated previously (sentinel exists); no action taken.
    AlreadyMigrated,
    /// Nothing to migrate (legacy source directory does not exist).
    SkippedNoSource,
}

/// Recursively copies contents of `src` to `dst` without overwriting existing files.
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !src.exists() {
        return Ok(());
    }
    if !dst.exists() {
        std::fs::create_dir_all(dst)?;
    }

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let target_path = dst.join(entry.file_name());

        if entry_path.is_dir() {
            copy_dir_recursive(&entry_path, &target_path)?;
        } else if entry_path.is_file() && !target_path.exists() {
            std::fs::copy(&entry_path, &target_path)?;
        }
    }
    Ok(())
}

pub fn old_xdg_cache_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".cache")
        .join(OLD_APP_ID)
}

pub fn old_xdg_data_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".local/share")
        .join(OLD_APP_ID)
}

/// Paths for migration operations.
#[derive(Debug, Clone)]
pub struct MigrationPaths {
    pub old_cache_dir: PathBuf,
    pub new_cache_dir: PathBuf,
    pub old_data_dir: PathBuf,
    pub new_data_dir: PathBuf,
    pub state_dir: PathBuf,
}

impl MigrationPaths {
    pub fn default_xdg() -> Self {
        Self {
            old_cache_dir: old_xdg_cache_dir(),
            new_cache_dir: crate::utils::xdg_cache_dir(),
            old_data_dir: old_xdg_data_dir(),
            new_data_dir: crate::utils::xdg_data_dir(),
            state_dir: crate::utils::xdg_state_dir(),
        }
    }
}

/// Migrates cache packages from `old_cache_dir/packages` to `new_cache_dir/packages` exactly once.
/// The completion sentinel is stored in persistent application state (`state_dir`),
/// decoupled from the cache directory so cache clearing never triggers re-migration.
pub fn migrate_cache_packages(
    old_cache_dir: &Path,
    new_cache_dir: &Path,
    state_dir: &Path,
) -> std::io::Result<StepResult> {
    let sentinel = state_dir.join(CACHE_SENTINEL_NAME);
    let legacy_cache_sentinel = new_cache_dir.join(CACHE_SENTINEL_NAME);

    // If already marked in state_dir, or previously marked in new_cache_dir:
    if sentinel.exists() || legacy_cache_sentinel.exists() {
        if !sentinel.exists() {
            mark_sentinel_if_missing(&sentinel);
        }
        return Ok(StepResult::AlreadyMigrated);
    }

    let old_packages = old_cache_dir.join("packages");
    let new_packages = new_cache_dir.join("packages");

    if !old_packages.exists() {
        mark_sentinel_if_missing(&sentinel);
        return Ok(StepResult::SkippedNoSource);
    }

    copy_dir_recursive(&old_packages, &new_packages)?;

    mark_sentinel_if_missing(&sentinel);
    Ok(StepResult::Migrated)
}

/// Migrates user data from `old_data_dir` to `new_data_dir` exactly once.
/// The completion sentinel is stored in persistent application state (`state_dir`),
/// decoupled from the data directory.
pub fn migrate_user_data(
    old_data_dir: &Path,
    new_data_dir: &Path,
    state_dir: &Path,
) -> std::io::Result<StepResult> {
    let sentinel = state_dir.join(DATA_SENTINEL_NAME);
    let legacy_data_sentinel = new_data_dir.join(DATA_SENTINEL_NAME);

    if sentinel.exists() || legacy_data_sentinel.exists() {
        if !sentinel.exists() {
            mark_sentinel_if_missing(&sentinel);
        }
        return Ok(StepResult::AlreadyMigrated);
    }

    if !old_data_dir.exists() {
        mark_sentinel_if_missing(&sentinel);
        return Ok(StepResult::SkippedNoSource);
    }

    copy_dir_recursive(old_data_dir, new_data_dir)?;

    mark_sentinel_if_missing(&sentinel);
    Ok(StepResult::Migrated)
}

fn mark_sentinel_if_missing(sentinel_path: &Path) {
    if !sentinel_path.exists() {
        if let Some(parent) = sentinel_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(sentinel_path, b"");
    }
}

/// Migrates settings and cached/user data from `net.trowell.typesetter` to `dev.whetstone.editor`
/// if this is the first run and the old settings or directories exist.
pub fn migrate_from_typesetter_if_needed() {
    let paths = MigrationPaths::default_xdg();
    migrate_with_paths(&paths);
}

/// Core migration runner parameterized by paths.
pub fn migrate_with_paths(paths: &MigrationPaths) {
    let schema_source = gio::SettingsSchemaSource::default();

    let new_schema_installed = schema_source
        .as_ref()
        .and_then(|source| source.lookup(crate::config::APP_ID, true))
        .is_some();

    // Check if an existing installation has already completed migration
    if new_schema_installed {
        let new_settings = gio::Settings::new(crate::config::APP_ID);
        if new_settings.boolean("preferences-migrated") {
            // Already migrated previously. Ensure sentinels exist in state_dir so legacy
            // directories are never touched, and return immediately.
            mark_sentinel_if_missing(&paths.state_dir.join(CACHE_SENTINEL_NAME));
            mark_sentinel_if_missing(&paths.state_dir.join(DATA_SENTINEL_NAME));
            return;
        }
    }

    // 1. Migrate cache packages
    let cache_ok = match migrate_cache_packages(&paths.old_cache_dir, &paths.new_cache_dir, &paths.state_dir) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("whetstone: legacy cache migration failed: {e}");
            false
        }
    };

    // 2. Migrate user data
    let data_ok = match migrate_user_data(&paths.old_data_dir, &paths.new_data_dir, &paths.state_dir) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("whetstone: legacy user data migration failed: {e}");
            false
        }
    };

    // 3. Migrate preferences if new schema is installed
    if new_schema_installed {
        let new_settings = gio::Settings::new(crate::config::APP_ID);

        let old_schema_exists = schema_source
            .as_ref()
            .and_then(|source| source.lookup(OLD_APP_ID, true))
            .is_some();

        if old_schema_exists {
            let old_settings = gio::Settings::new(OLD_APP_ID);
            let new_schema = new_settings.settings_schema().unwrap();
            let old_schema = old_settings.settings_schema().unwrap();

            for key in new_schema.list_keys() {
                let key_str = key.as_str();
                if key_str == "preferences-migrated" {
                    continue;
                }
                if old_schema.has_key(key_str) {
                    let val = old_settings.value(key_str);
                    let _ = new_settings.set_value(key_str, &val);
                }
            }
        }

        // Only mark migration complete if required filesystem operations succeeded!
        if cache_ok && data_ok {
            let _ = new_settings.set_boolean("preferences-migrated", true);
            gio::Settings::sync();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_copy_dir_recursive() {
        let temp_dir = tempfile::tempdir().unwrap();
        let src = temp_dir.path().join("src");
        let dst = temp_dir.path().join("dst");

        fs::create_dir_all(src.join("subdir")).unwrap();
        fs::write(src.join("file1.txt"), "hello").unwrap();
        fs::write(src.join("subdir/file2.txt"), "world").unwrap();

        copy_dir_recursive(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("file1.txt")).unwrap(), "hello");
        assert_eq!(fs::read_to_string(dst.join("subdir/file2.txt")).unwrap(), "world");

        // Test non-destructive: existing file in dst is preserved
        fs::write(dst.join("file1.txt"), "preserved").unwrap();
        fs::write(src.join("file1.txt"), "overwritten?").unwrap();
        copy_dir_recursive(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(dst.join("file1.txt")).unwrap(), "preserved");
    }

    #[test]
    fn test_initial_migration_copies_legacy_packages() {
        let temp = tempfile::tempdir().unwrap();
        let old_cache = temp.path().join("old_cache");
        let new_cache = temp.path().join("new_cache");
        let state_dir = temp.path().join("state");

        let old_pkg_032 = old_cache.join("packages/preview/cetz/0.3.2");
        let old_pkg_034 = old_cache.join("packages/preview/cetz/0.3.4");
        fs::create_dir_all(&old_pkg_032).unwrap();
        fs::create_dir_all(&old_pkg_034).unwrap();
        fs::write(old_pkg_032.join("typst.toml"), "version = 0.3.2").unwrap();
        fs::write(old_pkg_034.join("typst.toml"), "version = 0.3.4").unwrap();

        let res = migrate_cache_packages(&old_cache, &new_cache, &state_dir).unwrap();
        assert_eq!(res, StepResult::Migrated);
        assert!(new_cache.join("packages/preview/cetz/0.3.2/typst.toml").exists());
        assert!(new_cache.join("packages/preview/cetz/0.3.4/typst.toml").exists());
        // Sentinel lives in state_dir, not new_cache
        assert!(state_dir.join(CACHE_SENTINEL_NAME).exists());
        assert!(!new_cache.join(CACHE_SENTINEL_NAME).exists());
    }

    #[test]
    fn test_deletion_persistence_and_no_restoration() {
        let temp = tempfile::tempdir().unwrap();
        let old_cache = temp.path().join("old_cache");
        let new_cache = temp.path().join("new_cache");
        let state_dir = temp.path().join("state");

        let old_pkg_032 = old_cache.join("packages/preview/cetz/0.3.2");
        let old_pkg_034 = old_cache.join("packages/preview/cetz/0.3.4");
        fs::create_dir_all(&old_pkg_032).unwrap();
        fs::create_dir_all(&old_pkg_034).unwrap();
        fs::write(old_pkg_032.join("typst.toml"), "version = 0.3.2").unwrap();
        fs::write(old_pkg_034.join("typst.toml"), "version = 0.3.4").unwrap();

        // 1. Initial migration
        let res1 = migrate_cache_packages(&old_cache, &new_cache, &state_dir).unwrap();
        assert_eq!(res1, StepResult::Migrated);

        // 2. User deletes cetz 0.3.2 from Whetstone cache
        fs::remove_dir_all(new_cache.join("packages/preview/cetz/0.3.2")).unwrap();
        assert!(!new_cache.join("packages/preview/cetz/0.3.2").exists());

        // 3. Application restart triggers migration again
        let res2 = migrate_cache_packages(&old_cache, &new_cache, &state_dir).unwrap();
        assert_eq!(res2, StepResult::AlreadyMigrated);

        // 4. cetz 0.3.2 is NOT restored, while cetz 0.3.4 remains untouched
        assert!(!new_cache.join("packages/preview/cetz/0.3.2").exists());
        assert!(new_cache.join("packages/preview/cetz/0.3.4/typst.toml").exists());
        assert_eq!(
            fs::read_to_string(new_cache.join("packages/preview/cetz/0.3.4/typst.toml")).unwrap(),
            "version = 0.3.4"
        );
    }

    #[test]
    fn test_cache_clearing_entire_directory_never_restores_legacy() {
        let temp = tempfile::tempdir().unwrap();
        let old_cache = temp.path().join("old_cache");
        let new_cache = temp.path().join("new_cache");
        let state_dir = temp.path().join("state");

        let old_pkg = old_cache.join("packages/preview/cetz/0.3.2");
        fs::create_dir_all(&old_pkg).unwrap();
        fs::write(old_pkg.join("typst.toml"), "version = 0.3.2").unwrap();

        // 1. Initial migration succeeds
        let res1 = migrate_cache_packages(&old_cache, &new_cache, &state_dir).unwrap();
        assert_eq!(res1, StepResult::Migrated);
        assert!(new_cache.join("packages/preview/cetz/0.3.2").exists());

        // 2. User or cache cleaner wipes the entire Whetstone cache directory
        fs::remove_dir_all(&new_cache).unwrap();
        assert!(!new_cache.exists());

        // 3. Application restarts and runs migration
        let res2 = migrate_cache_packages(&old_cache, &new_cache, &state_dir).unwrap();
        assert_eq!(res2, StepResult::AlreadyMigrated);

        // 4. Legacy packages must NOT be restored
        assert!(!new_cache.join("packages/preview/cetz/0.3.2").exists());
    }

    #[test]
    fn test_data_clearing_entire_directory_never_restores_legacy() {
        let temp = tempfile::tempdir().unwrap();
        let old_data = temp.path().join("old_data");
        let new_data = temp.path().join("new_data");
        let state_dir = temp.path().join("state");

        let old_template = old_data.join("templates/custom/template.typ");
        fs::create_dir_all(old_template.parent().unwrap()).unwrap();
        fs::write(&old_template, "custom template").unwrap();

        // 1. Initial migration succeeds
        let res1 = migrate_user_data(&old_data, &new_data, &state_dir).unwrap();
        assert_eq!(res1, StepResult::Migrated);
        assert!(new_data.join("templates/custom/template.typ").exists());

        // 2. User data directory wiped completely
        fs::remove_dir_all(&new_data).unwrap();
        assert!(!new_data.exists());

        // 3. Application restarts and runs migration
        let res2 = migrate_user_data(&old_data, &new_data, &state_dir).unwrap();
        assert_eq!(res2, StepResult::AlreadyMigrated);

        // 4. Legacy templates must NOT be restored
        assert!(!new_data.join("templates/custom/template.typ").exists());
    }

    #[test]
    fn test_existing_whetstone_files_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let old_cache = temp.path().join("old_cache");
        let new_cache = temp.path().join("new_cache");
        let state_dir = temp.path().join("state");

        let old_pkg = old_cache.join("packages/preview/cetz/0.3.2");
        let new_pkg = new_cache.join("packages/preview/cetz/0.3.2");
        fs::create_dir_all(&old_pkg).unwrap();
        fs::create_dir_all(&new_pkg).unwrap();
        fs::write(old_pkg.join("typst.toml"), "legacy version").unwrap();
        fs::write(new_pkg.join("typst.toml"), "whetstone version").unwrap();

        let res = migrate_cache_packages(&old_cache, &new_cache, &state_dir).unwrap();
        assert_eq!(res, StepResult::Migrated);
        assert_eq!(
            fs::read_to_string(new_pkg.join("typst.toml")).unwrap(),
            "whetstone version"
        );
    }

    #[test]
    fn test_failed_migration_not_marked_complete() {
        let temp = tempfile::tempdir().unwrap();
        let old_cache = temp.path().join("old_cache");
        let new_cache = temp.path().join("new_cache");
        let state_dir = temp.path().join("state");

        let old_pkg = old_cache.join("packages/preview/cetz/0.3.2");
        fs::create_dir_all(&old_pkg).unwrap();
        fs::write(old_pkg.join("file.txt"), "content").unwrap();

        // Cause migration failure: create a file named "packages" where a directory is expected
        fs::create_dir_all(&new_cache).unwrap();
        fs::write(new_cache.join("packages"), "blocking file").unwrap();

        let err = migrate_cache_packages(&old_cache, &new_cache, &state_dir);
        assert!(err.is_err());
        // Sentinel must NOT be written when migration fails
        assert!(!state_dir.join(CACHE_SENTINEL_NAME).exists());
    }

    #[test]
    fn test_subsequent_successful_migration_after_failure() {
        let temp = tempfile::tempdir().unwrap();
        let old_cache = temp.path().join("old_cache");
        let new_cache = temp.path().join("new_cache");
        let state_dir = temp.path().join("state");

        let old_pkg = old_cache.join("packages/preview/cetz/0.3.2");
        fs::create_dir_all(&old_pkg).unwrap();
        fs::write(old_pkg.join("file.txt"), "content").unwrap();

        // Block initially
        fs::create_dir_all(&new_cache).unwrap();
        fs::write(new_cache.join("packages"), "blocking file").unwrap();
        assert!(migrate_cache_packages(&old_cache, &new_cache, &state_dir).is_err());
        assert!(!state_dir.join(CACHE_SENTINEL_NAME).exists());

        // Unblock and retry
        fs::remove_file(new_cache.join("packages")).unwrap();
        let res = migrate_cache_packages(&old_cache, &new_cache, &state_dir).unwrap();
        assert_eq!(res, StepResult::Migrated);
        assert!(state_dir.join(CACHE_SENTINEL_NAME).exists());
        assert_eq!(
            fs::read_to_string(new_cache.join("packages/preview/cetz/0.3.2/file.txt")).unwrap(),
            "content"
        );
    }

    #[test]
    fn test_user_data_migration_performed_once() {
        let temp = tempfile::tempdir().unwrap();
        let old_data = temp.path().join("old_data");
        let new_data = temp.path().join("new_data");
        let state_dir = temp.path().join("state");

        fs::create_dir_all(old_data.join("templates/custom")).unwrap();
        fs::write(old_data.join("templates/custom/template.typ"), "custom template").unwrap();

        // Initial migration
        let res = migrate_user_data(&old_data, &new_data, &state_dir).unwrap();
        assert_eq!(res, StepResult::Migrated);
        assert!(new_data.join("templates/custom/template.typ").exists());
        assert!(state_dir.join(DATA_SENTINEL_NAME).exists());

        // User deletes template in Whetstone
        fs::remove_file(new_data.join("templates/custom/template.typ")).unwrap();

        // Second run
        let res2 = migrate_user_data(&old_data, &new_data, &state_dir).unwrap();
        assert_eq!(res2, StepResult::AlreadyMigrated);

        // Deleted template is not restored
        assert!(!new_data.join("templates/custom/template.typ").exists());
    }

    #[test]
    fn test_existing_installation_compatibility() {
        let temp = tempfile::tempdir().unwrap();
        let paths = MigrationPaths {
            old_cache_dir: temp.path().join("old_cache"),
            new_cache_dir: temp.path().join("new_cache"),
            old_data_dir: temp.path().join("old_data"),
            new_data_dir: temp.path().join("new_data"),
            state_dir: temp.path().join("state"),
        };

        // Create legacy packages
        fs::create_dir_all(paths.old_cache_dir.join("packages/preview/cetz/0.3.2")).unwrap();

        // Pre-create the sentinel in state_dir simulating an already completed migration
        fs::create_dir_all(&paths.state_dir).unwrap();
        fs::write(paths.state_dir.join(CACHE_SENTINEL_NAME), b"").unwrap();

        // Running cache migration must not copy anything from legacy
        let res = migrate_cache_packages(&paths.old_cache_dir, &paths.new_cache_dir, &paths.state_dir).unwrap();
        assert_eq!(res, StepResult::AlreadyMigrated);
        assert!(!paths.new_cache_dir.join("packages/preview/cetz/0.3.2").exists());
    }
}
