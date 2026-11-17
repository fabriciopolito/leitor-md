//! Linha de imagens: exibe imagens em tamanho natural, lado a lado
//! (quebrando linha quando necessário) e reduzindo proporcionalmente
//! qualquer imagem mais larga que a coluna de leitura.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

const SPACING: i32 = 6;

mod imp {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    pub struct ImageRow {
        pub center: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ImageRow {
        const NAME: &'static str = "LeitorImageRow";
        type Type = super::ImageRow;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ImageRow {
        fn dispose(&self) {
            while let Some(c) = self.obj().first_child() {
                c.unparent();
            }
        }
    }

    impl ImageRow {
        /// Calcula a posição (x, y, w, h) de cada filho para a largura dada.
        pub fn layout(&self, width: i32) -> (Vec<(gtk::Widget, i32, i32, i32, i32)>, i32) {
            let mut out = Vec::new();
            let mut line: Vec<(gtk::Widget, i32, i32)> = Vec::new();
            let (mut x, mut y, mut line_h) = (0, 0, 0);
            let center = self.center.get();
            let flush = |line: &mut Vec<(gtk::Widget, i32, i32)>, out: &mut Vec<_>, x: i32, y: i32| {
                let off = if center { ((width - (x - SPACING)) / 2).max(0) } else { 0 };
                let mut cx = off;
                for (w, cw, ch) in line.drain(..) {
                    out.push((w, cx, y, cw, ch));
                    cx += cw + SPACING;
                }
            };
            let mut child = self.obj().first_child();
            while let Some(c) = child {
                child = c.next_sibling();
                if !c.is_visible() {
                    continue;
                }
                let (_, nw, _, _) = c.measure(gtk::Orientation::Horizontal, -1);
                let (mut w, mut h) = (nw.max(1), 0);
                if w > width && width > 0 {
                    w = width;
                }
                let (_, nh, _, _) = c.measure(gtk::Orientation::Vertical, w);
                h = h.max(nh);
                if x > 0 && x + w > width {
                    flush(&mut line, &mut out, x, y);
                    y += line_h + SPACING;
                    x = 0;
                    line_h = 0;
                }
                line.push((c, w, h));
                x += w + SPACING;
                line_h = line_h.max(h);
            }
            flush(&mut line, &mut out, x, y);
            (out, y + line_h)
        }
    }

    impl WidgetImpl for ImageRow {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orient: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if orient == gtk::Orientation::Horizontal {
                let mut nat = 0;
                let mut c = self.obj().first_child();
                while let Some(w) = c {
                    nat += w.measure(gtk::Orientation::Horizontal, -1).1 + SPACING;
                    c = w.next_sibling();
                }
                (0, (nat - SPACING).max(0), -1, -1)
            } else {
                let width = if for_size < 0 { i32::MAX / 2 } else { for_size };
                let (_, h) = self.layout(width);
                (h, h, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            for (w, x, y, cw, ch) in self.layout(width).0 {
                w.size_allocate(&gtk::Allocation::new(x, y, cw, ch), -1);
            }
        }
    }
}

glib::wrapper! {
    pub struct ImageRow(ObjectSubclass<imp::ImageRow>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ImageRow {
    pub fn new(center: bool) -> Self {
        let o: Self = glib::Object::new();
        o.imp().center.set(center);
        o
    }

    pub fn append(&self, w: &impl IsA<gtk::Widget>) {
        w.set_parent(self);
    }
}
