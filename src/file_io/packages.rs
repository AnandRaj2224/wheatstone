// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use semver::Version;
use serde::Deserialize;
use std::path::{Path, PathBuf};

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Deserialize)]
struct PackageMeta {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(rename = "display-name", default)]
    display_name: Option<String>,
    #[serde(default)]
    repository: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TemplateMeta {
    path: String,
    entrypoint: String,
    #[serde(default)]
    thumbnail: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TypstToml {
    package: PackageMeta,
    #[serde(default)]
    template: Option<TemplateMeta>,
}

/// A locally installed package, ready to present in the UI.
#[derive(Debug, Clone)]
pub struct PackageInfo {
    /// Short machine-readable name from `[package].name` (e.g. `diatypst`).
    pub name: String,
    /// Full slash-separated path as it appears in the import spec
    /// (e.g. `skriptum/diatypst`). For simple ident namespaces this is
    /// the same as `name`.
    pub path: String,
    /// Optional human-readable name from `[package].display-name`.
    /// Falls back to [`name`] when not present.
    pub display_name: Option<String>,
    /// Version string from `[package].version`
    pub version: String,
    /// Description from `[package].description`, if present
    pub description: Option<String>,
    /// Authors from `[package].authors`
    pub authors: Vec<String>,
    /// Namespace the package was found under (e.g. `local`, `preview`, `github.com`)
    pub namespace: String,
    /// Absolute path to the package directory
    pub package_dir: PathBuf,
    /// Repository URL from `[package].repository`, if present
    pub repository: Option<String>,
}

impl PackageInfo {
    /// Returns the display name if set, otherwise the short machine-readable name.
    pub fn display_name_or_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }

    /// Returns the Typst package specifier, e.g. `@preview/name:1.0.0` or `@github.com/user/repo:1.0.0`.
    pub fn specifier(&self) -> String {
        format!("@{}/{}:{}", self.namespace, self.path, self.version)
    }
}

/// A locally installed template package, ready to present in the UI.
#[derive(Debug, Clone)]
pub struct TemplateInfo {
    /// The underlying package metadata.
    pub package: PackageInfo,
    /// Absolute path to the template entrypoint file
    /// (`[template].path` / `[template].entrypoint`)
    pub entrypoint_path: PathBuf,
    /// Absolute path to the thumbnail image, if present and the file exists
    pub thumbnail_path: Option<PathBuf>,
    /// Whether this template was bundled with the application rather than installed by the user.
    pub is_bundled: bool,
}

impl TemplateInfo {
    /// Convenience forwarder — returns display name or falls back to name.
    pub fn display_name_or_name(&self) -> &str {
        self.package.display_name_or_name()
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Scans the user packages directory, returning a sorted list of all
/// [`PackageInfo`] found, regardless of whether they have a `[template]`
/// section. All versions of each package are included.
pub fn load_packages() -> Vec<PackageInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut packages: Vec<PackageInfo> = Vec::new();

    let user_packages = crate::utils::xdg_data_dir().join("packages");
    scan_dir(&user_packages, false, &mut seen, &mut packages, false);

    packages.sort_by(|a, b| {
        a.name.cmp(&b.name).then_with(|| {
            let va = Version::parse(&a.version).ok();
            let vb = Version::parse(&b.version).ok();
            vb.cmp(&va)
        })
    });

    packages
}

/// Scans the package download cache, returning a sorted list of all
/// [`PackageInfo`] found. All versions of each package are included.
/// These packages were downloaded automatically by the compiler and can
/// be re-fetched on demand, so deletion is non-destructive.
pub fn load_cached_packages() -> Vec<PackageInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut packages: Vec<PackageInfo> = Vec::new();

    let cache_dir = crate::utils::xdg_cache_dir().join("packages");
    scan_dir(&cache_dir, false, &mut seen, &mut packages, false);

    packages.sort_by(|a, b| {
        a.name.cmp(&b.name).then_with(|| {
            let va = Version::parse(&a.version).ok();
            let vb = Version::parse(&b.version).ok();
            vb.cmp(&va)
        })
    });

    packages
}

/// Scans the bundled templates directory, returning a sorted list of all
/// [`PackageInfo`] found.
pub fn load_bundled_packages() -> Vec<PackageInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut packages: Vec<PackageInfo> = Vec::new();

    let bundled_dir = crate::utils::bundled_templates_dir();
    scan_dir(&bundled_dir, true, &mut seen, &mut packages, false);

    packages.sort_by(|a, b| {
        a.name.cmp(&b.name).then_with(|| {
            let va = Version::parse(&a.version).ok();
            let vb = Version::parse(&b.version).ok();
            vb.cmp(&va)
        })
    });

    packages
}

/// Scans the user packages directory and the bundled templates directory,
/// returning a deduplicated, sorted list of [`TemplateInfo`] for packages
/// that have a `[template]` section in their `typst.toml`. Only the highest
/// version of each package is returned.
pub fn load_templates() -> Vec<TemplateInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut templates: Vec<TemplateInfo> = Vec::new();

    let user_packages = crate::utils::xdg_data_dir().join("packages");
    scan_dir(&user_packages, false, &mut seen, &mut templates, true);

    let bundled_packages = crate::utils::bundled_templates_dir();
    scan_dir(&bundled_packages, true, &mut seen, &mut templates, true);

    templates.sort_by(|a, b| a.package.name.cmp(&b.package.name));
    templates
}

// ============================================================================
// Internal Implementation
// ============================================================================

/// Trait allowing `scan_dir` to push into either a `Vec<PackageInfo>` or
/// a `Vec<TemplateInfo>` depending on the caller.
trait PackageCollector {
    fn collect(
        &mut self,
        seen: &mut std::collections::HashSet<String>,
        package: PackageInfo,
        template: Option<TemplateMeta>,
        package_dir: &Path,
        is_bundled: bool,
    );
}

impl PackageCollector for Vec<PackageInfo> {
    fn collect(
        &mut self,
        _seen: &mut std::collections::HashSet<String>,
        package: PackageInfo,
        _template: Option<TemplateMeta>,
        _package_dir: &Path,
        _is_bundled: bool,
    ) {
        self.push(package);
    }
}

impl PackageCollector for Vec<TemplateInfo> {
    fn collect(
        &mut self,
        seen: &mut std::collections::HashSet<String>,
        package: PackageInfo,
        template: Option<TemplateMeta>,
        package_dir: &Path,
        is_bundled: bool,
    ) {
        let Some(template) = template else { return };

        let entrypoint_path = package_dir.join(&template.path).join(&template.entrypoint);

        if !entrypoint_path.exists() {
            eprintln!(
                "packages: entrypoint not found: {}",
                entrypoint_path.display()
            );
            return;
        }

        let thumbnail_path = template
            .thumbnail
            .as_deref()
            .map(|thumb| package_dir.join(thumb))
            .filter(|p| p.exists());

        let info = TemplateInfo {
            package,
            entrypoint_path,
            thumbnail_path,
            is_bundled,
        };

        if let Some(existing) = self
            .iter_mut()
            .find(|t| t.package.name == info.package.name)
        {
            let new_ver = Version::parse(&info.package.version).ok();
            let old_ver = Version::parse(&existing.package.version).ok();
            if new_ver > old_ver || (new_ver == old_ver && !info.is_bundled && existing.is_bundled)
            {
                *existing = info;
            }
        } else {
            seen.insert(info.package.name.clone());
            self.push(info);
        }
    }
}

/// Scans `packages_dir`, which is expected to contain namespace subdirectories.
/// For each namespace, recursively searches for version directories — identified
/// as directories whose name parses as a semver version and contain a
/// `typst.toml`. Everything between the namespace directory and the version
/// directory is joined with `/` to form the package path, allowing arbitrarily
/// deep paths (e.g. `github.com/user/repo/1.0.0` or
/// `gitlab.com/group/subgroup/repo/1.0.0`).
fn scan_dir<C: PackageCollector>(
    packages_dir: &Path,
    is_bundled: bool,
    seen: &mut std::collections::HashSet<String>,
    collector: &mut C,
    templates_only: bool,
) {
    let Ok(namespace_entries) = std::fs::read_dir(packages_dir) else {
        return;
    };

    for namespace_entry in namespace_entries.flatten() {
        let namespace_dir = namespace_entry.path();
        if !namespace_dir.is_dir() {
            continue;
        }

        let namespace = namespace_entry.file_name().to_string_lossy().to_string();

        find_versions(
            &namespace_dir,
            &namespace,
            &[],
            is_bundled,
            seen,
            collector,
            templates_only,
        );
    }
}

/// Recursively walks `dir`, accumulating path segments in `name_parts`.
/// When a directory whose name parses as semver is found and contains a
/// `typst.toml`, it is treated as a package version directory. The package
/// path is formed by joining `name_parts` with `/`, and the short name comes
/// from the manifest's `[package].name` field.
fn find_versions<C: PackageCollector>(
    dir: &Path,
    namespace: &str,
    name_parts: &[String],
    is_bundled: bool,
    seen: &mut std::collections::HashSet<String>,
    collector: &mut C,
    templates_only: bool,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden directories (e.g. `.tmp-*` dirs used during atomic
        // git clones).
        if dir_name.starts_with('.') {
            continue;
        }

        if Version::parse(&dir_name).is_ok() {
            // This looks like a version directory — check for typst.toml.
            let toml_path = path.join("typst.toml");
            if !toml_path.exists() {
                continue;
            }

            // Must have at least one name segment.
            if name_parts.is_empty() {
                continue;
            }

            let path_str = name_parts.join("/");

            match parse_toml(&toml_path) {
                Ok(parsed) => {
                    if templates_only && parsed.template.is_none() {
                        continue;
                    }

                    let package = PackageInfo {
                        name: parsed.package.name,
                        path: path_str,
                        display_name: parsed.package.display_name,
                        version: parsed.package.version,
                        description: parsed.package.description,
                        authors: parsed.package.authors,
                        namespace: namespace.to_string(),
                        package_dir: path.clone(),
                        repository: parsed.package.repository,
                    };

                    collector.collect(seen, package, parsed.template, &path, is_bundled);
                }
                Err(e) => eprintln!("packages: failed to parse {}: {}", toml_path.display(), e),
            }
        } else {
            // Not a version directory — recurse deeper, accumulating this
            // segment as part of the package path.
            let mut new_parts = name_parts.to_vec();
            new_parts.push(dir_name);
            find_versions(
                &path,
                namespace,
                &new_parts,
                is_bundled,
                seen,
                collector,
                templates_only,
            );
        }
    }
}

fn parse_toml(toml_path: &Path) -> Result<TypstToml, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(toml_path)?;
    Ok(toml::from_str(&contents)?)
}
