// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::{prelude::*, subclass::prelude::*};
use gtk::glib;
use typst_library::diag::Severity;

use crate::typst_system::PreviewDiagnostic;

use super::{PreviewObject, TypesetterPreview};

impl TypesetterPreview {
    /// Check if diagnostics contain file access errors that indicate the flatpak sandbox is blocking file access
    fn has_file_access_errors(diagnostics: &[PreviewDiagnostic]) -> bool {
        diagnostics.iter().any(|d| d.is_file_access_error())
    }

    pub(super) fn handle_compilation_result(
        &self,
        text: String,
        current_file_path: Option<std::path::PathBuf>,
        project_root: Option<std::path::PathBuf>,
    ) {
        let Some(window) = self.find_parent_window() else {
            return;
        };

        window.start_progress_pulse();

        let self_clone = self.clone();

        glib::spawn_future_local(async move {
            let Some(window) = self_clone.find_parent_window() else {
                return;
            };

            // Get the compiler system
            let compiler = match window.compiler_system().await {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("TypesetterPreview: Failed to get compiler system: {}", e);
                    self_clone.hide_loading();
                    window.stop_progress_pulse();
                    return;
                }
            };

            // Single request, single response
            match compiler
                .request_preview(text, current_file_path, project_root)
                .await
            {
                Ok(result) => {
                    // Update dependency monitors with files accessed during this compilation
                    if let Some(m) = window.imp().dependency_monitor.borrow_mut().as_mut() {
                        m.update(result.dependencies.clone())
                    }

                    let imp = self_clone.imp();
                    if result.generation < imp.current_generation.get() {
                        #[cfg(debug_assertions)]
                        println!(
                            "TypesetterPreview: Discarding obsolete compilation result (gen {} < current {})",
                            result.generation,
                            imp.current_generation.get()
                        );
                        window.stop_progress_pulse();
                        return;
                    }
                    imp.current_generation.set(result.generation);

                    let page_count = result.page_metadata.len();

                    // Count errors and warnings for badge
                    let error_count = result
                        .diagnostics
                        .iter()
                        .filter(|d| matches!(d.severity, Severity::Error))
                        .count() as u32;
                    let warning_count = result
                        .diagnostics
                        .iter()
                        .filter(|d| matches!(d.severity, Severity::Warning))
                        .count() as u32;

                    window.update_preview_error_badge(error_count, warning_count);

                    // Check for file access errors and show/hide banner
                    if std::env::var("FLATPAK_ID").is_ok()
                        && Self::has_file_access_errors(&result.diagnostics)
                        && window.project_root().is_none()
                    {
                        window.imp().project_folder_banner.set_revealed(true);
                    } else {
                        window.imp().project_folder_banner.set_revealed(false);
                    }

                    if let Some(buffer) = self_clone.find_editor_buffer() {
                        let highlight_ranges: Vec<(usize, usize, Severity)> = result
                            .diagnostics
                            .iter()
                            .filter_map(|d| {
                                d.location_range
                                    .map(|(start, end)| (start, end, d.severity))
                            })
                            .collect();
                        Self::highlight_ranges(&buffer, &highlight_ranges);
                    }

                    // Update diagnostic hover lints in the editor
                    if let Some(editor) = self_clone
                        .find_parent_window()
                        .map(|w| w.imp().editor_container.get())
                    {
                        editor.update_diagnostic_lints(&result.diagnostics);
                    }

                    // Always update page store with metadata
                    // Handle empty result (compilation error)
                    if page_count == 0 {
                        self_clone.hide_loading();
                        self_clone.populate_diagnostics(&result.diagnostics);
                        imp.preview_pane
                            .set_reveal_bottom_bar(!result.diagnostics.is_empty());
                        imp.preview_pane.set_open(true);
                        imp.preview_pane.set_can_close(false);
                    } else {
                        self_clone.set_warning_count(warning_count);
                        imp.preview_pane.set_can_close(true);
                        let has_errors = result
                            .diagnostics
                            .iter()
                            .any(|d| matches!(d.severity, typst_library::diag::Severity::Error));
                        let sheet_was_open = imp.preview_pane.is_open(); // capture before set_open
                        if !has_errors {
                            imp.preview_pane.set_open(false);
                        }
                        imp.preview_pane
                            .set_reveal_bottom_bar(!result.diagnostics.is_empty());
                        if sheet_was_open && !has_errors {
                            // Sheet is animating closed, wait for it before repopulating
                            glib::timeout_add_local_once(
                                std::time::Duration::from_millis(400),
                                glib::clone!(
                                    #[weak(rename_to = preview)]
                                    self_clone,
                                    move || {
                                        preview.populate_diagnostics(&result.diagnostics);
                                    }
                                ),
                            );
                        } else {
                            self_clone.populate_diagnostics(&result.diagnostics);
                        }
                        let current_count = imp.page_store.n_items() as usize;

                        // Clean up excess pages if new count is smaller
                        if page_count < current_count {
                            // Remove textures from cache for pages being removed
                            for page_num in page_count..current_count {
                                imp.texture_cache.remove(result.generation, page_num);
                            }

                            let pages_to_remove: Vec<_> = ((page_count as u32)
                                ..current_count as u32)
                                .filter_map(|i| imp.page_store.item(i))
                                .filter_map(|obj| obj.downcast::<PreviewObject>().ok())
                                .collect();

                            for i in ((page_count as u32)..current_count as u32).rev() {
                                imp.page_store.remove(i);
                            }

                            glib::idle_add_local_once(move || {
                                for page in pages_to_remove {
                                    page.cleanup();
                                }
                            });
                        }

                        // Clear cache for previous generation textures to free memory
                        imp.texture_cache.clear();

                        // Clear textures from non-visible pages to free memory
                        let visible = imp.visible_pages.borrow().clone();
                        for i in 0..imp.page_store.n_items() {
                            let page_num = i as usize;
                            if !visible.contains(&page_num)
                                && let Some(obj) = imp.page_store.item(i)
                                && let Ok(page) = obj.downcast::<PreviewObject>()
                            {
                                // Clear texture from non-visible pages
                                if page.has_texture() {
                                    page.clear_texture();
                                }
                            }
                        }

                        // Create/update PreviewObjects with metadata
                        for metadata in result.page_metadata {
                            let pos = metadata.page_number;

                            if pos < current_count {
                                // Check if we need to update this object
                                if let Some(existing_obj) = imp.page_store.item(pos as u32)
                                    && let Ok(existing_page) =
                                        existing_obj.downcast::<PreviewObject>()
                                {
                                    // Check if dimensions changed
                                    let old_dims = existing_page.dimensions();
                                    let new_dims = metadata.dimensions;

                                    if old_dims != new_dims {
                                        // Create new object
                                        let obj = PreviewObject::new(&metadata, result.generation);
                                        imp.page_store.splice(pos as u32, 1, &[obj]);

                                        // Cleanup the old page after a brief delay to ensure unbind completes
                                        glib::idle_add_local_once(move || {
                                            existing_page.cleanup();
                                        });
                                    }
                                    // If dimensions are the same, keep old texture and mark for re-render
                                    else {
                                        // Keep the old texture visible (no clear!)
                                        // Update generation and mark that it needs updating
                                        existing_page.set_generation(result.generation);
                                        existing_page.set_needs_update(true);
                                    }
                                }
                            } else {
                                // Append new pages
                                let obj = PreviewObject::new(&metadata, result.generation);
                                imp.page_store.append(&obj);
                            }
                        }

                        // Page dimensions may have changed (new widest page,
                        // or the previously-widest page shrank/was removed).
                        // This keeps the fit-to-viewport scale current and
                        // resizes pages immediately if we're actually in
                        // automatic mode right now. In manual zoom mode it's
                        // a no-op. Manual zoom deliberately ignores this
                        // until the user hits reset. If the scale did change,
                        // the texture cache needs invalidating too, not
                        // debounced here, a compile is already a naturally-paced
                        // event, not a rapid resize/zoom burst.
                        let scale_changed = self_clone.refresh_auto_fit_scale();
                        if scale_changed {
                            self_clone.settle_render_scale();
                        }

                        // Prioritize first page rendering, then remaining visible pages
                        if self_clone.is_visible() {
                            self_clone.render_first_page_then_visible(result.generation);
                        } else {
                            // If preview is hidden when compilation finishes, render page 0
                            // in background so texture is ready immediately when preview is toggled
                            self_clone.render_page_async(result.generation, 0);
                        }
                    }

                    window.stop_progress_pulse();
                }
                Err(e) => {
                    eprintln!("TypesetterPreview: Compilation failed: {}", e);

                    self_clone.hide_loading();

                    // Show error badge on compilation failure
                    window.update_preview_error_badge(1, 0);

                    window.stop_progress_pulse();
                }
            }
        });
    }

    pub(super) fn populate_diagnostics(&self, diagnostics: &[PreviewDiagnostic]) {
        let imp = self.imp();

        // Clear all existing rows at once
        imp.diagnostics_rows.borrow_mut().clear();
        imp.diagnostics_list.remove_all();

        // Create new rows and append them to the ListBox
        for diag in diagnostics {
            let row = adw::ActionRow::new();

            // Prefix icon
            let icon = gtk::Image::new();
            icon.set_pixel_size(16);
            let icon_name = match diag.severity {
                Severity::Error => "dialog-error-symbolic",
                Severity::Warning => "dialog-warning-symbolic",
            };
            icon.set_icon_name(Some(icon_name));
            row.add_prefix(&icon);

            // Title/subtitle
            row.set_title(&crate::utils::capitalize_first_letter(&diag.message));
            if let Some(filename) = &diag.filename {
                row.set_subtitle(filename);
            }

            // Set tooltip from hints if any
            if !diag.hints.is_empty() {
                let tooltip = diag.hints.join("\n");
                row.set_tooltip_text(Some(&tooltip));
            }

            // Connect activation signal
            // Note: location_range contains character offsets (already converted in PreviewDiagnostic::compute_range)
            if let Some((_start, end)) = diag.location_range {
                row.set_activatable(true);
                row.connect_activated(glib::clone! {
                    #[weak(rename_to=this)] self,
                    move |_| {
                        if let (Some(buffer), Some(view)) = (this.find_editor_buffer(), this.find_view()) {
                            let mut iter = buffer.iter_at_offset(end as i32);
                            buffer.place_cursor(&iter);
                            let fraction = view.scroll_fraction();
                            view.scroll_to_iter(&mut iter, 0.0, true, 0.0, fraction);
                            view.grab_focus();
                        }
                    }
                });
            }

            // Append to the GtkListBox and keep a clone in the vector
            imp.diagnostics_list.append(&row);
            imp.diagnostics_rows
                .borrow_mut()
                .push(row.clone().upcast::<gtk::Widget>());
        }
    }

    fn highlight_ranges(buffer: &sourceview::Buffer, ranges: &[(usize, usize, Severity)]) {
        for tag_name in ["error", "warning"] {
            buffer.remove_tag_by_name(tag_name, &buffer.start_iter(), &buffer.end_iter());
        }
        for (start, end, severity) in ranges {
            let tag_name = match severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            // Note: ranges contain character offsets (already converted in PreviewDiagnostic::compute_range)
            let start_iter = buffer.iter_at_offset(*start as i32);
            let end_iter = buffer.iter_at_offset(*end as i32);
            buffer.apply_tag_by_name(tag_name, &start_iter, &end_iter);
        }
    }

    pub fn jump_from_preview(&self, page_number: usize, x: f32, y: f32) {
        self.jump_from_preview_with_callback(page_number, x, y, |_| {});
    }

    pub fn jump_from_preview_with_callback<F>(
        &self,
        page_number: usize,
        x: f32,
        y: f32,
        callback: F,
    ) where
        F: FnOnce(crate::typst_system::JumpResult) + 'static,
    {
        let self_clone = self.clone();
        glib::spawn_future_local(async move {
            let Some(window) = self_clone.find_parent_window() else {
                return;
            };

            // Get the compiler system
            let compiler = match window.compiler_system().await {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("TypesetterPreview: Failed to get compiler system: {}", e);
                    self_clone.hide_loading();
                    window.stop_progress_pulse();
                    return;
                }
            };

            match compiler.jump_from_preview(page_number, x, y).await {
                Ok(jump_result) => {
                    match &jump_result {
                        crate::typst_system::JumpResult::Source(location) => {
                            self_clone.scroll_editor_to_location(location);
                        }
                        crate::typst_system::JumpResult::Url(url) => {
                            // Open the URL in a browser
                            gtk::UriLauncher::new(url).launch(
                                None::<&gtk::Window>,
                                gtk::gio::Cancellable::NONE,
                                |result| {
                                    if let Err(e) = result {
                                        eprintln!("TypesetterPreview: Failed to open URL: {}", e);
                                    }
                                },
                            );
                        }
                        crate::typst_system::JumpResult::None => {
                            #[cfg(debug_assertions)]
                            println!(
                                "TypesetterPreview: No source location found for click position"
                            );
                        }
                        _ => {
                            eprintln!("TypesetterPreview: Unexpected jump result type");
                        }
                    }

                    // Call the callback with the result
                    callback(jump_result);
                }
                Err(e) => {
                    eprintln!("TypesetterPreview: Jump from click failed: {}", e);
                }
            }
        });
    }

    fn scroll_editor_to_location(&self, location: &crate::typst_system::SourceLocation) {
        // Find the editor through the window
        let Some(window) = self.find_parent_window() else {
            return;
        };

        let editor = window.imp().editor_container.get();
        let buffer = editor.buffer();
        let view = editor.view();

        // Use the helper to convert byte offset to iter
        let iter = editor.byte_offset_to_iter(location.offset);
        buffer.place_cursor(&iter);

        let mut scroll_iter = iter;
        let fraction = view.scroll_fraction();
        view.scroll_to_iter(&mut scroll_iter, 0.0, true, 0.0, fraction);
        view.grab_focus();
    }
}
