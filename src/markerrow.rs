//! Linha "marcador + conteúdo" (itens de lista e notas de rodapé).
//!
//! Substitui um GtkBox horizontal: o GtkBox, ao se alocar, mede a largura dos
//! filhos para uma altura dada, e rótulos com quebra de linha respondem a isso
//! de forma inconsistente (aviso "Expect overlapping widgets" do GTK). Aqui o
//! marcador usa sua largura natural e o conteúdo fica com o restante.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct MarkerRow {
        pub marker: RefCell<Option<gtk::Widget>>,
        pub content: RefCell<Option<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MarkerRow {
        const NAME: &'static str = "LeitorMarkerRow";
        type Type = super::MarkerRow;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for MarkerRow {
        fn dispose(&self) {
            for w in [self.marker.take(), self.content.take()].into_iter().flatten() {
                w.unparent();
            }
        }
    }

    impl MarkerRow {
        fn parts(&self) -> Option<(gtk::Widget, gtk::Widget)> {
            Some((self.marker.borrow().clone()?, self.content.borrow().clone()?))
        }
        /// (largura do marcador, largura do conteúdo) para a largura total dada.
        fn widths(&self, m: &gtk::Widget, c: &gtk::Widget, total: i32) -> (i32, i32) {
            // O conteúdo (texto que quebra) não precisa de largura mínima;
            // evitar medi-lo na horizontal poupa uma passada inteira pelo Pango.
            let _ = c;
            let mw = m.measure(gtk::Orientation::Horizontal, -1).1;
            (mw, (total - mw).max(1))
        }
    }

    impl WidgetImpl for MarkerRow {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orient: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some((m, c)) = self.parts() else { return (0, 0, -1, -1) };
            if orient == gtk::Orientation::Horizontal {
                let (_, mnat, _, _) = m.measure(gtk::Orientation::Horizontal, -1);
                (mnat, mnat, -1, -1)
            } else {
                if for_size < 0 {
                    return (0, 0, -1, -1);
                }
                let (mw, cw) = self.widths(&m, &c, for_size);
                let mh = m.measure(gtk::Orientation::Vertical, mw).1;
                let (cmin, cnat, _, _) = c.measure(gtk::Orientation::Vertical, cw);
                (cmin.max(mh), cnat.max(mh), -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let Some((m, c)) = self.parts() else { return };
            let (mw, cw) = self.widths(&m, &c, width);
            let mh = m.measure(gtk::Orientation::Vertical, mw).1.min(height);
            m.size_allocate(&gtk::Allocation::new(0, 0, mw, mh), -1);
            c.size_allocate(&gtk::Allocation::new(mw, 0, cw, height), -1);
        }
    }
}

glib::wrapper! {
    pub struct MarkerRow(ObjectSubclass<imp::MarkerRow>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MarkerRow {
    pub fn new(marker: &impl IsA<gtk::Widget>, content: &impl IsA<gtk::Widget>) -> Self {
        let o: Self = glib::Object::new();
        marker.set_parent(&o);
        content.set_parent(&o);
        o.imp().marker.replace(Some(marker.clone().upcast()));
        o.imp().content.replace(Some(content.clone().upcast()));
        o
    }
}
