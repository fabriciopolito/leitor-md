//! Janela principal: barra superior, sumário lateral e área de leitura.

use crate::render::{self, Anchor, Doc, LinkHandler};
use crate::search::{Match, Search};
use crate::{highlight, theme, APP_ID, APP_NAME};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, graphene};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Instant;

/// Tamanho base do texto (100 %) e largura máxima da coluna de leitura.
const BASE_PT: f64 = 12.0;
const BASE_MAX_WIDTH: f64 = 780.0;
const SIDE_PAD: i32 = 36;
const ZOOM_STEPS: [f64; 14] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0];
const SCROLL_ANIM_US: f64 = 260_000.0;
/// Distância (px) do topo da janela usada para decidir o tópico "atual".
const ACTIVE_OFFSET: f64 = 48.0;

thread_local! {
    static WINDOW_COUNTER: Cell<u32> = const { Cell::new(0) };
}

#[derive(Default)]
struct State {
    path: Option<PathBuf>,
    /// Observa o arquivo aberto para recarregá-lo quando for alterado.
    monitor: Option<gio::FileMonitor>,
    reload_timer: Option<glib::SourceId>,
    /// Percentual lido exibido no rodapé do sumário (evita atualizações à toa).
    shown_pct: Option<i32>,
    doc: Option<Doc>,
    active: Option<usize>,
    anim: Option<gtk::TickCallbackId>,
    animating: bool,
    lock_active: bool,
    pending_frac: Option<f64>,
    /// Ação à espera de um trecho do documento ainda em montagem (ex.: pular
    /// para o fim). Chamada a cada lote; devolve `true` quando foi cumprida.
    waiter: Option<Box<dyn FnMut(&Rc<Viewer>) -> bool>>,
    /// Montagem acelerada (há uma ação esperando).
    turbo: bool,
    /// Busca ativa (destaques aplicados).
    search: Option<Search>,
    /// Muda a cada documento carregado (interrompe a inserção gradual anterior).
    generation: u64,
}

/// Texto montado por quadro durante a carga em segundo plano: valor inicial
/// e limites do ajuste automático (ver start_appending).
const CHARS_PER_FRAME: usize = 4_000;
const MIN_CHARS_PER_FRAME: usize = 800;
const MAX_CHARS_PER_FRAME: usize = 60_000;
/// Duração de quadro desejada durante a carga.
const FRAME_TARGET: std::time::Duration = std::time::Duration::from_millis(33);

pub struct Viewer {
    pub window: gtk::ApplicationWindow,
    file_label: gtk::Label,
    zoom_btn: gtk::Button,
    theme_btn: gtk::Button,
    sidebar_btn: gtk::ToggleButton,
    sidebar: gtk::Revealer,
    toc: gtk::ListBox,
    toc_scroll: gtk::ScrolledWindow,
    /// Seleção pendente do sumário (aplicada uma vez por ciclo; ver set_active).
    toc_sel: Rc<Cell<Option<Option<usize>>>>,
    scroller: gtk::ScrolledWindow,
    holder: column::ReadingColumn,
    zoom_css: gtk::CssProvider,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    search_count: gtk::Label,
    progress: gtk::ProgressBar,
    to_top: gtk::Revealer,
    stats: gtk::Label,
    root_name: String,
    zoom: Cell<f64>,
    st: RefCell<State>,
}

impl Viewer {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let n = WINDOW_COUNTER.with(|c| {
            c.set(c.get() + 1);
            c.get()
        });

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title(APP_NAME)
            .default_width(1120)
            .default_height(800)
            .icon_name(APP_ID)
            .build();

        // ----- Barra de ferramentas -----
        // A barra de título fica a cargo do gerenciador de janelas (no Mint,
        // a barra nativa do Cinnamon); logo abaixo vem uma barra de ferramentas
        // comum, com ícones do tema do sistema.
        let sidebar_btn = gtk::ToggleButton::builder()
            .active(true)
            .tooltip_text("Mostrar/ocultar sumário (F9)")
            .build();
        set_icon(sidebar_btn.upcast_ref(), &["sidebar-show-symbolic", "view-dual-symbolic"], "☰");

        let open_btn = gtk::Button::builder()
            .action_name("win.open")
            .tooltip_text("Abrir arquivo Markdown (Ctrl+O)")
            .build();
        let open_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        if let Some(img) = themed_image(&["document-open-symbolic"]) {
            open_content.append(&img);
        }
        open_content.append(&gtk::Label::new(Some("Abrir")));
        open_btn.set_child(Some(&open_content));

        let app_label = gtk::Label::new(Some(APP_NAME));
        app_label.add_css_class("app-name");
        let file_label = gtk::Label::new(Some("Nenhum arquivo"));
        file_label.add_css_class("file-name");
        file_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        file_label.set_hexpand(true);
        file_label.set_xalign(0.0);
        let title_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        title_box.add_css_class("title-box");
        title_box.append(&app_label);
        title_box.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        title_box.append(&file_label);
        title_box.set_hexpand(true);

        let theme_btn = gtk::Button::builder()
            .action_name("win.toggle-theme")
            .tooltip_text("Alternar tema claro/escuro")
            .build();

        let zoom_out = gtk::Button::builder()
            .action_name("win.zoom-out")
            .tooltip_text("Diminuir zoom (Ctrl −)")
            .build();
        set_icon(zoom_out.upcast_ref(), &["zoom-out-symbolic"], "−");
        let zoom_btn = gtk::Button::builder()
            .label("100%")
            .action_name("win.zoom-reset")
            .tooltip_text("Restaurar zoom padrão (Ctrl 0)")
            .build();
        zoom_btn.add_css_class("zoom-label");
        let zoom_in = gtk::Button::builder()
            .action_name("win.zoom-in")
            .tooltip_text("Aumentar zoom (Ctrl +)")
            .build();
        set_icon(zoom_in.upcast_ref(), &["zoom-in-symbolic"], "+");
        let zoom_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        zoom_box.add_css_class("linked");
        zoom_box.append(&zoom_out);
        zoom_box.append(&zoom_btn);
        zoom_box.append(&zoom_in);

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.add_css_class("toolbar");
        toolbar.add_css_class("main-toolbar");
        toolbar.append(&sidebar_btn);
        toolbar.append(&open_btn);
        toolbar.append(&title_box);
        let search_btn = gtk::ToggleButton::builder()
            .tooltip_text("Buscar no documento (Ctrl+F)")
            .build();
        set_icon(search_btn.upcast_ref(), &["system-search-symbolic", "edit-find-symbolic"], "⌕");
        toolbar.append(&search_btn);
        // Largura do texto: toda a janela (padrão) ou coluna confortável.
        let width_btn = gtk::ToggleButton::builder()
            .action_name("win.toggle-width")
            .tooltip_text("Texto na largura total da janela (Ctrl+L)\nDesative para a coluna de leitura confortável")
            .build();
        set_icon(width_btn.upcast_ref(), &["format-justify-fill-symbolic", "view-paged-symbolic"], "⇔");
        toolbar.append(&width_btn);
        toolbar.append(&zoom_box);
        toolbar.append(&theme_btn);

        // ----- Barra de busca -----
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Buscar no documento")
            .width_chars(28)
            .build();
        let find_prev = gtk::Button::builder()
            .action_name("win.find-prev")
            .tooltip_text("Ocorrência anterior (Shift+F3)")
            .build();
        set_icon(&find_prev, &["go-up-symbolic"], "↑");
        let find_next = gtk::Button::builder()
            .action_name("win.find-next")
            .tooltip_text("Próxima ocorrência (F3 ou Enter)")
            .build();
        set_icon(&find_next, &["go-down-symbolic"], "↓");
        let nav = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        nav.add_css_class("linked");
        nav.append(&find_prev);
        nav.append(&find_next);
        let search_count = gtk::Label::new(None);
        search_count.add_css_class("search-count");
        let sb_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        sb_box.append(&search_entry);
        sb_box.append(&nav);
        sb_box.append(&search_count);
        let search_bar = gtk::SearchBar::builder().child(&sb_box).show_close_button(true).build();
        search_bar.connect_entry(&search_entry);
        search_bar
            .bind_property("search-mode-enabled", &search_btn, "active")
            .bidirectional()
            .sync_create()
            .build();

        // ----- Sumário -----
        let toc = gtk::ListBox::new();
        toc.add_css_class("navigation-sidebar");
        toc.add_css_class("toc");
        toc.set_selection_mode(gtk::SelectionMode::Single);
        let placeholder = gtk::Label::new(Some("Sem tópicos"));
        placeholder.add_css_class("toc-empty");
        toc.set_placeholder(Some(&placeholder));
        let toc_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&toc)
            .build();
        let toc_title = gtk::Label::new(Some("SUMÁRIO"));
        toc_title.set_xalign(0.0);
        toc_title.add_css_class("toc-title");
        toc.set_cursor_from_name(Some("pointer"));
        // Rodapé do sumário: tamanho do documento, tempo de leitura e progresso.
        let stats = gtk::Label::new(None);
        stats.add_css_class("doc-stats");
        stats.set_xalign(0.0);
        stats.set_wrap(true);
        let side_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side_box.add_css_class("sidebar-pane");
        side_box.set_size_request(260, -1);
        side_box.append(&toc_title);
        side_box.append(&toc_scroll);
        side_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        side_box.append(&stats);
        let side_wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        side_wrap.append(&side_box);
        side_wrap.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        let sidebar = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideRight)
            .transition_duration(160)
            .reveal_child(true)
            .child(&side_wrap)
            .build();

        // ----- Área de leitura (coluna centralizada com largura máxima) -----
        let holder = column::ReadingColumn::new(BASE_MAX_WIDTH as i32);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .vexpand(true)
            .child(&holder)
            .build();
        scroller.add_css_class("md-view");

        // Botão flutuante "voltar ao topo" (aparece depois de rolar uma tela).
        let top_btn = gtk::Button::builder()
            .tooltip_text("Voltar ao início (Home)")
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .build();
        set_icon(&top_btn, &["go-top-symbolic", "go-up-symbolic"], "↑");
        top_btn.add_css_class("circular");
        top_btn.add_css_class("osd");
        top_btn.add_css_class("to-top");
        top_btn.set_cursor_from_name(Some("pointer"));
        let to_top = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .transition_duration(180)
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .margin_end(28)
            .margin_bottom(24)
            .child(&top_btn)
            .build();
        // Barra fina de progresso de leitura, sobre a borda superior do texto.
        let progress = gtk::ProgressBar::new();
        progress.add_css_class("reading-progress");
        progress.set_valign(gtk::Align::Start);
        progress.set_can_target(false);
        progress.set_visible(false);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&scroller));
        overlay.add_overlay(&progress);
        overlay.add_overlay(&to_top);

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.append(&sidebar);
        body.append(&overlay);
        // Borda inferior da barra de busca (a mesma linha usada na lateral),
        // visível só com a busca aberta.
        let search_sep = gtk::Separator::new(gtk::Orientation::Horizontal);
        search_bar.bind_property("search-mode-enabled", &search_sep, "visible").sync_create().build();
        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main.append(&toolbar);
        main.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        main.append(&search_bar);
        main.append(&search_sep);
        main.append(&body);
        window.set_child(Some(&main));

        let zoom_css = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            &gtk::prelude::WidgetExt::display(&window),
            &zoom_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );

        let v = Rc::new(Viewer {
            window,
            file_label,
            zoom_btn,
            theme_btn,
            sidebar_btn,
            sidebar,
            toc,
            toc_scroll,
            toc_sel: Rc::new(Cell::new(None)),
            scroller,
            holder,
            zoom_css,
            search_bar,
            search_entry,
            search_count,
            progress,
            to_top,
            stats,
            root_name: format!("md-root-{n}"),
            zoom: Cell::new(1.0),
            st: RefCell::new(State::default()),
        });
        v.setup();
        v.show_empty();
        v.apply_zoom(1.0);
        v.sync_theme_button();
        v
    }

    fn setup(self: &Rc<Self>) {
        // A janela mantém o Viewer vivo até ser destruída.
        let keep = self.clone();
        self.window.connect_destroy(move |w| {
            gtk::style_context_remove_provider_for_display(&WidgetExt::display(w), &keep.zoom_css);
            if let Some(id) = keep.st.borrow_mut().anim.take() {
                id.remove();
            }
            keep.st.borrow_mut().doc = None;
        });

        self.action("open", |v| v.choose_file());
        self.action("reload", |v| v.reload());
        self.action("zoom-in", |v| v.zoom_step(1));
        self.action("zoom-out", |v| v.zoom_step(-1));
        self.action("zoom-reset", |v| v.apply_zoom(1.0));

        // Largura do texto (ação com estado: botão e Ctrl+L ficam sincronizados).
        let width = gio::SimpleAction::new_stateful("toggle-width", None, &true.to_variant());
        let w = Rc::downgrade(self);
        width.connect_activate(move |a, _| {
            let full = !a.state().and_then(|s| s.get::<bool>()).unwrap_or(true);
            a.set_state(&full.to_variant());
            if let Some(v) = w.upgrade() {
                v.set_full_width(full);
            }
        });
        self.window.add_action(&width);
        self.action("toggle-theme", |_| theme::toggle());
        self.action("toggle-sidebar", |v| v.sidebar_btn.set_active(!v.sidebar_btn.is_active()));
        self.action("close", |v| v.window.close());
        self.action("go-top", |v| v.animate_to(0.0));
        self.action("shortcuts", |v| v.show_shortcuts());
        if let Some(btn) = self.to_top.child().and_downcast::<gtk::Button>() {
            btn.set_action_name(Some("win.go-top"));
        }
        self.action("find", |v| v.open_search());
        self.action("find-next", |v| v.find_step(1));
        self.action("find-prev", |v| v.find_step(-1));

        // Busca
        let w = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |_| {
            if let Some(v) = w.upgrade() {
                v.run_search();
            }
        });
        let w = Rc::downgrade(self);
        self.search_entry.connect_activate(move |_| {
            if let Some(v) = w.upgrade() {
                v.find_step(1);
            }
        });
        let w = Rc::downgrade(self);
        self.search_entry.connect_next_match(move |_| {
            if let Some(v) = w.upgrade() {
                v.find_step(1);
            }
        });
        let w = Rc::downgrade(self);
        self.search_entry.connect_previous_match(move |_| {
            if let Some(v) = w.upgrade() {
                v.find_step(-1);
            }
        });
        // Shift+Enter na caixa de busca: ocorrência anterior.
        let shift_enter = gtk::EventControllerKey::new();
        shift_enter.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = Rc::downgrade(self);
        shift_enter.connect_key_pressed(move |_, key, _, state| {
            let enter = matches!(key, gdk::Key::Return | gdk::Key::KP_Enter);
            if enter && state.contains(gdk::ModifierType::SHIFT_MASK) {
                if let Some(v) = w.upgrade() {
                    v.find_step(-1);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.search_entry.add_controller(shift_enter);
        let w = Rc::downgrade(self);
        self.search_bar.connect_search_mode_enabled_notify(move |bar| {
            let Some(v) = w.upgrade() else { return };
            if bar.is_search_mode() {
                v.search_entry.grab_focus();
                v.run_search();
            } else {
                v.clear_search();
                gtk::prelude::GtkWindowExt::set_focus(&v.window, None::<&gtk::Widget>);
            }
        });

        let w = Rc::downgrade(self);
        self.sidebar_btn.connect_toggled(move |b| {
            if let Some(v) = w.upgrade() {
                v.sidebar.set_reveal_child(b.is_active());
            }
        });

        // Tooltip do sumário: mostra o título completo só quando está cortado.
        self.toc.set_has_tooltip(true);
        self.toc.connect_query_tooltip(|toc, _x, y, _kbd, tip| {
            let Some(row) = toc.row_at_y(y) else { return false };
            let Some(label) = row.child().and_downcast::<gtk::Label>() else { return false };
            if !label.layout().is_ellipsized() {
                return false;
            }
            tip.set_text(Some(&label.text()));
            if let Some(b) = row.compute_bounds(toc) {
                tip.set_tip_area(&gdk::Rectangle::new(b.x() as i32, b.y() as i32, b.width() as i32, b.height() as i32));
            }
            true
        });

        // Sumário → navegação
        let w = Rc::downgrade(self);
        self.toc.connect_row_activated(move |_, row| {
            if let Some(v) = w.upgrade() {
                v.goto_heading(row.index() as usize);
            }
        });

        // Rolagem → destaque do tópico atual
        let adj = self.scroller.vadjustment();
        // Progresso de leitura e botão "voltar ao topo".
        for prop in ["value", "upper", "page-size"] {
            let w = Rc::downgrade(self);
            adj.connect_notify_local(Some(prop), move |_, _| {
                if let Some(v) = w.upgrade() {
                    v.update_progress();
                }
            });
        }
        // O documento desenha só o trecho visível (ver render::DocRoot):
        // ao rolar, pede um novo desenho para incluir as páginas que entraram.
        let holder = self.holder.downgrade();
        adj.connect_value_changed(move |_| {
            if let Some(root) = holder.upgrade().and_then(|h| h.first_child()) {
                root.queue_draw();
            }
        });
        let w = Rc::downgrade(self);
        adj.connect_value_changed(move |_| {
            if let Some(v) = w.upgrade() {
                {
                    let mut st = v.st.borrow_mut();
                    if st.animating {
                        return;
                    }
                    if st.lock_active {
                        st.lock_active = false;
                    }
                }
                v.update_active();
            }
        });
        let w = Rc::downgrade(self);
        adj.connect_upper_notify(move |adj| {
            if let Some(v) = w.upgrade() {
                let frac = v.st.borrow_mut().pending_frac.take();
                if let Some(f) = frac {
                    adj.set_value(f * adj.upper());
                }
                v.update_active();
            }
        });

        // Tema (global): atualiza botão e cores dos blocos de código.
        if let Some(settings) = gtk::Settings::default() {
            let w = Rc::downgrade(self);
            settings.connect_gtk_application_prefer_dark_theme_notify(move |_| {
                if let Some(v) = w.upgrade() {
                    v.sync_theme_button();
                    v.rehighlight();
                }
            });
            // Troca de tema (do sistema ou pelo botão, ex.: Mint-Y ↔ Mint-Y-Dark).
            let w = Rc::downgrade(self);
            settings.connect_gtk_theme_name_notify(move |_| {
                if let Some(v) = w.upgrade() {
                    v.sync_theme_button();
                    v.rehighlight();
                }
            });
            // Troca de tema de ícones: atualiza os ícones da barra.
            let w = Rc::downgrade(self);
            settings.connect_gtk_icon_theme_name_notify(move |_| {
                if let Some(v) = w.upgrade() {
                    v.sync_theme_button();
                }
            });
        }

        // Ctrl + roda do mouse = zoom
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = Rc::downgrade(self);
        scroll.connect_scroll(move |c, _, dy| {
            if !c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                return glib::Propagation::Proceed;
            }
            if let Some(v) = w.upgrade() {
                if dy < 0.0 {
                    v.zoom_step(1);
                } else if dy > 0.0 {
                    v.zoom_step(-1);
                }
            }
            glib::Propagation::Stop
        });
        self.scroller.add_controller(scroll);

        // Teclas de leitura (PgUp/PgDn, espaço, Home/End, setas)
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            let Some(v) = w.upgrade() else { return glib::Propagation::Proceed };
            // Teclas digitadas na busca pertencem à caixa de texto.
            if let Some(f) = gtk::prelude::GtkWindowExt::focus(&v.window) {
                if f.is_ancestor(&v.search_bar) {
                    return glib::Propagation::Proceed;
                }
            }
            if key == gdk::Key::Escape && v.search_bar.is_search_mode() {
                v.search_bar.set_search_mode(false);
                return glib::Propagation::Stop;
            }
            let adj = v.scroller.vadjustment();
            let page = adj.page_size() * 0.9;
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            match key {
                gdk::Key::Page_Down | gdk::Key::KP_Page_Down => v.animate_to(adj.value() + page),
                gdk::Key::Page_Up | gdk::Key::KP_Page_Up => v.animate_to(adj.value() - page),
                gdk::Key::space if shift => v.animate_to(adj.value() - page),
                gdk::Key::space => v.animate_to(adj.value() + page),
                gdk::Key::Home | gdk::Key::KP_Home => v.animate_to(0.0),
                gdk::Key::End | gdk::Key::KP_End => v.when_loaded(|v| {
                    let adj = v.scroller.vadjustment();
                    v.animate_to(adj.upper());
                }),
                gdk::Key::Down | gdk::Key::KP_Down => v.nudge(56.0),
                gdk::Key::Up | gdk::Key::KP_Up => v.nudge(-56.0),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.window.add_controller(keys);

        // Arrastar e soltar arquivos na janela
        let drop = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY);
        drop.set_types(&[gdk::FileList::static_type(), gio::File::static_type()]);
        let w = Rc::downgrade(self);
        drop.connect_drop(move |_, value, _, _| {
            let file = if let Ok(list) = value.get::<gdk::FileList>() {
                list.files().into_iter().next()
            } else {
                value.get::<gio::File>().ok()
            };
            match (w.upgrade(), file.and_then(|f| f.path())) {
                (Some(v), Some(p)) => {
                    v.load(&p);
                    true
                }
                _ => false,
            }
        });
        self.window.add_controller(drop);
    }

    fn action(self: &Rc<Self>, name: &str, f: impl Fn(&Rc<Viewer>) + 'static) {
        let a = gio::SimpleAction::new(name, None);
        let w = Rc::downgrade(self);
        a.connect_activate(move |_, _| {
            if let Some(v) = w.upgrade() {
                f(&v);
            }
        });
        self.window.add_action(&a);
    }

    // ---------- Conteúdo ----------

    fn set_content(&self, w: &gtk::Widget) {
        self.holder.set_child(w);
    }

    fn show_empty(self: &Rc<Self>) {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 10);
        b.add_css_class("empty-state");
        b.set_valign(gtk::Align::Center);
        b.set_halign(gtk::Align::Center);
        b.set_vexpand(true);
        let icon: gtk::Widget = match themed_image(&[APP_ID]) {
            Some(img) => {
                img.set_pixel_size(96);
                img.add_css_class("empty-logo");
                img.upcast()
            }
            None => {
                let l = gtk::Label::new(Some("M↓"));
                l.add_css_class("empty-icon");
                l.upcast()
            }
        };
        let title = gtk::Label::new(Some("Nenhum documento aberto"));
        title.add_css_class("empty-title");
        let hint = gtk::Label::new(Some(
            "Abra um arquivo .md com Ctrl+O ou arraste-o para esta janela.\nPressione F1 para ver os atalhos de teclado.",
        ));
        hint.add_css_class("empty-hint");
        hint.set_wrap(true);
        hint.set_justify(gtk::Justification::Center);
        let btn = gtk::Button::builder().label("Abrir arquivo…").action_name("win.open").build();
        btn.add_css_class("suggested-action");
        btn.add_css_class("pill");
        btn.set_halign(gtk::Align::Center);
        btn.set_margin_top(10);
        b.append(&icon);
        b.append(&title);
        b.append(&hint);
        b.append(&btn);
        self.set_content(b.upcast_ref());
    }

    fn show_error(&self, path: &Path, err: &str) {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
        b.add_css_class("empty-state");
        b.set_valign(gtk::Align::Center);
        b.set_halign(gtk::Align::Center);
        b.set_vexpand(true);
        let title = gtk::Label::new(Some("Não foi possível abrir o arquivo"));
        title.add_css_class("empty-title");
        let detail = gtk::Label::new(Some(&format!("{}\n{}", path.display(), err)));
        detail.add_css_class("empty-hint");
        detail.set_wrap(true);
        detail.set_selectable(true);
        detail.set_justify(gtk::Justification::Center);
        b.append(&title);
        b.append(&detail);
        self.set_content(b.upcast_ref());
    }

    pub fn load(self: &Rc<Self>, path: &Path) {
        let t0 = Instant::now();
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.file_label.set_text(&name);
        self.file_label.set_tooltip_text(Some(&path.to_string_lossy()));
        self.window.set_title(Some(&format!("{name} — {APP_NAME}")));

        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                self.clear_toc();
                self.st.borrow_mut().doc = None;
                self.st.borrow_mut().path = Some(path.clone());
                let msg = match e.kind() {
                    std::io::ErrorKind::NotFound => "Arquivo não encontrado.".to_string(),
                    std::io::ErrorKind::PermissionDenied => "Permissão negada.".to_string(),
                    _ => e.to_string(),
                };
                self.show_error(&path, &msg);
                self.update_stats();
                self.update_progress();
                return;
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

        let w: Weak<Viewer> = Rc::downgrade(self);
        let handler: LinkHandler = Rc::new(move |uri: &str| {
            let uri = uri.to_string();
            let w = w.clone();
            // Adiado: o widget que emitiu o clique pode ser substituído.
            glib::idle_add_local_once(move || {
                if let Some(v) = w.upgrade() {
                    v.handle_link(&uri);
                }
            });
        });
        let t_parse = Instant::now();
        let doc = render::render(text, path.parent(), handler);
        let t_render = t_parse.elapsed();
        doc.root.set_widget_name(&self.root_name);

        if let Some(id) = self.st.borrow_mut().anim.take() {
            id.remove();
        }
        self.set_content(doc.root.upcast_ref());
        self.build_toc(&doc);
        {
            let mut st = self.st.borrow_mut();
            st.doc = Some(doc);
            st.path = Some(path);
            st.active = None;
            st.animating = false;
            st.lock_active = false;
            st.pending_frac = None;
            st.search = None;
            st.waiter = None;
            st.turbo = false;
        }
        self.scroller.vadjustment().set_value(0.0);
        self.start_appending();
        self.update_stats();
        self.watch_file();
        // O tópico atual só pode ser calculado com o novo conteúdo já posicionado.
        self.after_layout(|v| {
            v.st.borrow_mut().active = None;
            v.update_active();
        });
        if self.search_bar.is_search_mode() {
            self.run_search();
        }
        // Destaque de sintaxe após o primeiro quadro: a janela aparece antes.
        let w = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(v) = w.upgrade() {
                v.rehighlight();
            }
        });
        // Evita que o primeiro rótulo selecionável receba foco (e selecione tudo).
        gtk::prelude::GtkWindowExt::set_focus(&self.window, None::<&gtk::Widget>);

        if std::env::var_os("LEITOR_MD_DEBUG").is_some() {
            eprintln!(
                "[leitor-md] {name}: {} bytes, render {:?}, total {:?}",
                bytes.len(),
                t_render,
                t0.elapsed()
            );
        }
    }

    fn reload(self: &Rc<Self>) {
        let (path, frac, value) = {
            let st = self.st.borrow();
            let adj = self.scroller.vadjustment();
            (st.path.clone(), if adj.upper() > 0.0 { adj.value() / adj.upper() } else { 0.0 }, adj.value())
        };
        if let Some(p) = path {
            let small = std::fs::metadata(&p).map(|m| m.len() < 300_000).unwrap_or(true);
            self.load(&p);
            if small {
                // Documento comum: monta tudo agora e mantém a mesma posição (em
                // pixels: uma edição em outro trecho não mexe no que se lê), sem
                // a tela passar pelo início do documento.
                self.append_pending(usize::MAX);
                self.scroller.vadjustment().set_value(value);
            } else {
                // Documento grande: volta à posição quando estiver completo.
                self.when_loaded(move |v| {
                    let adj = v.scroller.vadjustment();
                    adj.set_value(frac * adj.upper());
                });
            }
        }
    }

    /// Recarrega o documento automaticamente quando o arquivo é salvo por
    /// outro programa (mantendo a posição de leitura).
    fn watch_file(self: &Rc<Self>) {
        let path = self.st.borrow().path.clone();
        let monitor = path.and_then(|p| {
            gio::File::for_path(p).monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE).ok()
        });
        if let Some(m) = &monitor {
            let w = Rc::downgrade(self);
            m.connect_changed(move |_, _, _, event| {
                use gio::FileMonitorEvent as E;
                if !matches!(event, E::Changed | E::ChangesDoneHint | E::Created | E::MovedIn | E::Renamed) {
                    return;
                }
                let Some(v) = w.upgrade() else { return };
                // Editores salvam em vários passos: espera o arquivo assentar.
                if let Some(id) = v.st.borrow_mut().reload_timer.take() {
                    id.remove();
                }
                let w = Rc::downgrade(&v);
                let id = glib::timeout_add_local_once(std::time::Duration::from_millis(350), move || {
                    let Some(v) = w.upgrade() else { return };
                    v.st.borrow_mut().reload_timer = None;
                    let exists = v.st.borrow().path.as_ref().is_some_and(|p| p.is_file());
                    if exists {
                        v.reload();
                        v.flash_status("atualizado");
                    }
                });
                v.st.borrow_mut().reload_timer = Some(id);
            });
        }
        self.st.borrow_mut().monitor = monitor;
    }

    /// Mostra um aviso breve ao lado do nome do arquivo.
    fn flash_status(&self, msg: &str) {
        let name = self.st.borrow().path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string());
        let Some(name) = name else { return };
        self.file_label.set_text(&format!("{name}  ·  {msg}"));
        self.file_label.add_css_class("flash");
        let label = self.file_label.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(1800), move || {
            if let Some(l) = label.upgrade() {
                l.set_text(&name);
                l.remove_css_class("flash");
            }
        });
    }

    // ---------- Progresso de leitura ----------

    fn update_progress(&self) {
        let adj = self.scroller.vadjustment();
        let has_doc = self.st.borrow().doc.is_some();
        let range = adj.upper() - adj.page_size();
        let scrollable = has_doc && range > 1.0;
        let frac = if scrollable { (adj.value() / range).clamp(0.0, 1.0) } else { 1.0 };
        if self.progress.is_visible() != scrollable {
            self.progress.set_visible(scrollable);
        }
        if scrollable {
            self.progress.set_fraction(frac);
        }
        let show_top = has_doc && adj.value() > adj.page_size() * 0.8;
        if self.to_top.reveals_child() != show_top {
            self.to_top.set_reveal_child(show_top);
            self.to_top.set_can_target(show_top);
        }
        // Percentual lido no rodapé do sumário (só se o documento rola).
        let pct = if scrollable { (frac * 100.0).round() as i32 } else { -1 };
        if self.st.borrow().shown_pct != Some(pct) {
            self.st.borrow_mut().shown_pct = Some(pct);
            self.update_stats();
        }
    }

    fn update_stats(&self) {
        let st = self.st.borrow();
        let Some(doc) = st.doc.as_ref() else {
            drop(st);
            return self.stats.set_text("");
        };
        let words = doc.words;
        let minutes = words.div_ceil(200).max(1);
        let mut text = format!("{} palavras · ~{} min de leitura", thousands(words), minutes);
        if let Some(p) = st.shown_pct.filter(|p| *p >= 0) {
            text.push_str(&format!("\n{p}% lido"));
        }
        drop(st);
        self.stats.set_text(&text);
    }

    // ---------- Atalhos ----------

    fn show_shortcuts(&self) {
        const KEYS: &[(&str, &str)] = &[
            ("Ctrl+O", "Abrir arquivo"),
            ("Ctrl+F", "Buscar no documento"),
            ("F3  /  Shift+F3", "Próxima / anterior ocorrência"),
            ("Ctrl +  /  Ctrl −", "Aumentar / diminuir zoom"),
            ("Ctrl+0", "Zoom padrão"),
            ("Ctrl+L", "Largura total / coluna confortável"),
            ("Ctrl + roda do mouse", "Zoom"),
            ("F9  /  Ctrl+B", "Mostrar / ocultar sumário"),
            ("Ctrl+Shift+L", "Tema claro / escuro"),
            ("Espaço  /  PgDn", "Página seguinte"),
            ("Shift+Espaço  /  PgUp", "Página anterior"),
            ("Home  /  End", "Início / fim do documento"),
            ("F5  /  Ctrl+R", "Recarregar arquivo"),
            ("Ctrl+W  /  Ctrl+Q", "Fechar janela / sair"),
            ("F1", "Esta lista de atalhos"),
        ];
        let grid = gtk::Grid::builder().column_spacing(18).row_spacing(8).build();
        grid.add_css_class("shortcuts-grid");
        for (i, (k, d)) in KEYS.iter().enumerate() {
            let key = gtk::Label::new(Some(k));
            key.add_css_class("kbd");
            key.set_halign(gtk::Align::End);
            let desc = gtk::Label::new(Some(d));
            desc.set_xalign(0.0);
            grid.attach(&key, 0, i as i32, 1, 1);
            grid.attach(&desc, 1, i as i32, 1, 1);
        }
        let dialog = gtk::Window::builder()
            .title("Atalhos de teclado")
            .transient_for(&self.window)
            .modal(true)
            .resizable(false)
            .child(&grid)
            .build();
        let keys = gtk::EventControllerKey::new();
        let d = dialog.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            if matches!(key, gdk::Key::Escape | gdk::Key::F1) {
                if let Some(d) = d.upgrade() {
                    d.close();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        dialog.add_controller(keys);
        dialog.present();
    }

    fn choose_file(self: &Rc<Self>) {
        let md = gtk::FileFilter::new();
        md.set_name(Some("Arquivos Markdown"));
        for p in ["*.md", "*.markdown", "*.mdown", "*.mkd", "*.mkdn", "*.mdwn", "*.MD"] {
            md.add_pattern(p);
        }
        md.add_mime_type("text/markdown");
        let txt = gtk::FileFilter::new();
        txt.set_name(Some("Arquivos de texto"));
        txt.add_mime_type("text/plain");
        let all = gtk::FileFilter::new();
        all.set_name(Some("Todos os arquivos"));
        all.add_pattern("*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&md);
        filters.append(&txt);
        filters.append(&all);

        let dialog = gtk::FileDialog::builder()
            .title("Abrir arquivo Markdown")
            .modal(true)
            .filters(&filters)
            .default_filter(&md)
            .build();
        if let Some(dir) = self.st.borrow().path.as_ref().and_then(|p| p.parent()) {
            dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
        }
        let w = Rc::downgrade(self);
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |res| {
            if let (Some(v), Ok(file)) = (w.upgrade(), res) {
                if let Some(p) = file.path() {
                    v.load(&p);
                }
            }
        });
    }

    // ---------- Sumário ----------

    fn clear_toc(&self) {
        self.toc.remove_all();
        // remove_all() também remove o aviso de lista vazia; recoloca-o.
        let placeholder = gtk::Label::new(Some("Sem tópicos"));
        placeholder.add_css_class("toc-empty");
        self.toc.set_placeholder(Some(&placeholder));
    }

    fn build_toc(&self, doc: &Doc) {
        self.clear_toc();
        let min = doc.headings.iter().map(|h| h.level).min().unwrap_or(1);
        for h in &doc.headings {
            let depth = (h.level - min) as i32;
            let l = gtk::Label::new(Some(&h.title));
            l.set_xalign(0.0);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            // Sem tooltip por linha: registrar um tooltip custa ~0,2 ms por
            // widget (150 ms num sumário de 800 tópicos). A lista inteira tem um
            // único tooltip, calculado na hora (ver setup).
            l.set_margin_start(6 + depth * 14);
            l.add_css_class(&format!("toc-l{}", (depth + 1).min(4)));
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&l));
            self.toc.append(&row);
        }
        self.toc_scroll.vadjustment().set_value(0.0);
    }

    /// Posição vertical de um widget dentro da área rolável.
    fn y_of(&self, w: &gtk::Widget) -> Option<f64> {
        w.compute_point(&self.holder, &graphene::Point::new(0.0, 0.0)).map(|p| p.y() as f64)
    }

    fn update_active(&self) {
        let idx = {
            let st = self.st.borrow();
            if st.lock_active || st.animating {
                return;
            }
            let Some(doc) = &st.doc else { return };
            let hs = &doc.headings;
            if hs.is_empty() {
                return;
            }
            let adj = self.scroller.vadjustment();
            // No fim do documento, títulos que já não podem chegar ao topo da
            // janela contam como lidos se estiverem visíveis.
            let scrolled = adj.value() > 0.0 && adj.upper() > adj.page_size() + 1.0;
            let at_bottom = scrolled && doc.is_done() && adj.value() + adj.page_size() >= adj.upper() - 2.0;
            let y = if at_bottom {
                adj.value() + adj.page_size() - ACTIVE_OFFSET
            } else {
                adj.value() + ACTIVE_OFFSET
            };
            // Busca binária: último título cuja posição está acima da linha de
            // leitura. Só os títulos já montados (um prefixo da lista) contam.
            let built = hs.partition_point(|h| h.widget.is_some());
            let (mut lo, mut hi) = (0usize, built);
            while lo < hi {
                let mid = (lo + hi) / 2;
                match hs[mid].widget.as_ref().and_then(|w| self.y_of(w)) {
                    Some(p) if p <= y => lo = mid + 1,
                    _ => hi = mid,
                }
            }
            if lo == 0 { None } else { Some(lo - 1) }
        };
        self.set_active(idx);
    }

    fn set_active(&self, idx: Option<usize>) {
        if self.st.borrow().active == idx {
            return;
        }
        self.st.borrow_mut().active = idx;
        // A seleção visual é aplicada uma vez, no próximo ciclo ocioso, com o
        // valor final: selecionar e desselecionar linhas em sequência no mesmo
        // ciclo deixava, no tema Mint-Y, uma linha com o estilo de selecionada
        // preso (texto branco sem fundo). Também poupa trabalho ao rolar rápido.
        if self.toc_sel.replace(Some(idx)).is_some() {
            return; // já agendado; só o valor final importa
        }
        let (sel, toc, scroll) = (self.toc_sel.clone(), self.toc.downgrade(), self.toc_scroll.downgrade());
        glib::idle_add_local_once(move || {
            let (Some(idx), Some(toc), Some(scroll)) = (sel.take(), toc.upgrade(), scroll.upgrade()) else { return };
            match idx.and_then(|i| toc.row_at_index(i as i32)) {
                Some(row) => {
                    toc.select_row(Some(&row));
                    // Mantém o item destacado visível no sumário.
                    if let Some(p) = row.compute_point(&toc, &graphene::Point::new(0.0, 0.0)) {
                        let adj = scroll.vadjustment();
                        let (y, h) = (p.y() as f64, row.height() as f64);
                        if y < adj.value() {
                            adj.set_value(y - 8.0);
                        } else if y + h > adj.value() + adj.page_size() {
                            adj.set_value(y + h - adj.page_size() + 8.0);
                        }
                    }
                }
                None => toc.unselect_all(),
            }
        });
    }

    fn goto_heading(self: &Rc<Self>, i: usize) {
        let widget = move |v: &Viewer| {
            let st = v.st.borrow();
            st.doc.as_ref().and_then(|d| d.headings.get(i)).and_then(|h| h.widget.clone())
        };
        if self.st.borrow().doc.as_ref().is_none_or(|d| i >= d.headings.len()) {
            return;
        }
        self.set_active(Some(i));
        self.st.borrow_mut().lock_active = true;
        match widget(self) {
            Some(w) => self.scroll_to_widget(&w),
            // Título num trecho ainda em montagem: acelera a montagem e rola
            // assim que ele existir (a janela continua respondendo).
            None => self.wait_for(move |v| match widget(v) {
                Some(w) => {
                    v.after_layout(move |v| v.scroll_to_widget(&w));
                    true
                }
                None => !v.has_pending(),
            }),
        }
    }

    fn goto_anchor(self: &Rc<Self>, name: &str) {
        let name = render::percent_decode(name);
        let lookup = |v: &Viewer| -> Option<Anchor> {
            let st = v.st.borrow();
            let doc = st.doc.as_ref()?;
            doc.anchors
                .get(&name)
                .or_else(|| doc.anchors.get(&name.to_lowercase()))
                .or_else(|| doc.anchors.get(&render::slugify(&name)))
                .cloned()
        };
        match lookup(self) {
            Some(Anchor::Heading(i)) => self.goto_heading(i),
            Some(Anchor::Widget(w)) => self.scroll_to_widget(&w),
            // Pode ser uma âncora de um trecho ainda não montado (ex.: nota de
            // rodapé): tenta de novo com o documento completo.
            None if self.has_pending() => {
                let name = name.clone();
                self.when_loaded(move |v| v.goto_anchor(&name));
            }
            None => {}
        }
    }

    // ---------- Busca ----------

    fn open_search(&self) {
        self.search_bar.set_search_mode(true);
        self.search_entry.grab_focus();
        self.search_entry.select_region(0, -1);
    }

    fn clear_search(&self) {
        if let Some(s) = self.st.borrow_mut().search.take() {
            s.clear();
        }
        self.search_count.set_text("");
    }

    fn run_search(self: &Rc<Self>) {
        self.clear_search();
        let query = self.search_entry.text().to_string();
        if query.trim().is_empty() {
            return;
        }
        // A busca precisa do documento inteiro: se ainda está carregando,
        // acelera a montagem e busca assim que terminar.
        if self.has_pending() {
            self.search_count.set_text("Carregando…");
            return self.when_loaded(|v| v.run_search());
        }
        let root = match self.st.borrow().doc.as_ref() {
            Some(d) => d.root.clone(),
            None => return,
        };
        let mut s = Search::run(root.upcast_ref(), &query);
        // Começa pela primeira ocorrência a partir do trecho que está sendo lido.
        let top = self.scroller.vadjustment().value();
        s.select_from(|m| self.match_y(m), top);
        self.st.borrow_mut().search = Some(s);
        self.update_search_count();
        self.scroll_to_current_match();
    }

    fn find_step(self: &Rc<Self>, dir: i32) {
        if !self.search_bar.is_search_mode() {
            return self.open_search();
        }
        {
            let mut st = self.st.borrow_mut();
            let Some(s) = st.search.as_mut() else { return };
            s.step(dir);
        }
        self.update_search_count();
        self.scroll_to_current_match();
    }

    fn update_search_count(&self) {
        let st = self.st.borrow();
        let text = match st.search.as_ref() {
            Some(s) if s.matches.is_empty() => "Nenhum resultado".to_string(),
            Some(s) => format!("{} de {}", s.current + 1, s.matches.len()),
            None => String::new(),
        };
        self.search_count.set_text(&text);
    }

    /// Posição vertical (na área rolável) do início de uma ocorrência.
    fn match_y(&self, m: &Match) -> Option<f64> {
        let layout = m.label.layout();
        let rect = layout.index_to_pos(m.start as i32);
        let (_, oy) = m.label.layout_offsets();
        let y = oy as f64 + rect.y() as f64 / gtk::pango::SCALE as f64;
        m.label
            .compute_point(&self.holder, &graphene::Point::new(0.0, y as f32))
            .map(|p| p.y() as f64)
    }

    /// Rola até a ocorrência atual, se ela não estiver bem visível.
    fn scroll_to_current_match(self: &Rc<Self>) {
        let y = {
            let st = self.st.borrow();
            let Some(m) = st.search.as_ref().and_then(|s| s.matches.get(s.current)) else { return };
            self.match_y(m)
        };
        let Some(y) = y else { return };
        let adj = self.scroller.vadjustment();
        let (top, page) = (adj.value(), adj.page_size());
        if y < top + 40.0 || y > top + page - 80.0 {
            self.st.borrow_mut().lock_active = false;
            self.animate_to(y - page * 0.3);
        }
    }

    // ---------- Rolagem ----------

    fn scroll_to_widget(self: &Rc<Self>, w: &gtk::Widget) {
        // Recém-criado e ainda sem posição: espera o próximo layout.
        if w.height() == 0 {
            let w = w.clone();
            return self.after_layout(move |v| {
                if let Some(y) = v.y_of(&w) {
                    v.animate_to(y - 16.0);
                }
            });
        }
        if let Some(y) = self.y_of(w) {
            self.animate_to(y - 16.0);
        }
    }

    /// Executa `f` depois que o GTK tiver diagramado o próximo quadro
    /// (posições de widgets recém-criados já valem).
    fn after_layout(self: &Rc<Self>, f: impl FnOnce(&Rc<Viewer>) + 'static) {
        let w = Rc::downgrade(self);
        let f = RefCell::new(Some(f));
        let frames = Cell::new(0);
        self.window.add_tick_callback(move |_, _| {
            frames.set(frames.get() + 1);
            if frames.get() < 2 {
                return glib::ControlFlow::Continue;
            }
            if let (Some(v), Some(f)) = (w.upgrade(), f.borrow_mut().take()) {
                f(&v);
            }
            glib::ControlFlow::Break
        });
    }

    // ---------- Inserção gradual ----------

    /// Registra uma ação que espera a montagem do documento (ver `waiter`).
    /// A montagem é acelerada até a ação ser cumprida; a janela continua
    /// respondendo (antes, a montagem era feita de uma vez e a janela travava
    /// por segundos em documentos grandes).
    fn wait_for(self: &Rc<Self>, mut f: impl FnMut(&Rc<Viewer>) -> bool + 'static) {
        if f(self) {
            return;
        }
        let mut st = self.st.borrow_mut();
        st.waiter = Some(Box::new(f));
        st.turbo = true;
    }

    /// Executa `f` (com o layout já atualizado) quando o documento estiver
    /// completamente montado — na hora, se já estiver.
    fn when_loaded(self: &Rc<Self>, f: impl FnOnce(&Rc<Viewer>) + 'static) {
        let mut f = Some(f);
        self.wait_for(move |v| {
            if v.has_pending() {
                return false;
            }
            if let Some(f) = f.take() {
                v.after_layout(f);
            }
            true
        });
    }

    /// Chamado a cada lote montado: cumpre a ação em espera, se possível.
    fn poll_waiter(self: &Rc<Self>) {
        let Some(mut f) = self.st.borrow_mut().waiter.take() else { return };
        if f(self) {
            self.st.borrow_mut().turbo = false;
        } else {
            let mut st = self.st.borrow_mut();
            // Outra ação pode ter sido registrada durante a chamada.
            if st.waiter.is_none() {
                st.waiter = Some(f);
            }
        }
    }

    /// Monta mais `budget` caracteres do documento. Retorna `true` se algo
    /// foi criado.
    fn append_pending(&self, budget: usize) -> bool {
        self.st.borrow_mut().doc.as_mut().map(|d| d.step(budget)).unwrap_or(false)
    }

    fn has_pending(&self) -> bool {
        self.st.borrow().doc.as_ref().is_some_and(|d| !d.is_done())
    }

    /// Insere o restante do documento um pouco a cada quadro: o início
    /// aparece imediatamente e a janela continua respondendo.
    fn start_appending(self: &Rc<Self>) {
        let generation = {
            let mut st = self.st.borrow_mut();
            st.generation += 1;
            st.generation
        };
        if !self.has_pending() {
            return;
        }
        let w = Rc::downgrade(self);
        let t0 = Instant::now();
        let first = Cell::new(true);
        // Quantidade de texto por quadro, ajustada pelo tempo real de cada
        // quadro (criação + layout, que o GTK faz depois deste callback): mira
        // ~25 ms, para a rolagem continuar fluida enquanto o documento carrega,
        // em computadores rápidos ou lentos.
        let per_frame = Cell::new(CHARS_PER_FRAME);
        let last_tick: Cell<Option<Instant>> = Cell::new(None);
        self.window.add_tick_callback(move |_, _| {
            let Some(v) = w.upgrade() else { return glib::ControlFlow::Break };
            if v.st.borrow().generation != generation {
                return glib::ControlFlow::Break;
            }
            // O GTK chama este callback antes de pintar o quadro: no primeiro
            // quadro não fazemos nada, para a tela inicial aparecer o quanto antes.
            if first.replace(false) {
                return glib::ControlFlow::Continue;
            }
            let now = Instant::now();
            if let Some(prev) = last_tick.replace(Some(now)) {
                let dt = now - prev;
                let n = per_frame.get() as f64;
                let n = if dt > FRAME_TARGET * 3 / 2 {
                    n * 0.7
                } else if dt < FRAME_TARGET {
                    n * 1.2
                } else {
                    n
                };
                per_frame.set((n as usize).clamp(MIN_CHARS_PER_FRAME, MAX_CHARS_PER_FRAME));
            }
            let turbo = v.st.borrow().turbo;
            v.append_pending(if turbo { MAX_CHARS_PER_FRAME } else { per_frame.get() });
            v.poll_waiter();
            if v.has_pending() {
                return glib::ControlFlow::Continue;
            }
            if std::env::var_os("LEITOR_MD_DEBUG").is_some() {
                eprintln!("[leitor-md] documento completo na tela em {:?}", t0.elapsed());
            }
            glib::ControlFlow::Break
        });
    }

    fn nudge(&self, dy: f64) {
        if let Some(id) = self.st.borrow_mut().anim.take() {
            id.remove();
        }
        self.st.borrow_mut().animating = false;
        let adj = self.scroller.vadjustment();
        adj.set_value(adj.value() + dy);
    }

    /// Rolagem suave (ease-out) até a posição indicada.
    fn animate_to(self: &Rc<Self>, target: f64) {
        let adj = self.scroller.vadjustment();
        let target = target.clamp(adj.lower(), (adj.upper() - adj.page_size()).max(adj.lower()));
        if let Some(id) = self.st.borrow_mut().anim.take() {
            id.remove();
        }
        let start = adj.value();
        if (target - start).abs() < 1.0 || !self.scroller.settings().is_gtk_enable_animations() {
            self.st.borrow_mut().animating = false;
            adj.set_value(target);
            return;
        }
        self.st.borrow_mut().animating = true;
        let w = Rc::downgrade(self);
        let t0: Cell<Option<i64>> = Cell::new(None);
        let id = self.scroller.add_tick_callback(move |_, clock| {
            let now = clock.frame_time();
            let t0 = match t0.get() {
                Some(t) => t,
                None => {
                    t0.set(Some(now));
                    now
                }
            };
            let p = ((now - t0) as f64 / SCROLL_ANIM_US).min(1.0);
            let e = 1.0 - (1.0 - p).powi(3);
            adj.set_value(start + (target - start) * e);
            if p >= 1.0 {
                if let Some(v) = w.upgrade() {
                    let mut st = v.st.borrow_mut();
                    st.animating = false;
                    st.anim = None;
                    let locked = st.lock_active;
                    drop(st);
                    if !locked {
                        v.update_active();
                    }
                }
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
        self.st.borrow_mut().anim = Some(id);
    }

    // ---------- Zoom ----------

    fn zoom_step(&self, dir: i32) {
        let z = self.zoom.get();
        let next = if dir > 0 {
            ZOOM_STEPS.iter().copied().find(|s| *s > z + 0.001)
        } else {
            ZOOM_STEPS.iter().rev().copied().find(|s| *s < z - 0.001)
        };
        if let Some(n) = next {
            self.apply_zoom(n);
        }
    }

    /// Alterna entre texto na largura total e coluna confortável, mantendo a
    /// posição de leitura.
    fn set_full_width(&self, full: bool) {
        let adj = self.scroller.vadjustment();
        if adj.upper() > 0.0 {
            self.st.borrow_mut().pending_frac = Some(adj.value() / adj.upper());
        }
        self.holder.set_fill(full);
    }

    fn apply_zoom(&self, z: f64) {
        let adj = self.scroller.vadjustment();
        if adj.upper() > 0.0 && (z - self.zoom.get()).abs() > 0.001 {
            self.st.borrow_mut().pending_frac = Some(adj.value() / adj.upper());
        }
        self.zoom.set(z);
        self.zoom_css
            .load_from_string(&format!("#{} {{ font-size: {:.2}pt; }}", self.root_name, BASE_PT * z));
        self.holder.set_max_width((BASE_MAX_WIDTH * z).round() as i32);
        self.zoom_btn.set_label(&format!("{}%", (z * 100.0).round() as i32));
    }

    // ---------- Tema ----------

    fn sync_theme_button(&self) {
        let dark = theme::is_dark();
        if dark {
            set_icon(self.theme_btn.upcast_ref(), &["weather-clear-symbolic", "display-brightness-symbolic"], "☀");
        } else {
            set_icon(self.theme_btn.upcast_ref(), &["weather-clear-night-symbolic"], "☾");
        }
        self.theme_btn.set_tooltip_text(Some(if dark {
            "Usar tema claro (Ctrl+Shift+L)"
        } else {
            "Usar tema escuro (Ctrl+Shift+L)"
        }));
    }

    fn rehighlight(&self) {
        let dark = theme::is_dark();
        let st = self.st.borrow();
        if let Some(doc) = &st.doc {
            for cb in &doc.code_blocks {
                cb.label.set_markup(&highlight::code_markup(&cb.code, &cb.lang, dark));
            }
        }
    }

    // ---------- Links ----------

    fn handle_link(self: &Rc<Self>, uri: &str) {
        if let Some(frag) = uri.strip_prefix('#') {
            return self.goto_anchor(frag);
        }
        let has_scheme = uri
            .split_once(':')
            .map(|(s, _)| {
                !s.is_empty()
                    && s.len() > 1 // evita "C:" e similares
                    && s.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
            })
            .unwrap_or(false);
        if has_scheme && !uri.starts_with("file:") {
            return open_external(uri);
        }
        // Link local (relativo ao documento atual ou file://)
        let (p, frag) = match uri.split_once('#') {
            Some((p, f)) => (p, Some(f)),
            None => (uri, None),
        };
        let path = if uri.starts_with("file:") {
            gio::File::for_uri(p).path()
        } else {
            let rel = PathBuf::from(render::percent_decode(p));
            let base = self.st.borrow().path.as_ref().and_then(|x| x.parent().map(Path::to_path_buf));
            Some(match base {
                Some(b) if rel.is_relative() => b.join(rel),
                _ => rel,
            })
        };
        let Some(path) = path else { return };
        if !path.exists() {
            return;
        }
        let is_md = path
            .extension()
            .map(|e| matches!(e.to_string_lossy().to_lowercase().as_str(), "md" | "markdown" | "mdown" | "mkd" | "mkdn"))
            .unwrap_or(false);
        if is_md {
            self.load(&path);
            if let Some(f) = frag {
                // Pula para a âncora depois do primeiro layout do novo documento.
                let f = f.to_string();
                self.after_layout(move |v| v.goto_anchor(&f));
            }
        } else {
            open_external(&gio::File::for_path(&path).uri());
        }
    }
}

/// Coluna de leitura. Por padrão o texto ocupa toda a largura disponível
/// (com uma margem lateral); no modo "coluna confortável" ele fica
/// centralizado, com largura máxima proporcional ao zoom.
mod column {
    use super::SIDE_PAD;
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    mod imp {
        use super::*;
        use std::cell::Cell;

        #[derive(Default)]
        pub struct ReadingColumn {
            pub max_width: Cell<i32>,
            /// Texto na largura total (sem a largura máxima).
            pub fill: Cell<bool>,
            /// Largura com que a coluna foi exibida pela última vez (0 = nunca).
            pub alloc_width: Cell<i32>,
            /// Altura do conteúdo medida nessa largura.
            pub last_height: Cell<i32>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for ReadingColumn {
            const NAME: &'static str = "LeitorReadingColumn";
            type Type = super::ReadingColumn;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for ReadingColumn {
            fn dispose(&self) {
                while let Some(c) = self.obj().first_child() {
                    c.unparent();
                }
            }
        }

        impl ReadingColumn {
            fn col_width(&self, avail: i32, min: i32) -> i32 {
                let w = avail - 2 * SIDE_PAD;
                let w = if self.fill.get() { w } else { w.min(self.max_width.get()) };
                w.max(min).max(1)
            }
        }

        impl WidgetImpl for ReadingColumn {
            fn request_mode(&self) -> gtk::SizeRequestMode {
                gtk::SizeRequestMode::HeightForWidth
            }

            // O conteúdo nunca exige largura mínima (o texto quebra; tabelas e
            // código têm rolagem própria), então não medimos a largura dele.
            //
            // Altura: o GTK pergunta a altura do documento em várias larguras
            // (livre, largura mínima da janela…), mas só a largura em que a
            // coluna é exibida importa. Medir as outras custava, num documento
            // grande, ~60 ms por quadro (o texto inteiro rediagramado em larguras
            // que nunca aparecem). Essas consultas recebem a última altura real.
            //
            // A medida real continua sendo feita: (1) antes da primeira exibição,
            // para qualquer largura; (2) sempre na largura atual de exibição; e
            // (3) depois de um redimensionamento (ver size_allocate). Uma versão
            // anterior respondia *zero* às consultas especulativas, inclusive
            // antes da primeira exibição, e o sumário às vezes não rolava.
            fn measure(&self, orient: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
                let Some(child) = self.obj().first_child() else { return (0, 0, -1, -1) };
                if orient == gtk::Orientation::Horizontal {
                    return (0, self.max_width.get() + 2 * SIDE_PAD, -1, -1);
                }
                let shown = self.alloc_width.get();
                if shown > 0 && for_size != shown {
                    let h = self.last_height.get();
                    return (h, h, -1, -1);
                }
                let cw = if for_size < 0 { self.max_width.get() } else { self.col_width(for_size, 0) };
                // O viewport usa a altura mínima; pedimos a natural para que
                // imagens (que podem encolher) apareçam em tamanho real.
                let (_, nat, _, _) = child.measure(gtk::Orientation::Vertical, cw);
                if shown > 0 {
                    self.last_height.set(nat);
                }
                (nat, nat, -1, -1)
            }

            fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
                let Some(child) = self.obj().first_child() else { return };
                if self.alloc_width.replace(width) != width {
                    // Nova largura: a altura usada neste quadro pode estar
                    // desatualizada; mede de verdade no próximo.
                    let (_, nat, _, _) = child.measure(gtk::Orientation::Vertical, self.col_width(width, 0));
                    self.last_height.set(nat);
                    if nat != height {
                        let obj = self.obj().clone();
                        gtk::glib::idle_add_local_once(move || obj.queue_resize());
                    }
                }
                let cw = self.col_width(width, 0);
                let x = ((width - cw) / 2).max(0);
                child.size_allocate(&gtk::Allocation::new(x, 0, cw, height), -1);
            }
        }
    }

    glib::wrapper! {
        pub struct ReadingColumn(ObjectSubclass<imp::ReadingColumn>)
            @extends gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl ReadingColumn {
        pub fn new(max_width: i32) -> Self {
            let o: Self = glib::Object::new();
            o.imp().max_width.set(max_width);
            o.imp().fill.set(true);
            o
        }
        pub fn set_fill(&self, fill: bool) {
            if self.imp().fill.replace(fill) != fill {
                self.queue_resize();
            }
        }
        pub fn set_child(&self, w: &gtk::Widget) {
            while let Some(c) = self.first_child() {
                c.unparent();
            }
            w.set_parent(self);
            // Conteúdo novo: a primeira medida é sempre real.
            self.imp().alloc_width.set(0);
        }
        pub fn set_max_width(&self, v: i32) {
            self.imp().max_width.set(v);
            self.queue_resize();
        }
    }
}

/// 12345 → "12.345".
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    out
}

/// Ícone do tema do sistema (o primeiro nome disponível), se houver.
pub(crate) fn themed_image(names: &[&str]) -> Option<gtk::Image> {
    let display = gdk::Display::default()?;
    let theme = gtk::IconTheme::for_display(&display);
    names.iter().find(|n| theme.has_icon(n)).map(|n| gtk::Image::from_icon_name(n))
}

/// Coloca no botão o ícone do tema do sistema ou, se nenhum existir,
/// um símbolo de texto (funciona em qualquer instalação).
fn set_icon(button: &gtk::Button, names: &[&str], glyph: &str) {
    match themed_image(names) {
        Some(img) => {
            button.set_child(Some(&img));
            button.remove_css_class("glyph");
        }
        None => {
            button.set_label(glyph);
            button.add_css_class("glyph");
        }
    }
}

/// Abre um URI no aplicativo padrão do sistema (navegador, etc.).
///
/// Dentro do AppImage o ambiente é ajustado para as bibliotecas embarcadas;
/// restauramos o ambiente original para que o navegador do sistema não as herde.
pub fn open_external(uri: &str) {
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(uri);
    if std::env::var_os("LEITOR_MD_APPDIR").is_some() {
        for var in crate::APPIMAGE_ENV_VARS {
            match std::env::var_os(format!("LEITOR_MD_ORIG_{var}")) {
                Some(v) => cmd.env(var, v),
                None => cmd.env_remove(var),
            };
        }
        for (k, _) in std::env::vars_os() {
            if k.to_string_lossy().starts_with("LEITOR_MD_") {
                cmd.env_remove(k);
            }
        }
    }
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(_) => {
            let _ = gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>);
        }
    }
}
