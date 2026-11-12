// Copyright 2026 Haydn Trowell
// SPDX-License-Identifier: GPL-3.0-or-later

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use gtk::{gio, glib};
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ExplorerNode {
        pub path: RefCell<PathBuf>,
        pub name: RefCell<String>,
        pub is_directory: RefCell<bool>,
        pub icon_name: RefCell<String>,
        pub ancestors: RefCell<Vec<PathBuf>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ExplorerNode {
        const NAME: &'static str = "ExplorerNode";
        type Type = super::ExplorerNode;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for ExplorerNode {}
}

glib::wrapper! {
    pub struct ExplorerNode(ObjectSubclass<imp::ExplorerNode>);
}

impl ExplorerNode {
    pub fn new(path: PathBuf, ancestors: Vec<PathBuf>) -> Self {
        let node: Self = glib::Object::new();
        let is_dir = path.is_dir();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        let icon_name = Self::determine_icon(&path, is_dir);

        *node.imp().path.borrow_mut() = path;
        *node.imp().name.borrow_mut() = name;
        *node.imp().is_directory.borrow_mut() = is_dir;
        *node.imp().icon_name.borrow_mut() = icon_name;
        *node.imp().ancestors.borrow_mut() = ancestors;

        node
    }

    pub fn determine_icon(path: &Path, is_dir: bool) -> String {
        if is_dir {
            "folder-symbolic".to_string()
        } else {
            match path.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase()).as_deref() {
                Some("typ") => "code-symbolic".to_string(),
                Some("png" | "jpg" | "jpeg" | "svg" | "webp" | "gif" | "bmp") => {
                    "image-x-generic-symbolic".to_string()
                }
                Some("pdf") => "x-office-document-symbolic".to_string(),
                _ => "text-x-generic-symbolic".to_string(),
            }
        }
    }

    pub fn path(&self) -> PathBuf {
        self.imp().path.borrow().clone()
    }

    pub fn name(&self) -> String {
        self.imp().name.borrow().clone()
    }

    pub fn is_directory(&self) -> bool {
        *self.imp().is_directory.borrow()
    }

    pub fn icon_name(&self) -> String {
        self.imp().icon_name.borrow().clone()
    }

    pub fn read_children(&self) -> Option<gio::ListModel> {
        if !self.is_directory() {
            return None;
        }

        let path = self.path();
        let read_dir = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => return Some(gio::ListStore::new::<ExplorerNode>().upcast()),
        };

        let mut child_entries = Vec::new();

        for entry in read_dir.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            if Self::is_ignored(&file_name) {
                continue;
            }

            let entry_path = entry.path();
            if let Ok(canonical) = std::fs::canonicalize(&entry_path)
                && self.imp().ancestors.borrow().contains(&canonical)
            {
                // Loop detected via symlink, ignore
                continue;
            }

            let is_dir = entry_path.is_dir();
            child_entries.push((is_dir, file_name, entry_path));
        }

        Self::sort_entries(&mut child_entries);

        let store = gio::ListStore::new::<ExplorerNode>();
        let mut new_ancestors = self.imp().ancestors.borrow().clone();
        if let Ok(canonical) = std::fs::canonicalize(&path) {
            new_ancestors.push(canonical);
        }

        for (_, _, child_path) in child_entries {
            let child_node = ExplorerNode::new(child_path, new_ancestors.clone());
            store.append(&child_node);
        }

        Some(store.upcast())
    }

    pub fn is_ignored(name: &str) -> bool {
        name.starts_with('.') || name == "node_modules" || name == "target" || name == "__pycache__"
    }

    pub fn sort_entries(entries: &mut [(bool, String, PathBuf)]) {
        entries.sort_by_cached_key(|entry| (!entry.0, entry.1.to_lowercase()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ignored() {
        assert!(ExplorerNode::is_ignored(".git"));
        assert!(ExplorerNode::is_ignored(".hidden"));
        assert!(ExplorerNode::is_ignored(".gitignore"));
        assert!(ExplorerNode::is_ignored("target"));
        assert!(ExplorerNode::is_ignored("node_modules"));
        assert!(ExplorerNode::is_ignored("__pycache__"));

        assert!(!ExplorerNode::is_ignored("main.typ"));
        assert!(!ExplorerNode::is_ignored("images"));
        assert!(!ExplorerNode::is_ignored("chapter1.typ"));
        assert!(!ExplorerNode::is_ignored("README.md"));
    }

    #[test]
    fn test_sort_entries_directories_first_and_alphabetical() {
        let mut entries = vec![
            (false, "z_doc.typ".to_string(), PathBuf::from("z_doc.typ")),
            (true, "zebra_folder".to_string(), PathBuf::from("zebra_folder")),
            (false, "a_doc.typ".to_string(), PathBuf::from("a_doc.typ")),
            (true, "alpha_folder".to_string(), PathBuf::from("alpha_folder")),
            (false, "B_doc.typ".to_string(), PathBuf::from("B_doc.typ")),
        ];

        ExplorerNode::sort_entries(&mut entries);

        let names: Vec<&str> = entries.iter().map(|(_, name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec!["alpha_folder", "zebra_folder", "a_doc.typ", "B_doc.typ", "z_doc.typ"]
        );
    }

    #[test]
    fn test_determine_icon() {
        assert_eq!(
            ExplorerNode::determine_icon(Path::new("somedir"), true),
            "folder-symbolic"
        );
        assert_eq!(
            ExplorerNode::determine_icon(Path::new("main.typ"), false),
            "code-symbolic"
        );
        assert_eq!(
            ExplorerNode::determine_icon(Path::new("image.PNG"), false),
            "image-x-generic-symbolic"
        );
        assert_eq!(
            ExplorerNode::determine_icon(Path::new("output.pdf"), false),
            "x-office-document-symbolic"
        );
        assert_eq!(
            ExplorerNode::determine_icon(Path::new("data.csv"), false),
            "text-x-generic-symbolic"
        );
    }

    #[test]
    fn test_read_children_and_symlink_loop_detection() {
        let temp_dir = std::env::temp_dir().join(format!(
            "whetstone_test_explorer_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let subfolder = temp_dir.join("subfolder");
        std::fs::create_dir_all(&subfolder).unwrap();

        let hidden_dir = temp_dir.join(".git");
        std::fs::create_dir_all(&hidden_dir).unwrap();

        let target_dir = temp_dir.join("target");
        std::fs::create_dir_all(&target_dir).unwrap();

        let file_typ = temp_dir.join("main.typ");
        std::fs::write(&file_typ, "content").unwrap();

        let file_png = temp_dir.join("image.png");
        std::fs::write(&file_png, "png content").unwrap();

        let sub_file = subfolder.join("nested.typ");
        std::fs::write(&sub_file, "nested content").unwrap();

        // Create symlink loop: subfolder/loop -> temp_dir
        #[cfg(unix)]
        {
            let _ = std::os::unix::fs::symlink(&temp_dir, subfolder.join("loop_to_root"));
        }

        let root_node = ExplorerNode::new(temp_dir.clone(), Vec::new());
        let children = root_node.read_children().expect("children model exists");

        // .git and target should be ignored
        // subfolder should be first
        assert_eq!(children.n_items(), 3);

        let item0 = children.item(0).and_downcast::<ExplorerNode>().unwrap();
        assert_eq!(item0.name(), "subfolder");
        assert!(item0.is_directory());

        let item1 = children.item(1).and_downcast::<ExplorerNode>().unwrap();
        assert_eq!(item1.name(), "image.png");
        assert!(!item1.is_directory());

        let item2 = children.item(2).and_downcast::<ExplorerNode>().unwrap();
        assert_eq!(item2.name(), "main.typ");
        assert!(!item2.is_directory());

        // Read children of subfolder (should not loop indefinitely despite symlink to temp_dir)
        let sub_children = item0.read_children().expect("sub children exist");
        // Only nested.typ should be present; loop_to_root was skipped due to ancestor loop detection
        assert_eq!(sub_children.n_items(), 1);
        let sub_item0 = sub_children.item(0).and_downcast::<ExplorerNode>().unwrap();
        assert_eq!(sub_item0.name(), "nested.typ");

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
