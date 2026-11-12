// Copyright 2026 Haydn Trowell
// SPDX-License-Identifier: GPL-3.0-or-later

pub mod node;

pub use node::ExplorerNode;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use gettextrs::gettext;
use gtk::{CompositeTemplate, gio, glib, pango};
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(CompositeTemplate)]
    #[template(resource = "/dev/whetstone/editor/gtk/explorer.ui")]
    pub struct TypesetterExplorer {
        #[template_child]
        pub header_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub title_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub open_folder_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub refresh_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub view_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub tree_scroll: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub file_list: TemplateChild<gtk::ListView>,
        #[template_child]
        pub empty_state: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub empty_open_button: TemplateChild<gtk::Button>,

        pub root_path: RefCell<Option<PathBuf>>,
        pub root_store: gio::ListStore,
    }

    impl Default for TypesetterExplorer {
        fn default() -> Self {
            Self {
                header_box: TemplateChild::default(),
                title_label: TemplateChild::default(),
                open_folder_button: TemplateChild::default(),
                refresh_button: TemplateChild::default(),
                view_stack: TemplateChild::default(),
                tree_scroll: TemplateChild::default(),
                file_list: TemplateChild::default(),
                empty_state: TemplateChild::default(),
                empty_open_button: TemplateChild::default(),
                root_path: RefCell::new(None),
                root_store: gio::ListStore::new::<ExplorerNode>(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TypesetterExplorer {
        const NAME: &'static str = "TypesetterExplorer";
        type Type = super::TypesetterExplorer;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TypesetterExplorer {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("file-activated")
                        .param_types([gio::File::static_type()])
                        .build(),
                    glib::subclass::Signal::builder("open-folder-requested").build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_widgets();
        }
    }

    impl WidgetImpl for TypesetterExplorer {}
    impl BoxImpl for TypesetterExplorer {}
}

glib::wrapper! {
    pub struct TypesetterExplorer(ObjectSubclass<imp::TypesetterExplorer>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Buildable, gtk::ConstraintTarget, gtk::Accessible, gtk::Orientable;
}

impl Default for TypesetterExplorer {
    fn default() -> Self {
        Self::new()
    }
}

impl TypesetterExplorer {
    pub fn new() -> Self {
        glib::Object::new()
    }

    fn setup_widgets(&self) {
        let imp = self.imp();

        let tree_model = gtk::TreeListModel::new(imp.root_store.clone(), false, false, |item| {
            item.downcast_ref::<ExplorerNode>()
                .and_then(|node| node.read_children())
        });
        let selection = gtk::SingleSelection::new(Some(tree_model));
        imp.file_list.set_model(Some(&selection));

        let factory = gtk::SignalListItemFactory::new();

        factory.connect_setup(|_, list_item| {
            let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
                return;
            };

            let expander = gtk::TreeExpander::new();
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row_box.set_margin_top(2);
            row_box.set_margin_bottom(2);
            row_box.set_margin_end(4);

            let icon = gtk::Image::new();
            let label = gtk::Label::new(None);
            label.set_ellipsize(pango::EllipsizeMode::End);
            label.set_xalign(0.0);
            label.set_hexpand(true);

            row_box.append(&icon);
            row_box.append(&label);
            expander.set_child(Some(&row_box));
            list_item.set_child(Some(&expander));
        });

        factory.connect_bind(|_, list_item| {
            let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(expander) = list_item.child().and_downcast::<gtk::TreeExpander>() else {
                return;
            };
            let Some(tree_row) = list_item.item().and_downcast::<gtk::TreeListRow>() else {
                return;
            };

            expander.set_list_row(Some(&tree_row));

            let Some(row_box) = expander.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let Some(icon) = row_box.first_child().and_downcast::<gtk::Image>() else {
                return;
            };
            let Some(label) = icon.next_sibling().and_downcast::<gtk::Label>() else {
                return;
            };

            if let Some(node) = tree_row.item().and_downcast::<ExplorerNode>() {
                icon.set_icon_name(Some(&node.icon_name()));
                label.set_text(&node.name());
                row_box.set_tooltip_text(Some(&node.path().to_string_lossy()));
            }
        });

        factory.connect_unbind(|_, list_item| {
            let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            if let Some(expander) = list_item.child().and_downcast::<gtk::TreeExpander>() {
                expander.set_list_row(None);
            }
        });

        imp.file_list.set_factory(Some(&factory));

        imp.file_list.connect_activate(glib::clone!(
            #[weak(rename_to = explorer)]
            self,
            move |list_view, position| {
                let Some(model) = list_view.model() else {
                    return;
                };
                let Some(tree_row) = model.item(position).and_downcast::<gtk::TreeListRow>() else {
                    return;
                };
                let Some(node) = tree_row.item().and_downcast::<ExplorerNode>() else {
                    return;
                };

                if node.is_directory() {
                    tree_row.set_expanded(!tree_row.is_expanded());
                } else {
                    let file = gio::File::for_path(node.path());
                    explorer.emit_by_name::<()>("file-activated", &[&file]);
                }
            }
        ));

        imp.open_folder_button.connect_clicked(glib::clone!(
            #[weak(rename_to = explorer)]
            self,
            move |_| {
                explorer.emit_by_name::<()>("open-folder-requested", &[]);
            }
        ));

        imp.empty_open_button.connect_clicked(glib::clone!(
            #[weak(rename_to = explorer)]
            self,
            move |_| {
                explorer.emit_by_name::<()>("open-folder-requested", &[]);
            }
        ));

        imp.refresh_button.connect_clicked(glib::clone!(
            #[weak(rename_to = explorer)]
            self,
            move |_| {
                explorer.refresh();
            }
        ));
    }

    pub fn set_root_path(&self, path: Option<&Path>) {
        let imp = self.imp();
        match path {
            Some(p) => {
                if imp.root_path.borrow().as_deref() == Some(p) {
                    return;
                }
                self.load_directory(p);
            }
            None => {
                *imp.root_path.borrow_mut() = None;
                imp.root_store.remove_all();
                imp.title_label.set_text(&gettext("Project Files"));
                imp.title_label.set_tooltip_text(None);
                imp.view_stack.set_visible_child_name("empty");
            }
        }
    }

    pub fn refresh(&self) {
        let current_root = self.imp().root_path.borrow().clone();
        if let Some(path) = current_root {
            self.load_directory(&path);
        }
    }

    pub fn root_path(&self) -> Option<PathBuf> {
        self.imp().root_path.borrow().clone()
    }

    fn load_directory(&self, path: &Path) {
        let imp = self.imp();
        if !path.exists() || !path.is_dir() {
            imp.view_stack.set_visible_child_name("empty");
            imp.title_label.set_text(&gettext("Project Files"));
            imp.title_label.set_tooltip_text(None);
            imp.root_store.remove_all();
            *imp.root_path.borrow_mut() = None;
            return;
        }

        *imp.root_path.borrow_mut() = Some(path.to_path_buf());
        let folder_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        imp.title_label.set_text(&folder_name);
        imp.title_label.set_tooltip_text(Some(&path.to_string_lossy()));

        let root_node = ExplorerNode::new(path.to_path_buf(), Vec::new());
        imp.root_store.remove_all();
        if let Some(children) = root_node.read_children() {
            for i in 0..children.n_items() {
                if let Some(child) = children.item(i).and_downcast::<ExplorerNode>() {
                    imp.root_store.append(&child);
                }
            }
        }

        imp.view_stack.set_visible_child_name("tree");
    }

    pub fn connect_file_activated<F: Fn(&Self, &gio::File) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_closure(
            "file-activated",
            false,
            glib::closure_local!(move |explorer: Self, file: gio::File| {
                f(&explorer, &file);
            }),
        )
    }

    pub fn connect_open_folder_requested<F: Fn(&Self) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_closure(
            "open-folder-requested",
            false,
            glib::closure_local!(move |explorer: Self| {
                f(&explorer);
            }),
        )
    }
}
