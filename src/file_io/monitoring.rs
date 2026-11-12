// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::{FileExt, FileMonitorExt};
use gtk::gio;
use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

/// Manages GIO file monitors for a set of dependency paths.
/// Call `update` after each successful compilation with the new dependency set.
/// The provided callback is fired when any monitored file changes on disk,
/// debounced to prevent multiple recompiles from rapid successive events.
pub struct DependencyMonitor {
    monitors: HashMap<PathBuf, gio::FileMonitor>,
    callback: Rc<dyn Fn() + 'static>,
    debounce_id: Rc<RefCell<Option<gtk::glib::SourceId>>>,
}

impl DependencyMonitor {
    pub fn new(callback: impl Fn() + 'static) -> Self {
        Self {
            monitors: HashMap::new(),
            callback: Rc::new(callback),
            debounce_id: Rc::new(RefCell::new(None)),
        }
    }

    /// Update the set of monitored files, diffing against the current set.
    /// Adds monitors for new paths, removes monitors for dropped paths.
    pub fn update(&mut self, new_paths: Vec<PathBuf>) {
        let new_set: std::collections::HashSet<PathBuf> = new_paths.into_iter().collect();
        let old_set: std::collections::HashSet<PathBuf> = self.monitors.keys().cloned().collect();

        let to_remove: Vec<_> = old_set.difference(&new_set).cloned().collect();
        let to_add: Vec<_> = new_set.difference(&old_set).cloned().collect();

        #[cfg(debug_assertions)]
        {
            println!(
                "DependencyMonitor: Removing {} monitors: {:?}",
                to_remove.len(),
                to_remove
            );
            println!(
                "DependencyMonitor: Adding {} monitors: {:?}",
                to_add.len(),
                to_add
            );
        }

        // Remove monitors for paths that are no longer dependencies
        for path in &to_remove {
            if let Some(monitor) = self.monitors.remove(path) {
                monitor.cancel();
            }
        }

        // Add monitors for new paths
        for path in &to_add {
            self.add_monitor(path.clone());
        }

        #[cfg(debug_assertions)]
        println!(
            "DependencyMonitor: Now monitoring {} files",
            self.monitors.len()
        );
    }

    /// Cancel all monitors and clear state.
    pub fn clear(&mut self) {
        // Cancel any pending debounce timer
        if let Some(id) = self.debounce_id.borrow_mut().take() {
            id.remove();
        }

        // Cancel all file monitors
        for (_, monitor) in self.monitors.drain() {
            monitor.cancel();
        }
    }

    /// Cancel any pending debounce timer and schedule a new one.
    /// The callback will fire once after a short delay, collapsing
    /// multiple rapid file change events into a single recompile.
    fn schedule_debounced_callback(
        debounce_id: &Rc<RefCell<Option<gtk::glib::SourceId>>>,
        callback: &Rc<dyn Fn() + 'static>,
    ) {
        // Cancel any pending debounce
        if let Some(id) = debounce_id.borrow_mut().take() {
            id.remove();
        }

        let callback = Rc::clone(callback);
        let debounce_id_clone = Rc::clone(debounce_id);
        let id =
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                // Clear the stored id before firing
                debounce_id_clone.borrow_mut().take();
                callback();
            });

        debounce_id.borrow_mut().replace(id);
    }

    fn add_monitor(&mut self, path: PathBuf) {
        let file = gio::File::for_path(&path);
        let monitor = match file.monitor(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("DependencyMonitor: Failed to monitor {:?}: {}", path, e);
                return;
            }
        };

        let debounce_id = Rc::clone(&self.debounce_id);
        let callback = Rc::clone(&self.callback);
        monitor.connect_changed(move |_, _, _, event| {
            if matches!(
                event,
                gio::FileMonitorEvent::ChangesDoneHint | gio::FileMonitorEvent::Changed
            ) {
                Self::schedule_debounced_callback(&debounce_id, &callback);
            }
        });

        self.monitors.insert(path, monitor);
    }
}

impl Drop for DependencyMonitor {
    fn drop(&mut self) {
        self.clear();
    }
}

impl std::fmt::Debug for DependencyMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DependencyMonitor")
            .field("monitors", &self.monitors.keys().collect::<Vec<_>>())
            .finish()
    }
}
