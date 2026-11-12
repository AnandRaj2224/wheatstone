// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt::Write as _;

use gtk::prelude::*;

use crate::config::{TYPST_VERSION, VERSION};

pub fn build_debug_info() -> String {
    let mut info = String::new();

    let _ = writeln!(info, "Typesetter: {VERSION}");
    let _ = writeln!(info, "Build: {}", build_profile());
    let _ = writeln!(info);

    let _ = writeln!(info, "Typst: {TYPST_VERSION}");
    let _ = writeln!(info);

    let _ = writeln!(
        info,
        "GTK: {}.{}.{}",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version()
    );
    let _ = writeln!(
        info,
        "Libadwaita: {}.{}.{}",
        adw::major_version(),
        adw::minor_version(),
        adw::micro_version()
    );
    let _ = writeln!(info);

    let _ = writeln!(info, "OS: {}", os_pretty_name());
    let _ = writeln!(info, "Kernel: {}", kernel_version());
    let _ = writeln!(info, "Architecture: {}", std::env::consts::ARCH);
    let _ = writeln!(info, "CPU cores: {}", cpu_count());
    let _ = writeln!(info, "Memory: {}", total_memory());
    let _ = writeln!(info);

    if let Some(desktop) = non_empty_env("XDG_CURRENT_DESKTOP") {
        let _ = writeln!(info, "Desktop: {desktop}");
    }
    let _ = writeln!(
        info,
        "Locale: {}",
        non_empty_env("LANG").unwrap_or_else(|| "unknown".into())
    );
    let _ = writeln!(info, "Windowing system: {}", windowing_system());
    let _ = writeln!(info);

    let _ = writeln!(info, "Monitors:");
    for monitor in monitors() {
        let _ = writeln!(info, "  {monitor}");
    }
    let _ = writeln!(info);

    let overrides = environment_overrides();
    if !overrides.is_empty() {
        let _ = writeln!(info, "Environment overrides: {}", overrides.join(", "));
    }

    info
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

fn build_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "Debug"
    } else {
        "Release"
    }
}

fn os_pretty_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                line.strip_prefix("PRETTY_NAME=")
                    .map(|value| value.trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| std::env::consts::OS.to_string())
}

fn kernel_version() -> String {
    std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|version| version.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn cpu_count() -> String {
    std::thread::available_parallelism()
        .map(|count| count.get().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn total_memory() -> String {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                line.strip_prefix("MemTotal:").map(|rest| {
                    let kib: u64 = rest.trim().trim_end_matches(" kB").parse().unwrap_or(0);
                    format!("{:.1} GiB", kib as f64 / 1024.0 / 1024.0)
                })
            })
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn windowing_system() -> String {
    if non_empty_env("WAYLAND_DISPLAY").is_some() {
        "Wayland".to_string()
    } else if let Some(session_type) = non_empty_env("XDG_SESSION_TYPE") {
        session_type
    } else if non_empty_env("DISPLAY").is_some() {
        "X11".to_string()
    } else {
        "unknown".to_string()
    }
}

fn monitors() -> Vec<String> {
    let Some(display) = gtk::gdk::Display::default() else {
        return vec!["unknown".to_string()];
    };

    let monitors = display.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitors.item(i))
        .filter_map(|item| item.downcast::<gtk::gdk::Monitor>().ok())
        .map(|monitor| {
            let geometry = monitor.geometry();
            format!(
                "{}x{} @ {}x",
                geometry.width(),
                geometry.height(),
                monitor.scale_factor()
            )
        })
        .collect()
}

fn environment_overrides() -> Vec<String> {
    [
        "GDK_BACKEND",
        "GSK_RENDERER",
        "GTK_THEME",
        "LIBGL_ALWAYS_SOFTWARE",
    ]
    .iter()
    .filter_map(|key| non_empty_env(key).map(|value| format!("{key}={value}")))
    .collect()
}
