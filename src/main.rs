// Copyright 2025 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

mod application;
mod config;
mod editor;
mod explorer;
mod file_io;
#[cfg(debug_assertions)]
mod memory_tracking;
mod migration;
mod outline;
mod package_manager;
mod preferences;
mod preview;
mod recents_menu;
mod statistics;
mod system_info;
mod templates_menu;
mod theme_selector;
mod typst_system;
mod utils;
mod window;

use self::application::TypesetterApplication;
use self::window::TypesetterWindow;

use config::{APP_ID, GETTEXT_PACKAGE, LOCALEDIR, PKGDATADIR};
use gettextrs::{LocaleCategory, bind_textdomain_codeset, bindtextdomain, setlocale, textdomain};
use gtk::{gio, glib, prelude::*};

fn main() -> glib::ExitCode {
    setlocale(LocaleCategory::LcAll, "");
    bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("Unable to bind the text domain");
    bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8")
        .expect("Unable to set the text domain encoding");
    textdomain(GETTEXT_PACKAGE).expect("Unable to switch to the text domain");

    let resources = gio::Resource::load(PKGDATADIR.to_owned() + "/whetstone.gresource")
        .expect("Could not load resources");
    gio::resources_register(&resources);

    gtk::init().expect("Failed to initialize GTK");

    sourceview::init();

    let app = TypesetterApplication::new(APP_ID, &gio::ApplicationFlags::HANDLES_OPEN);

    app.run()
}
