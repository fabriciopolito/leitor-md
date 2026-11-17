//! Contêiner com rolagem apenas horizontal para tabelas largas.
//!
//! O `GtkScrolledWindow` mede a altura do filho na largura mínima, o que
//! deixaria tabelas com texto quebrado muito altas. Este widget mede a altura
//! na largura que a tabela realmente vai ocupar.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct HScroll {
        pub sw: RefCell<Option<gtk::ScrolledWindow>>,
        pub child: RefCell<Option<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HScroll {
        const NAME: &'static str = "LeitorHScroll";
        type Type = super::HScroll;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for HScroll {
        fn dispose(&self) {
            if let Some(sw) = self.sw.take() {
                sw.unparent();
            }
            self.child.take();
        }
    }

    impl WidgetImpl for HScroll {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orient: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.child.borrow().clone() else { return (0, 0, -1, -1) };
            let (wmin, wnat, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
            if orient == gtk::Orientation::Horizontal {
                (0, wnat, -1, -1)
            } else {
                let w = if for_size < 0 { wnat } else { for_size.max(wmin).min(wnat) };
                let (_, nat, _, _) = child.measure(gtk::Orientation::Vertical, w);
                // +2 px: margem para a borda inferior não ser cortada por arredondamento.
                (nat + 2, nat + 2, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(sw) = self.sw.borrow().as_ref() {
                sw.size_allocate(&gtk::Allocation::new(0, 0, width, height), baseline);
            }
        }
    }
}

glib::wrapper! {
    pub struct HScroll(ObjectSubclass<imp::HScroll>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl HScroll {
    pub fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let o: Self = glib::Object::new();
        let sw = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(child)
            .build();
        sw.set_parent(&o);
        o.imp().sw.replace(Some(sw));
        o.imp().child.replace(Some(child.clone().upcast()));
        o
    }
}
