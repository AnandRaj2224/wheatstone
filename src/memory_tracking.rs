// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::atomic::{AtomicUsize, Ordering};

pub struct MemoryCounters {
    pub textures_created: AtomicUsize,
    pub textures_dropped: AtomicUsize,
    pub documents_created: AtomicUsize,
    pub documents_dropped: AtomicUsize,
    pub worlds_created: AtomicUsize,
    pub worlds_dropped: AtomicUsize,
}

impl MemoryCounters {
    pub const fn new() -> Self {
        Self {
            textures_created: AtomicUsize::new(0),
            textures_dropped: AtomicUsize::new(0),
            documents_created: AtomicUsize::new(0),
            documents_dropped: AtomicUsize::new(0),
            worlds_created: AtomicUsize::new(0),
            worlds_dropped: AtomicUsize::new(0),
        }
    }

    pub fn report(&self) {
        let created = self.textures_created.load(Ordering::Relaxed);
        let dropped = self.textures_dropped.load(Ordering::Relaxed);
        let tex_live = created.saturating_sub(dropped);

        let doc_created = self.documents_created.load(Ordering::Relaxed);
        let doc_dropped = self.documents_dropped.load(Ordering::Relaxed);
        let doc_live = doc_created.saturating_sub(doc_dropped);

        let world_created = self.worlds_created.load(Ordering::Relaxed);
        let world_dropped = self.worlds_dropped.load(Ordering::Relaxed);
        let world_live = world_created.saturating_sub(world_dropped);

        println!("\nMEMORY TRACKING\n===============\n");
        println!(
            "Textures:  {} live ({} created, {} dropped)",
            tex_live, created, dropped
        );
        println!(
            "Documents: {} live ({} created, {} dropped)",
            doc_live, doc_created, doc_dropped
        );
        println!(
            "Worlds:    {} live ({} created, {} dropped)",
            world_live, world_created, world_dropped
        );

        #[cfg(target_os = "linux")]
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if line.starts_with("VmRSS:") {
                    let mem_str = format!("Process:   {}", line.trim());
                    println!("{:<58}\n", mem_str);
                    break;
                }
            }
        }

        let mut has_warnings = false;
        if tex_live > 20 {
            println!("WARNING: {} live textures is HIGH", tex_live);
            has_warnings = true;
        }
        if doc_live > 3 {
            println!("WARNING: {} live documents is HIGH (expected ≤3)", doc_live);
            has_warnings = true;
        }
        if world_live > 2 {
            println!("WARNING: {} live worlds is HIGH (expected ≤2)", world_live);
            has_warnings = true;
        }

        if !has_warnings {
            println!("All metrics within expected ranges");
        }
    }
}

pub static MEMORY_COUNTERS: MemoryCounters = MemoryCounters::new();
