// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use chrono::{DateTime, Datelike, FixedOffset, Local, Utc};
use std::{
    collections::HashMap,
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};
use typst::{
    Library, LibraryExt, World, comemo,
    diag::{FileResult, SourceDiagnostic},
    ecow::EcoVec,
    foundations::{Bytes, Datetime, Duration},
    syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot},
    text::{Font, FontBook},
    utils::LazyHash,
};
use typst_ide::IdeWorld;
use typst_kit::{
    downloader::SystemDownloader,
    fonts::{self, FontStore},
    packages::{FsPackages, SystemPackages, UniversePackages},
};

use super::file_slot::FileSlot;

pub struct TypesetterWorld {
    root: PathBuf,
    main: FileId,
    diagnostics: EcoVec<SourceDiagnostic>,
    open_source: Option<Source>,
    pub(crate) library: Arc<LazyHash<Library>>,
    pub(crate) fonts: Arc<FontStore>,
    pub(crate) package_storage: Arc<SystemPackages>,
    pub files: Arc<Mutex<HashMap<FileId, FileSlot>>>,
    now: OnceLock<DateTime<Utc>>,
}

impl fmt::Debug for TypesetterWorld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `FontStore` doesn't implement `Debug`, so it and a couple of other
        // less-interesting fields are omitted here.
        f.debug_struct("TypesetterWorld")
            .field("root", &self.root)
            .field("main", &self.main)
            .field("diagnostics", &self.diagnostics)
            .field("open_source", &self.open_source)
            .field("package_storage", &self.package_storage)
            .field("files", &self.files)
            .finish_non_exhaustive()
    }
}

#[allow(clippy::new_without_default)]
impl TypesetterWorld {
    pub fn new() -> Self {
        #[cfg(debug_assertions)]
        crate::memory_tracking::MEMORY_COUNTERS
            .worlds_created
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        Self::with_shared_resources(None, None, None, None)
    }

    /// Create a new world, optionally sharing resources.
    pub(crate) fn with_shared_resources(
        fonts: Option<Arc<FontStore>>,
        package_storage: Option<Arc<SystemPackages>>,
        files: Option<Arc<Mutex<HashMap<FileId, FileSlot>>>>,
        library: Option<Arc<LazyHash<Library>>>,
    ) -> Self {
        let root = PathBuf::from("/virtual");
        let main =
            RootedPath::new(VirtualRoot::Project, VirtualPath::new("main.typ").unwrap()).intern();

        let fonts = fonts.unwrap_or_else(|| {
            let mut store = FontStore::new();

            // Scan user local drop-zone
            let user_font_path = crate::utils::xdg_data_dir().join("fonts");
            let _ = std::fs::create_dir_all(&user_font_path);
            store.extend(fonts::scan(&user_font_path));

            // TODO: This was a user suggestion (possibly vibe suggested?); double check that this doesn't cause fonts in here to get loaded twice in Flatpak builds as Flatpak already automatically maps the font directory correctly
            // Explicitly scan the app's asset directory so non-Flatpak builds can find shipped fonts
            let app_font_path = std::path::PathBuf::from(crate::config::PKGDATADIR).join("fonts");
            store.extend(fonts::scan(&app_font_path));

            // Load standard Typst defaults
            store.extend(fonts::embedded());

            // Scan global host fonts
            store.extend(fonts::system());

            Arc::new(store)
        });

        let package_storage = package_storage.unwrap_or_else(|| {
            let downloader = SystemDownloader::new(concat!(
                "dev.whetstone.editor/",
                env!("CARGO_PKG_VERSION")
            ));

            let cache_dir = crate::utils::xdg_cache_dir().join("packages");
            let package_path = crate::utils::xdg_data_dir().join("packages");
            let _ = std::fs::create_dir_all(&cache_dir);
            let _ = std::fs::create_dir_all(package_path.join("local"));

            Arc::new(SystemPackages::from_parts(
                Some(FsPackages::new(package_path)),
                Some(FsPackages::new(cache_dir)),
                UniversePackages::new(downloader),
            ))
        });

        let library = library.unwrap_or_else(|| {
            Arc::new(LazyHash::new(
                Library::builder()
                    .with_features(std::iter::once(typst::Feature::Html).collect())
                    .build(),
            ))
        });

        Self {
            root,
            main,
            diagnostics: EcoVec::new(),
            open_source: None,
            library,
            fonts,
            package_storage,
            files: files.unwrap_or_else(|| Arc::new(Mutex::new(HashMap::new()))),
            now: OnceLock::new(),
        }
    }

    pub fn update(
        &mut self,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
        content: String,
    ) {
        {
            let mut files = self.files.lock().unwrap();
            for slot in files.values_mut() {
                slot.mark_not_accessed();
            }
        }

        self.open_source = None;
        self.diagnostics = EcoVec::new();
        self.now.take();

        if let Some(file_path) = &path {
            let canon_file =
                std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.clone());
            let canon_project_root = project_root.as_ref().map(|pr| {
                std::fs::canonicalize(pr).unwrap_or_else(|_| pr.clone())
            });

            // Virtualize relative to project_root if it contains file_path,
            // otherwise fall back to file's parent directory.
            let (root, vpath) = if let Some(ref pr) = canon_project_root
                && let Ok(vpath) = VirtualPath::virtualize(pr, &canon_file)
            {
                (pr.clone(), vpath)
            } else if let Some(parent) = canon_file.parent()
                && let Ok(vpath) = VirtualPath::virtualize(parent, &canon_file)
            {
                (parent.to_path_buf(), vpath)
            } else {
                let file_name = canon_file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("main.typ");
                let vpath = VirtualPath::new(file_name)
                    .unwrap_or_else(|_| VirtualPath::new("main.typ").unwrap());
                (
                    canon_file
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from("/virtual")),
                    vpath,
                )
            };

            self.root = root;
            self.main = RootedPath::new(VirtualRoot::Project, vpath).intern();
        } else {
            self.root = project_root.unwrap_or_else(|| PathBuf::from("/virtual"));
            self.main =
                RootedPath::new(VirtualRoot::Project, VirtualPath::new("main.typ").unwrap())
                    .intern();
        }

        self.open_source = Some(Source::new(self.main, content));
    }

    pub fn diagnostics(&self) -> EcoVec<SourceDiagnostic> {
        self.diagnostics.clone()
    }

    pub fn set_diagnostics(&mut self, diagnostics: EcoVec<SourceDiagnostic>) {
        self.diagnostics = diagnostics;
    }

    pub fn accessed_files(&self) -> Vec<PathBuf> {
        let files = self.files.lock().unwrap();
        files
            .values()
            .filter_map(|f| f.accessed_local_path(&self.root))
            .collect()
    }

    /// Returns a reference to the package storage.
    pub fn package_storage(&self) -> &Arc<SystemPackages> {
        &self.package_storage
    }

    fn slot<F, T>(&self, id: FileId, f: F) -> T
    where
        F: FnOnce(&mut FileSlot) -> T,
    {
        let mut map = self.files.lock().expect("files mutex poisoned");
        f(map.entry(id).or_insert_with(|| FileSlot::new(id)))
    }

    /// Call this after compilation to free memory aggressively.
    pub fn post_compile_cleanup(&mut self) {
        let mut files = self.files.lock().unwrap();
        let main_id = self.main;

        #[cfg(debug_assertions)]
        let before_count = files.len();

        files.retain(|id, _| *id == main_id);

        for slot in files.values_mut() {
            slot.clear_bytes_cache();
        }

        files.shrink_to_fit();
        comemo::evict(10);

        #[cfg(debug_assertions)]
        println!(
            "TypesetterWorld: post_compile_cleanup removed {} files",
            before_count - files.len()
        );
    }

    #[cfg(debug_assertions)]
    pub fn diagnose_memory(&self) {
        let files = self.files.lock().unwrap();
        let total_files = files.len();
        let total_size: usize = files.values().map(|f| f.cached_size()).sum();
        let files_with_source: usize = files.values().filter(|f| f.has_source()).count();
        let files_with_bytes: usize = files.values().filter(|f| f.has_bytes()).count();

        println!("\n========== WORLD MEMORY DIAGNOSTIC ==========");
        println!("Cached files: {}", total_files);
        println!("  - With source: {}", files_with_source);
        println!("  - With bytes: {}", files_with_bytes);
        println!("Total cache size: {} MB", total_size / 1_000_000);
        println!("Files Arc strong count: {}", Arc::strong_count(&self.files));
        println!("Fonts Arc strong count: {}", Arc::strong_count(&self.fonts));
        println!("==============================================\n");
    }

    fn walk_dir(
        &self,
        dir: &std::path::Path,
        known: &std::collections::HashSet<FileId>,
        ids: &mut Vec<FileId>,
        depth: usize,
    ) {
        // Abort if there are too many results (which can happen if the user did something
        // silly like using their entire home directory as the project root). Otherwise,
        // the scan could take a long time.
        if depth > 3 || ids.len() > 128 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || matches!(name, "node_modules" | "target") {
                continue;
            }
            if path.is_dir() {
                self.walk_dir(&path, known, ids, depth + 1);
            } else if let Ok(rel) = path.strip_prefix(&self.root)
                && let Some(rel_str) = rel.to_str()
                && let Ok(vpath) = VirtualPath::new(rel_str)
            {
                let id = RootedPath::new(VirtualRoot::Project, vpath).intern();
                if !known.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }
}

impl Clone for TypesetterWorld {
    fn clone(&self) -> Self {
        #[cfg(debug_assertions)]
        crate::memory_tracking::MEMORY_COUNTERS
            .worlds_created
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        Self::with_shared_resources(
            Some(Arc::clone(&self.fonts)),
            Some(Arc::clone(&self.package_storage)),
            Some(Arc::clone(&self.files)),
            Some(Arc::clone(&self.library)),
        )
    }
}

impl World for TypesetterWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if let Some(source) = &self.open_source
            && source.id() == id
        {
            return Ok(source.clone());
        }
        self.slot(id, |slot| slot.source(&self.root, &self.package_storage))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if let Some(source) = &self.open_source
            && source.id() == id
        {
            return Ok(Bytes::from_string(source.text().to_string()));
        }
        self.slot(id, |slot| slot.bytes(&self.root, &self.package_storage))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        let now = self.now.get_or_init(Utc::now);

        let with_offset = match offset {
            None => now.with_timezone(&Local).fixed_offset(),
            Some(duration) => {
                let seconds = duration.seconds();
                if !seconds.is_finite()
                    || seconds < f64::from(i32::MIN)
                    || seconds > f64::from(i32::MAX)
                {
                    return None;
                }
                now.with_timezone(&FixedOffset::east_opt(seconds as i32)?)
            }
        };

        Datetime::from_ymd(
            with_offset.year(),
            with_offset.month().try_into().ok()?,
            with_offset.day().try_into().ok()?,
        )
    }
}

impl Drop for TypesetterWorld {
    fn drop(&mut self) {
        if Arc::strong_count(&self.files) == 1 {
            self.files.lock().unwrap().clear();
        }

        self.open_source = None;
        self.diagnostics = EcoVec::new();
        self.now.take();

        #[cfg(debug_assertions)]
        crate::memory_tracking::MEMORY_COUNTERS
            .worlds_dropped
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl IdeWorld for TypesetterWorld {
    fn upcast(&self) -> &dyn World {
        self
    }

    fn files(&self) -> Vec<FileId> {
        use std::collections::HashSet;

        let known: HashSet<FileId> = {
            let files = self.files.lock().unwrap();
            files.keys().copied().collect()
        };

        let mut ids: Vec<FileId> = known.iter().copied().collect();
        self.walk_dir(&self.root, &known, &mut ids, 0);
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typst_system::DocumentArena;
    use std::fs;
    use tempfile::tempdir;
    use typst_layout::PagedDocument;

    #[test]
    fn test_sibling_relative_import_with_project_root() {
        // Reproduce exact user scenario:
        // Project root: Notes
        // Notes/Quant/quant.typ
        // Notes/Quant/quantNotes.typ importing "quant.typ"
        let temp = tempdir().expect("tempdir");
        let notes_dir = temp.path().canonicalize().unwrap();
        let quant_dir = notes_dir.join("Quant");
        fs::create_dir_all(&quant_dir).unwrap();

        let quant_file = quant_dir.join("quant.typ");
        fs::write(&quant_file, "#let answer = 42;\n").unwrap();

        let quant_notes_file = quant_dir.join("quantNotes.typ");
        let source = "#import \"quant.typ\": *\n= Quant Notes\nResult: #answer".to_string();
        fs::write(&quant_notes_file, &source).unwrap();

        let mut world = TypesetterWorld::new();
        world.update(Some(quant_notes_file.clone()), Some(notes_dir.clone()), source);

        let compilation = typst::compile::<PagedDocument>(&world);
        assert!(
            compilation.output.is_ok(),
            "Compilation failed: {:?}",
            compilation.output.err()
        );

        let accessed = world.accessed_files();
        let accessed_names: Vec<_> = accessed.iter().filter_map(|p| p.file_name()).collect();
        assert!(
            accessed_names.iter().any(|name| *name == "quant.typ"),
            "quant.typ was not accessed: {:?}",
            accessed
        );
    }

    #[test]
    fn test_subdirectory_import() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().canonicalize().unwrap();
        let chapters = root.join("chapters");
        fs::create_dir_all(&chapters).unwrap();

        let chap1 = chapters.join("chap1.typ");
        fs::write(&chap1, "#let chap1_title = \"Introduction\";\n").unwrap();

        let main_file = root.join("main.typ");
        let source = "#import \"chapters/chap1.typ\": *\nTitle: #chap1_title".to_string();
        fs::write(&main_file, &source).unwrap();

        let mut world = TypesetterWorld::new();
        world.update(Some(main_file.clone()), Some(root.clone()), source);

        let compilation = typst::compile::<PagedDocument>(&world);
        assert!(
            compilation.output.is_ok(),
            "Compilation failed: {:?}",
            compilation.output.err()
        );

        let accessed = world.accessed_files();
        let accessed_names: Vec<_> = accessed.iter().filter_map(|p| p.file_name()).collect();
        assert!(
            accessed_names.iter().any(|name| *name == "chap1.typ"),
            "chap1.typ was not accessed: {:?}",
            accessed
        );
    }

    #[test]
    fn test_nested_cross_directory_imports() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().canonicalize().unwrap();
        let sub1 = root.join("sub1");
        let sub2 = sub1.join("sub2");
        fs::create_dir_all(&sub2).unwrap();

        let common = root.join("common.typ");
        fs::write(&common, "#let common_val = 100;\n").unwrap();

        let sibling = sub2.join("sibling.typ");
        fs::write(&sibling, "#let sibling_val = 200;\n").unwrap();

        let doc = sub2.join("doc.typ");
        let source =
            "#import \"../../common.typ\": *\n#import \"sibling.typ\": *\nTotal: #(common_val + sibling_val)"
                .to_string();
        fs::write(&doc, &source).unwrap();

        let mut world = TypesetterWorld::new();
        world.update(Some(doc.clone()), Some(root.clone()), source);

        let compilation = typst::compile::<PagedDocument>(&world);
        assert!(
            compilation.output.is_ok(),
            "Compilation failed: {:?}",
            compilation.output.err()
        );

        let accessed = world.accessed_files();
        let accessed_names: Vec<_> = accessed.iter().filter_map(|p| p.file_name()).collect();
        assert!(accessed_names.iter().any(|name| *name == "common.typ"));
        assert!(accessed_names.iter().any(|name| *name == "sibling.typ"));
    }

    #[test]
    fn test_relative_image_and_data_loading() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().canonicalize().unwrap();
        let assets = root.join("assets");
        fs::create_dir_all(&assets).unwrap();

        // Minimal valid 1x1 PNG bytes
        let png_bytes = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, // PNG signature
            0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89,
            0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, // IDAT chunk
            0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, // IEND chunk
            0xae, 0x42, 0x60, 0x82,
        ];
        let img_path = assets.join("pixel.png");
        fs::write(&img_path, &png_bytes).unwrap();

        let csv_path = assets.join("data.csv");
        fs::write(&csv_path, "name,score\nAlice,95\nBob,88\n").unwrap();

        let doc = assets.join("report.typ");
        let source =
            "#let d = csv(\"data.csv\")\n#image(\"pixel.png\")\nTop student: #d.at(1).at(0)"
                .to_string();
        fs::write(&doc, &source).unwrap();

        let mut world = TypesetterWorld::new();
        world.update(Some(doc.clone()), Some(root.clone()), source);

        let compilation = typst::compile::<PagedDocument>(&world);
        assert!(
            compilation.output.is_ok(),
            "Compilation failed: {:?}",
            compilation.output.err()
        );

        let accessed = world.accessed_files();
        let accessed_names: Vec<_> = accessed.iter().filter_map(|p| p.file_name()).collect();
        assert!(accessed_names.iter().any(|name| *name == "pixel.png"));
        assert!(accessed_names.iter().any(|name| *name == "data.csv"));
    }

    #[test]
    fn test_standalone_file_without_project_root() {
        let temp = tempdir().expect("tempdir");
        let dir = temp.path().canonicalize().unwrap();

        let helper = dir.join("helper.typ");
        fs::write(&helper, "#let standalone_var = \"working\";\n").unwrap();

        let doc = dir.join("doc.typ");
        let source = "#import \"helper.typ\": *\nValue: #standalone_var".to_string();
        fs::write(&doc, &source).unwrap();

        let mut world = TypesetterWorld::new();
        // project_root = None
        world.update(Some(doc.clone()), None, source);

        let compilation = typst::compile::<PagedDocument>(&world);
        assert!(
            compilation.output.is_ok(),
            "Compilation failed: {:?}",
            compilation.output.err()
        );

        let accessed = world.accessed_files();
        let accessed_names: Vec<_> = accessed.iter().filter_map(|p| p.file_name()).collect();
        assert!(accessed_names.iter().any(|name| *name == "helper.typ"));
    }

    #[test]
    fn test_file_outside_configured_project_root() {
        let temp = tempdir().expect("tempdir");
        let project_root = temp.path().join("proj_root");
        fs::create_dir_all(&project_root).unwrap();

        let outside_dir = temp.path().join("outside_dir");
        fs::create_dir_all(&outside_dir).unwrap();

        let helper = outside_dir.join("helper.typ");
        fs::write(&helper, "#let outside_var = \"success\";\n").unwrap();

        let doc = outside_dir.join("doc.typ");
        let source = "#import \"helper.typ\": *\nValue: #outside_var".to_string();
        fs::write(&doc, &source).unwrap();

        let mut world = TypesetterWorld::new();
        // project_root does not contain doc.typ, so world must fall back to doc.typ's parent
        world.update(Some(doc.clone()), Some(project_root), source);

        let compilation = typst::compile::<PagedDocument>(&world);
        assert!(
            compilation.output.is_ok(),
            "Compilation failed: {:?}",
            compilation.output.err()
        );

        let accessed = world.accessed_files();
        let accessed_names: Vec<_> = accessed.iter().filter_map(|p| p.file_name()).collect();
        assert!(accessed_names.iter().any(|name| *name == "helper.typ"));
    }

    #[test]
    fn test_document_switching_preserves_shared_resources_and_isolation() {
        let arena = DocumentArena::new();

        let temp_a = tempdir().expect("tempdir");
        let root_a = temp_a.path().canonicalize().unwrap();
        let helper_a = root_a.join("helper_a.typ");
        fs::write(&helper_a, "#let a_val = \"doc_A\";\n").unwrap();
        let doc_a = root_a.join("main.typ");
        let source_a = "#import \"helper_a.typ\": *\nDoc: #a_val".to_string();

        let res_a = arena.compile_and_update(source_a, Some(doc_a), Some(root_a));
        assert!(
            res_a.diagnostics.is_empty(),
            "Doc A had errors: {:?}",
            res_a.diagnostics
        );
        assert!(!res_a.page_metadata.is_empty());

        // Measure reset timing - must be fast (< 5ms) because fonts/packages are reused
        let t_reset = std::time::Instant::now();
        arena.reset();
        let reset_duration = t_reset.elapsed();
        assert!(
            reset_duration < std::time::Duration::from_millis(15),
            "Arena reset took too long ({:?}) - fonts might be rescanning",
            reset_duration
        );

        let temp_b = tempdir().expect("tempdir");
        let root_b = temp_b.path().canonicalize().unwrap();
        let helper_b = root_b.join("helper_b.typ");
        fs::write(&helper_b, "#let b_val = \"doc_B\";\n").unwrap();
        let doc_b = root_b.join("main.typ");
        let source_b = "#import \"helper_b.typ\": *\nDoc: #b_val".to_string();

        let res_b = arena.compile_and_update(source_b, Some(doc_b), Some(root_b));
        assert!(
            res_b.diagnostics.is_empty(),
            "Doc B had errors: {:?}",
            res_b.diagnostics
        );
        assert!(!res_b.page_metadata.is_empty());

        // Ensure dependencies only contain doc B's files, not doc A's
        let dep_names: Vec<_> = res_b.dependencies.iter().filter_map(|p| p.file_name()).collect();
        assert!(dep_names.iter().any(|name| *name == "helper_b.typ"));
        assert!(!dep_names.iter().any(|name| *name == "helper_a.typ"));
    }

    #[test]
    fn test_accessed_files_tracked_on_compilation_failure() {
        let arena = DocumentArena::new();
        let temp = tempdir().expect("tempdir");
        let root = temp.path().canonicalize().unwrap();

        let helper = root.join("quant.typ");
        fs::write(&helper, "#let answer = 42;\n").unwrap();

        let doc = root.join("main.typ");
        // Import exists, but file has a subsequent syntax error
        let source = "#import \"quant.typ\": *\n#invalid syntax here +++".to_string();

        let res = arena.compile_and_update(source, Some(doc), Some(root));
        assert!(
            !res.diagnostics.is_empty(),
            "Expected compilation diagnostics/errors"
        );

        // The accessed helper file MUST be tracked even on failure so file monitors can reload when fixed
        let dep_names: Vec<_> = res.dependencies.iter().filter_map(|p| p.file_name()).collect();
        assert!(
            dep_names.iter().any(|name| *name == "quant.typ"),
            "quant.typ should be in dependencies on failure: {:?}",
            res.dependencies
        );
    }
}
