// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

mod compilation;
mod file_slot;
mod ide;
mod markdown;
mod rendering;
mod types;
mod world;

use gtk::glib::Bytes;
use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};
use typst_layout::PagedDocument;

pub use compilation::CompilationWorker;
pub use ide::IdeWorker;
pub use rendering::RenderingWorker;
pub use types::*;

use world::TypesetterWorld;

/// In debug builds, wraps PagedDocument to track creation and destruction
/// via MEMORY_COUNTERS. In release builds, this is a transparent zero-cost
/// newtype with no instrumentation.
struct TrackedDocument(PagedDocument);

impl TrackedDocument {
    fn new(doc: PagedDocument) -> Self {
        #[cfg(debug_assertions)]
        {
            crate::memory_tracking::MEMORY_COUNTERS
                .documents_created
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            println!("TrackedDocument: Created ({} pages)", doc.pages().len());
        }
        Self(doc)
    }
}

impl Drop for TrackedDocument {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        {
            crate::memory_tracking::MEMORY_COUNTERS
                .documents_dropped
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            println!("TrackedDocument: Dropped ({} pages)", self.0.pages().len());
        }
    }
}

/// Storage slot for compiled document and world
struct DocumentSlot {
    generation: u64,
    world: TypesetterWorld,
    document: Option<TrackedDocument>,
    /// Whether the most recent compilation succeeded. When false, `document`
    /// holds the last *successful* document (which may be from a prior source
    /// version) and `world` reflects the current (erroring) source. IDE
    /// features that correlate world↔document positions should check this
    /// flag and degrade gracefully rather than producing misleading results.
    last_compile_succeeded: bool,
}

/// Central arena that owns all compilation state
pub struct DocumentArena {
    current: RwLock<DocumentSlot>,
    generation: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for DocumentArena {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentArena")
            .field("generation", &self.current_generation())
            .finish_non_exhaustive()
    }
}

impl DocumentArena {
    pub(crate) fn new() -> Self {
        Self {
            current: RwLock::new(DocumentSlot {
                generation: 0,
                world: TypesetterWorld::new(),
                document: None,
                last_compile_succeeded: false,
            }),
            generation: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Get current document generation (lock-free)
    pub fn current_generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Returns whether the most recent compilation succeeded.
    ///
    /// When this returns `false`, the arena's `document` field still holds
    /// the last successful document for rendering purposes, but the `world`
    /// reflects the current (erroring) source. Callers that need a coherent
    /// world+document pair (e.g. jump-to-source, jump-from-preview) should
    /// check this and return a degraded result rather than acting on the
    /// mismatched pair.
    pub fn last_compile_succeeded(&self) -> bool {
        self.current
            .read()
            .expect("RwLock poisoned")
            .last_compile_succeeded
    }

    /// Access document with function (uses read lock)
    pub fn with_document<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&TypesetterWorld, Option<&PagedDocument>) -> R,
    {
        let slot = self.current.read().expect("RwLock poisoned");
        f(&slot.world, slot.document.as_ref().map(|d| &d.0))
    }

    /// Compile and update document, minimizing write lock duration
    pub fn compile_and_update(
        &self,
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> PreviewResult {
        #[cfg(debug_assertions)]
        println!("\n=== COMPILE START ===");

        // Clone the world for compilation
        let mut compilation_world = {
            let slot = self.current.read().expect("RwLock poisoned");
            slot.world.clone()
        };

        #[cfg(debug_assertions)]
        let file_count_before = compilation_world.files.lock().unwrap().len();

        #[cfg(debug_assertions)]
        println!("Compiling with {} files in cache", file_count_before);

        // Update world state
        compilation_world.update(path, project_root, source);

        // Compile
        let compilation = typst::compile::<PagedDocument>(&compilation_world);

        // Store diagnostics in the compilation world
        {
            let mut diag_items = compilation.warnings.clone();
            if let Err(ref errs) = compilation.output {
                diag_items.extend(errs.clone());
            }
            compilation_world.set_diagnostics(diag_items);
        }

        // Collect diagnostics for the result
        let diagnostics = compilation_world
            .diagnostics()
            .iter()
            .filter(|d| {
                matches!(
                    d.severity,
                    typst_library::diag::Severity::Error | typst_library::diag::Severity::Warning
                )
            })
            .map(|d| PreviewDiagnostic::new(d, &compilation_world))
            .collect();

        // Collect dependencies before post_compile_cleanup strips the file cache
        let dependencies = compilation_world.accessed_files();

        // Prepare the result based on compilation output
        let (new_document, compile_succeeded, mut result) = match compilation.output {
            Ok(doc) => {
                let metadata: Vec<PageMetadata> = doc
                    .pages()
                    .iter()
                    .enumerate()
                    .map(|(i, page)| PageMetadata {
                        page_number: i,
                        dimensions: (
                            page.frame.width().to_pt() as f32,
                            page.frame.height().to_pt() as f32,
                        ),
                    })
                    .collect();

                #[cfg(debug_assertions)]
                println!("Compilation successful, {} pages", metadata.len());

                let result = PreviewResult {
                    generation: 0,
                    page_metadata: metadata,
                    diagnostics,
                    dependencies,
                };

                (Some(TrackedDocument::new(doc)), true, result)
            }
            Err(_) => {
                #[cfg(debug_assertions)]
                println!("Compilation failed, preserving last successful document");

                let result = PreviewResult {
                    generation: 0,
                    page_metadata: Vec::new(),
                    diagnostics,
                    dependencies,
                };

                (None, false, result)
            }
        };

        // Memory management (must come after accessed_files() is called above)
        compilation_world.post_compile_cleanup();

        #[cfg(debug_assertions)]
        {
            compilation_world.diagnose_memory();
            let file_count_after = compilation_world.files.lock().unwrap().len();
            println!(
                "After cleanup: {} files in cache (was {})",
                file_count_after, file_count_before
            );
            if file_count_after > file_count_before {
                println!(
                    "WARNING: File cache grew by {} files!",
                    file_count_after - file_count_before
                );
            }
        }

        // Acquire write lock to swap in the new world and, on success, the new
        // document. On failure the old TrackedDocument (if any) is left in place
        // so the rendering worker can continue serving the last known-good layout.
        {
            let mut slot = self.current.write().expect("RwLock poisoned");

            slot.world = compilation_world;

            if compile_succeeded {
                // Replace the document with the freshly compiled one. The old
                // TrackedDocument is dropped here, firing its Drop impl.
                #[cfg(debug_assertions)]
                if let Some(ref old_doc) = slot.document {
                    println!(
                        "Replacing old document with {} pages",
                        old_doc.0.pages().len()
                    );
                }
                slot.document = new_document;
                slot.last_compile_succeeded = true;
            } else {
                // Leave slot.document untouched — keep the last successful
                // document available for rendering even though the current
                // source has errors.
                slot.last_compile_succeeded = false;

                #[cfg(debug_assertions)]
                {
                    match &slot.document {
                        Some(doc) => println!(
                            "Compile failed — retaining stale document ({} pages) for rendering",
                            doc.0.pages().len()
                        ),
                        None => println!("Compile failed — no prior document to retain"),
                    }
                }
            }

            let new_gen = self
                .generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                + 1;
            slot.generation = new_gen;
            result.generation = new_gen;

            #[cfg(debug_assertions)]
            println!("Arena: Updated to generation {}", new_gen);
        }

        #[cfg(debug_assertions)]
        println!("=== COMPILE END ===\n");

        result
    }

    /// Reset world (for document switching)
    pub fn reset(&self) {
        let mut slot = self.current.write().expect("RwLock poisoned");

        #[cfg(debug_assertions)]
        if let Some(ref doc) = slot.document {
            println!(
                "Arena: Resetting, dropping document with {} pages",
                doc.0.pages().len()
            );
        }

        // Reuse shared resources (fonts, packages, library) to avoid expensive font scanning
        let fonts = Some(Arc::clone(&slot.world.fonts));
        let package_storage = Some(Arc::clone(&slot.world.package_storage));
        let library = Some(Arc::clone(&slot.world.library));

        slot.world = TypesetterWorld::with_shared_resources(fonts, package_storage, None, library);
        slot.document = None;
        slot.last_compile_succeeded = false;

        let new_gen = self
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1;
        slot.generation = new_gen;

        #[cfg(debug_assertions)]
        println!("Arena: Reset to generation {}", new_gen);
    }
}

#[derive(Clone, Debug)]
pub struct TypstSystem {
    arena: Arc<DocumentArena>,
    compilation_worker: CompilationWorker,
    rendering_worker: RenderingWorker,
    ide_worker: IdeWorker,
}

impl TypstSystem {
    pub fn new() -> Self {
        let arena = Arc::new(DocumentArena::new());

        let compilation_worker = CompilationWorker::new(Arc::clone(&arena));
        let rendering_worker = RenderingWorker::new(Arc::clone(&arena));
        let ide_worker = IdeWorker::new(Arc::clone(&arena));

        Self {
            arena,
            compilation_worker,
            rendering_worker,
            ide_worker,
        }
    }

    /// Request a preview: compile and return metadata
    pub async fn request_preview(
        &self,
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Result<PreviewResult, CompilerError> {
        self.compilation_worker
            .compile_for_preview(source, path, project_root)
            .await
    }

    /// Render a specific page on demand at the given scale (physical
    /// pixels per Typst point) for a specific document generation.
    pub async fn render_page(
        &self,
        generation: u64,
        page_number: usize,
        pixel_per_pt: f64,
    ) -> RenderResponse {
        self.rendering_worker
            .render_page(generation, page_number, pixel_per_pt)
            .await
    }

    /// Export to PDF
    pub async fn request_pdf(
        &self,
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
        pdf_standard: String,
    ) -> Result<Option<Bytes>, CompilerError> {
        self.compilation_worker
            .compile_pdf(source, path, project_root, pdf_standard)
            .await
    }

    /// Export to HTML
    pub async fn request_html(
        &self,
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Result<Option<Bytes>, CompilerError> {
        self.compilation_worker
            .compile_html(source, path, project_root)
            .await
    }

    /// Export to Markdown
    pub async fn request_markdown(
        &self,
        source: String,
        path: Option<PathBuf>,
        project_root: Option<PathBuf>,
    ) -> Result<Option<String>, CompilerError> {
        self.compilation_worker
            .compile_markdown(source, path, project_root)
            .await
    }

    /// Jump from preview to source
    pub async fn jump_from_preview(
        &self,
        page: usize,
        x: f32,
        y: f32,
    ) -> Result<JumpResult, CompilerError> {
        self.ide_worker.jump_from_preview(page, x, y).await
    }

    /// Jump from source to preview
    pub async fn jump_from_source(
        &self,
        cursor_offset: usize,
    ) -> Result<JumpResult, CompilerError> {
        self.ide_worker.jump_from_source(cursor_offset).await
    }

    /// Clear all cached data to free memory (for document switching)
    pub fn reset_world(&self) {
        self.arena.reset();
    }

    /// Request autocomplete suggestions at the given cursor offset
    pub async fn request_autocomplete(
        &self,
        cursor_offset: usize,
        explicit: bool,
    ) -> Result<Vec<typst_ide::Completion>, CompilerError> {
        self.ide_worker.autocomplete(cursor_offset, explicit).await
    }

    /// Request document statistics including word and page count
    pub async fn request_document_stats(&self) -> Result<DocumentStats, CompilerError> {
        self.ide_worker.document_stats().await
    }

    /// Request the document outline (headings with source positions)
    pub async fn request_document_outline(&self) -> Result<Vec<DocumentHeading>, CompilerError> {
        self.ide_worker.document_outline().await
    }

    /// Request hover tooltip at the given cursor offset
    pub async fn request_hover(
        &self,
        cursor_offset: usize,
    ) -> Result<Option<HoverResult>, CompilerError> {
        self.ide_worker.hover(cursor_offset).await
    }

    /// Debug method to report memory state
    #[cfg(debug_assertions)]
    pub fn debug_memory_state(&self) {
        self.arena.with_document(|world, doc_opt| {
            let file_count = world.files.lock().unwrap().len();

            println!("\nTYPST_SYSTEM MEMORY STATE:");

            if let Some(doc) = doc_opt {
                println!("  Cached document: YES ({} pages)", doc.pages().len());
            } else {
                println!("  Cached document: NO");
            }

            println!("  Cached files: {}", file_count);
            println!("  Arena generation: {}", self.arena.current_generation());
            println!(
                "  Arena Arc strong count: {}",
                Arc::strong_count(&self.arena)
            );
            println!(
                "  Last compile succeeded: {}",
                self.arena.last_compile_succeeded()
            );
        });
    }

    /// Returns true if a document has been successfully compiled and is available
    pub fn has_document(&self) -> bool {
        self.arena.with_document(|_, doc_opt| doc_opt.is_some())
    }

    /// Returns whether the most recent compilation succeeded
    pub fn arena_last_compile_succeeded(&self) -> bool {
        self.arena.last_compile_succeeded()
    }

    /// Immediately cancel all workers without blocking
    pub fn cancel_all(&self) {
        self.compilation_worker.cancel();
        self.rendering_worker.cancel();
        self.ide_worker.cancel();
    }
}

impl Default for TypstSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TypstSystem {
    fn drop(&mut self) {
        self.cancel_all();

        #[cfg(debug_assertions)]
        {
            self.arena.with_document(|_world, doc_opt| {
                if doc_opt.is_some() {
                    println!("TypstSystem: Dropping with document still cached");
                }
            });
            println!("TypstSystem: Dropped");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_baseline_pipeline_measurements() {
        let system = TypstSystem::new();
        let runtime = crate::application::TypesetterApplication::tokio_runtime();

        runtime.block_on(async {
            // Scenario 1: 1-Page Document
            let doc_1_source = "= Title\nThis is a simple one-page document with some text.".to_string();
            let t0 = Instant::now();
            let res_1 = system.request_preview(doc_1_source, None, None).await.expect("compile 1-page");
            let t2 = t0.elapsed();
            assert_eq!(res_1.page_metadata.len(), 1);

            let t_render_start = Instant::now();
            let render_p0 = system.render_page(res_1.generation, 0, 1.5).await;
            let t3 = t_render_start.elapsed();
            assert!(matches!(render_p0, RenderResponse::PageImage(_)));

            println!("\n=== BASELINE METRICS: 1-Page Document ===");
            println!("T2 (Compilation time): {:?}", t2);
            println!("T3 (Page 0 render duration): {:?}", t3);
            println!("Total T5 (Compile + Page 0 render): {:?}", t2 + t3);

            // Scenario 2: 5-Page Document
            let mut doc_5_source = String::new();
            for i in 1..=5 {
                doc_5_source.push_str(&format!(
                    "= Page {}\nSome heading and content.\n#table(columns: 2, [Cell 1], [Cell 2])\n",
                    i
                ));
                if i < 5 {
                    doc_5_source.push_str("#pagebreak()\n");
                }
            }
            system.arena.reset();

            let t0_5 = Instant::now();
            let res_5 = system.request_preview(doc_5_source, None, None).await.expect("compile 5-page");
            let t2_5 = t0_5.elapsed();
            assert_eq!(res_5.page_metadata.len(), 5);

            let mut page_durations = Vec::new();
            let t_all_start = Instant::now();
            for page_num in 0..res_5.page_metadata.len() {
                let p_start = Instant::now();
                let render_resp = system.render_page(res_5.generation, page_num, 1.5).await;
                let dur = p_start.elapsed();
                assert!(matches!(render_resp, RenderResponse::PageImage(_)));
                page_durations.push(dur);
            }
            let t6_all = t_all_start.elapsed();

            println!("\n=== BASELINE METRICS: 5-Page Document ===");
            println!("T2 (Compilation time): {:?}", t2_5);
            println!("T3 (Page 0 render duration): {:?}", page_durations[0]);
            for (idx, dur) in page_durations.iter().enumerate() {
                println!("  Page {} render duration: {:?}", idx, dur);
            }
            println!("T6 (All 5 pages render duration): {:?}", t6_all);
            println!("Total T6 from start: {:?}", t2_5 + t6_all);

            // Scenario 3: 15-Page Document
            let mut doc_15_source = String::new();
            for i in 1..=15 {
                doc_15_source.push_str(&format!(
                    "= Section {}\nParagraph text with math $x^2 + y^2 = z^2$ and tables.\n#table(columns: 3, [A], [B], [C])\n",
                    i
                ));
                if i < 15 {
                    doc_15_source.push_str("#pagebreak()\n");
                }
            }
            system.arena.reset();

            let t0_15 = Instant::now();
            let res_15 = system.request_preview(doc_15_source, None, None).await.expect("compile 15-page");
            let t2_15 = t0_15.elapsed();
            assert_eq!(res_15.page_metadata.len(), 15);

            let p0_start = Instant::now();
            let p0_resp = system.render_page(res_15.generation, 0, 1.5).await;
            let t3_15 = p0_start.elapsed();
            assert!(matches!(p0_resp, RenderResponse::PageImage(_)));

            println!("\n=== BASELINE METRICS: 15-Page Document ===");
            println!("T2 (Compilation time): {:?}", t2_15);
            println!("T3 (Page 0 render duration): {:?}", t3_15);
            println!("First page ready (Compile + Page 0): {:?}", t2_15 + t3_15);
        });
    }

    #[test]
    fn test_render_worker_generation_cancellation() {
        let system = TypstSystem::new();
        let runtime = crate::application::TypesetterApplication::tokio_runtime();

        runtime.block_on(async {
            let doc_source = "= Test Document\nSome content".to_string();
            let res = system.request_preview(doc_source, None, None).await.expect("compile");
            let generation = res.generation;

            // Render with matching generation succeeds
            let resp_valid = system.render_page(generation, 0, 1.0).await;
            assert!(matches!(resp_valid, RenderResponse::PageImage(_)));

            // Render with obsolete generation is cancelled immediately
            let resp_obsolete = system.render_page(generation - 1, 0, 1.0).await;
            assert!(matches!(resp_obsolete, RenderResponse::Cancelled));
        });
    }

    #[test]
    fn test_rapid_document_switch_cancellation() {
        let system = TypstSystem::new();
        let runtime = crate::application::TypesetterApplication::tokio_runtime();

        runtime.block_on(async {
            // Document A
            let res_a = system.request_preview("= Doc A".to_string(), None, None).await.expect("compile A");
            let gen_a = res_a.generation;

            // Rapid document switch: arena is reset for Document B
            system.arena.reset();
            let res_b = system.request_preview("= Doc B".to_string(), None, None).await.expect("compile B");
            let gen_b = res_b.generation;
            assert!(gen_b > gen_a);

            // Any render request with gen_a is cancelled
            let resp_a = system.render_page(gen_a, 0, 1.0).await;
            assert!(matches!(resp_a, RenderResponse::Cancelled));

            // Render request with gen_b succeeds
            let resp_b = system.render_page(gen_b, 0, 1.0).await;
            assert!(matches!(resp_b, RenderResponse::PageImage(_)));
        });
    }

    #[test]
    fn test_rendering_semaphore_concurrency_and_permit_release() {
        let system = TypstSystem::new();
        let runtime = crate::application::TypesetterApplication::tokio_runtime();

        runtime.block_on(async {
            let doc_source = "= Page 1\n#pagebreak()\n= Page 2\n#pagebreak()\n= Page 3\n#pagebreak()\n= Page 4\n#pagebreak()\n= Page 5".to_string();
            let res = system.request_preview(doc_source, None, None).await.expect("compile");
            let generation = res.generation;

            // Run 5 concurrent renders (worker semaphore limits concurrent execution to 2 permits)
            let (r0, r1, r2, r3, r4) = tokio::join!(
                system.render_page(generation, 0, 1.0),
                system.render_page(generation, 1, 1.0),
                system.render_page(generation, 2, 1.0),
                system.render_page(generation, 3, 1.0),
                system.render_page(generation, 4, 1.0),
            );

            assert!(matches!(r0, RenderResponse::PageImage(_)));
            assert!(matches!(r1, RenderResponse::PageImage(_)));
            assert!(matches!(r2, RenderResponse::PageImage(_)));
            assert!(matches!(r3, RenderResponse::PageImage(_)));
            assert!(matches!(r4, RenderResponse::PageImage(_)));

            // Verify permits were cleanly released by running a subsequent render job
            let sixth = system.render_page(generation, 0, 1.0).await;
            assert!(matches!(sixth, RenderResponse::PageImage(_)));
        });
    }

    #[test]
    fn test_rendering_cancellation_releases_semaphore_permits() {
        let system = TypstSystem::new();
        let runtime = crate::application::TypesetterApplication::tokio_runtime();

        runtime.block_on(async {
            let doc_source = "= Page 1\n#pagebreak()\n= Page 2".to_string();
            let res = system.request_preview(doc_source, None, None).await.expect("compile");
            let generation = res.generation;

            // Dispatch 5 obsolete render jobs that get cancelled (pre-render or post-render)
            let (c0, c1, c2, c3, c4) = tokio::join!(
                system.render_page(generation - 1, 0, 1.0),
                system.render_page(generation - 1, 0, 1.0),
                system.render_page(generation - 1, 0, 1.0),
                system.render_page(generation - 1, 0, 1.0),
                system.render_page(generation - 1, 0, 1.0),
            );

            assert!(matches!(c0, RenderResponse::Cancelled));
            assert!(matches!(c1, RenderResponse::Cancelled));
            assert!(matches!(c2, RenderResponse::Cancelled));
            assert!(matches!(c3, RenderResponse::Cancelled));
            assert!(matches!(c4, RenderResponse::Cancelled));

            // Verify that all permits were released upon cancellation so valid jobs proceed immediately
            let valid = system.render_page(generation, 0, 1.0).await;
            assert!(matches!(valid, RenderResponse::PageImage(_)));
        });
    }

    #[test]
    fn test_rendering_async_task_cancellation_preserves_permit_ownership() {
        let system = TypstSystem::new();
        let runtime = crate::application::TypesetterApplication::tokio_runtime();

        runtime.block_on(async {
            let doc_source = "= Page 1\n#pagebreak()\n= Page 2\n#pagebreak()\n= Page 3".to_string();
            let res = system.request_preview(doc_source, None, None).await.expect("compile");
            let generation = res.generation;

            // Start rendering, but drop/cancel the asynchronous future while the blocking render is executing
            let render_fut = system.render_page(generation, 0, 1.0);
            tokio::select! {
                _ = render_fut => {}
                _ = async {
                    // Yield to ensure message was received by worker and permit acquired
                    tokio::task::yield_now().await;
                } => {
                    // render_fut dropped here, disconnecting the oneshot channel while blocking worker is running
                }
            }

            // Wait a brief duration for the blocking thread to finish and drop its OwnedSemaphorePermit
            std::thread::sleep(std::time::Duration::from_millis(60));

            // Verify that subsequent render requests acquire permits cleanly without deadlock
            let next = system.render_page(generation, 0, 1.0).await;
            assert!(matches!(next, RenderResponse::PageImage(_)));
        });
    }

    #[test]
    fn test_rapid_document_switch_three_documents_latest_wins() {
        let system = TypstSystem::new();
        let runtime = crate::application::TypesetterApplication::tokio_runtime();

        runtime.block_on(async {
            // Rapid document switch: Document A, B, and C
            let res_a = system.request_preview("= Document A".to_string(), None, None).await.expect("compile A");
            let gen_a = res_a.generation;

            system.arena.reset();
            let res_b = system.request_preview("= Document B".to_string(), None, None).await.expect("compile B");
            let gen_b = res_b.generation;

            system.arena.reset();
            let res_c = system.request_preview("= Document C".to_string(), None, None).await.expect("compile C");
            let gen_c = res_c.generation;

            assert!(gen_b > gen_a);
            assert!(gen_c > gen_b);

            // Obsolete jobs for A and B are cancelled
            let (resp_a, resp_b, resp_c) = tokio::join!(
                system.render_page(gen_a, 0, 1.0),
                system.render_page(gen_b, 0, 1.0),
                system.render_page(gen_c, 0, 1.0),
            );

            assert!(matches!(resp_a, RenderResponse::Cancelled));
            assert!(matches!(resp_b, RenderResponse::Cancelled));
            assert!(matches!(resp_c, RenderResponse::PageImage(_)));
        });
    }
}
