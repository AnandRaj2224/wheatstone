<div align="center">
  <img src="LOGO.png" alt="Whetstone Logo" width="128" height="128"/>

  # Whetstone

  **A fast, distraction-free Typst editor designed for GNOME.**

  [![License: GPL-3.0-or-later](https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg)](LICENSE)
  [![Built with GTK4](https://img.shields.io/badge/GUI-GTK4%20%7C%20Libadwaita-blue)](https://gtk.org)
  [![Rust](https://img.shields.io/badge/Language-Rust-orange.svg)](https://www.rust-lang.org)
  [![Typst](https://img.shields.io/badge/Powered%20by-Typst-239dad.svg)](https://typst.app)
</div>

---

## Overview

**Whetstone** is a native, minimalist editor for composing documents using the [Typst](https://typst.app) markup language. Built from the ground up in Rust with GTK4 and Libadwaita, Whetstone couples the clean ergonomics of modern GNOME with instant live compilation, deep multi-file project support, and memory-efficient rendering.

Whetstone is a modernized fork of [Typesetter](https://codeberg.org/haydn/typesetter), enhanced with a built-in project file tree, resizable split-pane layout, robust sibling-path resolution, and high-performance render lifecycle management.

---

## Key Features

### Enhanced in Whetstone
- **🗂️ Built-in File Explorer Sidebar:** Effortlessly navigate multi-file Typst projects, chapters, and assets. Includes automatic directory expansion, file filtering, symlink loop protection, and quick folder switching.
- **↔️ Resizable Panes with Persistent Split:** Smoothly adjust the boundary between the source code editor and preview pane with minimum bound guards. Your preferred split ratio is debounced and remembered across sessions.
- **⚡ High-Performance Preview Pipeline:**
  - **Generation Tracking:** Render cancellation prevents stale compilation passes from overwriting current documents during fast typing.
  - **Bounded LRU Cache:** Dual-limited texture cache (page count and memory ceiling) prevents memory leaks when browsing large multi-page documents.
  - **Zero-Copy Rendering:** Direct buffer transfers into GTK memory textures minimize memory allocations and CPU overhead.
  - **Visible-Page Texture Detachment:** Offscreen page textures are detached while remaining quickly retrievable from cache.
- **📁 Multi-File Relative Import Resolution:** Fully handles nested project subfolders and relative `#import` / `#include` paths against the active project root without breaking dependency monitoring.
- **🔄 Seamless Typesetter Migration:** Automatically and safely migrates user preferences, downloaded Typst packages, and application state from legacy Typesetter installs on first launch.

### Core Typst & Editor Capabilities
- **Live Preview & Click-to-Jump:** Bidirectional navigation between the source text and compiled pages.
- **Magnifier Tool:** Click and hold on the preview to inspect vector typography and diagrams up close.
- **Accessibility Checker:** Built-in simulation filters for various forms of color vision deficiency (protanopia, deuteranopia, tritanopia).
- **Prose Assistance:** Integrated offline grammar suggestions (via [Harper](https://github.com/Automattic/harper)) and spellchecking (via Libspelling).
- **Centered Typing & Theming:** Centered scrolling keeps the active line visually anchored; full support for GNOME light and dark color schemes.
- **Offline & Private:** Fully local compilation. No telemetry, no cloud dependency, and no account required.
- **Package & Template Manager:** Browse, install, and manage Typst packages and document templates locally.

---

## Installation & Building

### Prerequisites

To build Whetstone natively, you will need:
- **Rust Toolchain** (2024 edition compatible, `rustc` & `cargo` >= 1.85)
- **Meson** (>= 1.7.0) and **Ninja**
- **GTK4** (`gtk4` >= 4.18) & **Libadwaita** (`libadwaita-1` >= 1.6)
- **GtkSourceView 5** (`gtksourceview-5` >= 5.14)
- **libspelling** (>= 0.4)
- **desktop-file-utils**, **appstream**, **glib-2.0**

On Fedora / Silverblue:
```bash
sudo dnf install meson ninja-build rust cargo gtk4-devel libadwaita-devel gtksourceview5-devel libspelling-devel
```

On Arch Linux:
```bash
sudo pacman -S meson ninja rust gtk4 libadwaita gtksourceview5 libspelling
```

On Debian / Ubuntu (24.04+):
```bash
sudo apt install meson ninja-build cargo libgtk-4-dev libadwaita-1-dev libgtksourceview-5-dev libspelling-1-dev
```

### Native Build (Meson)

```bash
# Clone the repository
git clone https://github.com/AnandRaj2224/whetstone.git
cd whetstone

# Setup build directory
meson setup build --prefix=/usr/local

# Compile
meson compile -C build

# Install (optional)
sudo meson install -C build
```

Run directly from build directory:
```bash
./build/src/whetstone
```

### Building with Flatpak

Whetstone includes a Flatpak manifest (`dev.whetstone.editor.json`):

```bash
# Install GNOME SDK and Rust extension
flatpak install flathub org.gnome.Platform//50 org.gnome.Sdk//50 org.freedesktop.Sdk.Extension.rust-stable//50

# Build and install locally
flatpak-builder --user --install --force-clean build-flatpak dev.whetstone.editor.json

# Run
flatpak run dev.whetstone.editor
```

---

## Project Structure

```text
whetstone/
├── LOGO.png                    # Application logo
├── data/                       # Desktop files, icons, GSettings schema, UI definitions
│   ├── dev.whetstone.editor.gschema.xml
│   ├── gtk/                    # GTK4 composite template UI XML files
│   └── icons/                  # Action icons & desktop icon installation
├── po/                         # Translations / gettext catalogs
├── src/                        # Rust source code
│   ├── application.rs          # AdwApplication lifecycle & actions
│   ├── config.rs.in            # Build-time configuration template
│   ├── editor/                 # Text editor, syntax highlighting, spelling, grammar
│   ├── explorer/               # File tree sidebar & node navigation
│   ├── migration.rs            # One-time migration from Typesetter
│   ├── preview/                # Live preview, LRU texture cache, magnifier
│   ├── typst_system/           # Typst world, compiler arena, multi-file resolution
│   └── window.rs               # Main application window & resizable paned layout
├── dev.whetstone.editor.json   # Flatpak manifest
├── Cargo.toml                  # Rust dependencies & metadata
├── meson.build                 # Root Meson build specification
└── LICENSE                     # GNU General Public License v3.0
```

---

## Upstream & Acknowledgements

Whetstone is based on **[Typesetter](https://codeberg.org/haydn/typesetter)**, originally created by [Haydn Trowell](https://codeberg.org/haydn). We are deeply grateful to Haydn Trowell and the Typesetter contributors for laying the foundation of this application.

Special thanks to the [Typst team](https://github.com/typst/typst) for building such an incredible typesetting engine.

---

## License

Whetstone is free and open-source software distributed under the terms of the **[GNU General Public License v3.0 or later](LICENSE)** (GPL-3.0-or-later).
