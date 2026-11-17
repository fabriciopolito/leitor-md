//! Converte Markdown (CommonMark + extensões GFM) em widgets GTK nativos.
//!
//! Não há navegador embutido: cada bloco vira um widget leve —
//! parágrafos e títulos são `GtkLabel` com markup Pango, tabelas são
//! `GtkGrid`, imagens são `GtkPicture`, e assim por diante.

use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub type LinkHandler = Rc<dyn Fn(&str)>;

pub struct Heading {
    pub level: u8,
    pub title: String,
    /// Criado quando o trecho do documento que contém o título é montado.
    pub widget: Option<gtk::Widget>,
}

/// Destino de um link interno (`#âncora`).
#[derive(Clone)]
pub enum Anchor {
    /// Índice em `Document::headings`.
    Heading(usize),
    /// Outro elemento já criado (ex.: nota de rodapé).
    Widget(gtk::Widget),
}

pub struct CodeBlock {
    pub label: gtk::Label,
    pub code: String,
    pub lang: String,
}

pub struct Document {
    pub root: gtk::Box,
    /// Todos os títulos do documento (conhecidos desde o início, por uma
    /// pré-leitura rápida), na ordem em que aparecem.
    pub headings: Vec<Heading>,
    pub anchors: HashMap<String, Anchor>,
    /// Blocos de código já criados.
    pub code_blocks: Vec<CodeBlock>,
    /// Número de palavras do documento (para o tempo estimado de leitura).
    pub words: usize,
}

/// Documento em montagem. O Markdown inteiro é analisado de uma vez (rápido:
/// ~6 ms para 1 MB), mas os widgets são criados aos poucos: só o começo antes
/// do primeiro quadro, o restante em lotes (ver `step`). Criar os ~13 mil
/// widgets de um documento grande custava ~350 ms antes de a janela aparecer.
pub struct Doc {
    r: Renderer,
    events: std::vec::IntoIter<Event<'static>>,
    done: bool,
}

impl std::ops::Deref for Doc {
    type Target = Document;
    fn deref(&self) -> &Document {
        &self.r.doc
    }
}

impl Doc {
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Monta o próximo trecho do documento, com cerca de `budget` caracteres
    /// de texto, num contêiner próprio (a raiz fica com poucos filhos e o GTK
    /// não reavalia o documento inteiro a cada lote). Retorna `true` se algo
    /// foi criado.
    pub fn step(&mut self, budget: usize) -> bool {
        if self.done {
            return false;
        }
        // Páginas de tamanho limitado (o desenho descarta páginas fora da tela,
        // então uma página gigante anularia esse ganho).
        let mut left = budget;
        while left > 0 && !self.done {
            let chunk = left.min(PAGE_CHARS);
            self.step_page(chunk);
            left -= chunk;
        }
        true
    }

    fn step_page(&mut self, budget: usize) {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.add_css_class("md-page");
        self.r.doc.root.append(&page);
        self.r.stack[0] = page;
        self.r.img_row = None;
        let mut used = 0;
        for ev in self.events.by_ref() {
            self.r.event(ev);
            used += std::mem::take(&mut self.r.weight);
            if used >= budget && self.r.at_block_boundary() {
                break;
            }
        }
        if self.events.len() == 0 {
            self.r.flush("p");
            self.done = true;
            // Libera a lista de eventos (já consumida, mas ainda alocada).
            self.events = Vec::new().into_iter();
            self.r.footnotes = HashMap::new();
        }
        // Daqui em diante os blocos de código já nascem coloridos.
        self.r.highlight_now = true;
    }
}

const H_CLASSES: [&str; 6] = ["h1", "h2", "h3", "h4", "h5", "h6"];
const BULLETS: [&str; 3] = ["•", "◦", "▪"];

fn esc(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

/// Texto inline acumulado: markup Pango + texto puro (fallback) + tags abertas.
#[derive(Default)]
struct Inline {
    markup: String,
    plain: String,
    open: Vec<(String, &'static str)>,
}

impl Inline {
    fn text(&mut self, t: &str) {
        self.markup.push_str(&esc(t));
        self.plain.push_str(t);
    }
    fn raw(&mut self, markup: &str, plain: &str) {
        self.markup.push_str(markup);
        self.plain.push_str(plain);
    }
    fn open(&mut self, tag: String, close: &'static str) {
        self.markup.push_str(&tag);
        self.open.push((tag, close));
    }
    fn close(&mut self) {
        if let Some((_, c)) = self.open.pop() {
            self.markup.push_str(c);
        }
    }
    fn is_blank(&self) -> bool {
        self.plain.trim().is_empty()
    }
    /// Retira o conteúdo acumulado, fechando as tags abertas e
    /// reabrindo-as no buffer seguinte (útil quando uma imagem divide um link).
    fn take(&mut self) -> (String, String) {
        let mut m = std::mem::take(&mut self.markup);
        for (_, c) in self.open.iter().rev() {
            m.push_str(c);
        }
        for (o, _) in &self.open {
            self.markup.push_str(o);
        }
        (m, std::mem::take(&mut self.plain))
    }
}

struct Table {
    grid: gtk::Grid,
    aligns: Vec<Alignment>,
    row: i32,
    col: i32,
    head: bool,
}

struct Renderer {
    base: Option<PathBuf>,
    on_link: LinkHandler,
    stack: Vec<gtk::Box>,
    inl: Inline,
    lists: Vec<Option<u64>>,
    marker: Option<gtk::Label>,
    heading: Option<(u8, Option<String>)>,
    code: Option<(String, String)>,
    html: Option<String>,
    table: Option<Table>,
    image: Option<(String, String)>,
    links: Vec<String>,
    img_row: Option<crate::imagerow::ImageRow>,
    in_meta: bool,
    footnotes: HashMap<String, usize>,
    doc: Document,
    /// Texto processado desde a última consulta (orçamento de `Doc::step`).
    weight: usize,
    /// Próximo título (índice em `doc.headings`) a receber widget.
    next_heading: usize,
    /// Colorir blocos de código já na criação (depois do primeiro trecho).
    highlight_now: bool,
}

/// Quanto texto é montado antes do primeiro quadro (o bastante para encher
/// a janela; o restante vem nos quadros seguintes).
const FIRST_SCREEN_CHARS: usize = 4_000;
/// Texto máximo por página (contêiner) do documento.
const PAGE_CHARS: usize = 6_000;

pub fn render(src: &str, base: Option<&Path>, on_link: LinkHandler) -> Doc {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let events: Vec<Event<'static>> = Parser::new_ext(src, opts).map(Event::into_static).collect();
    let (headings, anchors) = prescan(&events);

    let root: gtk::Box = doc_root::DocRoot::new().upcast();
    root.add_css_class("md-content");
    enable_lazy_selection(&root);
    let r = Renderer {
        base: base.map(Path::to_path_buf),
        on_link,
        stack: vec![root.clone()],
        inl: Inline::default(),
        lists: Vec::new(),
        marker: None,
        heading: None,
        code: None,
        html: None,
        table: None,
        image: None,
        links: Vec::new(),
        img_row: None,
        in_meta: false,
        footnotes: HashMap::new(),
        doc: Document { root, headings, anchors, code_blocks: Vec::new(), words: count_words(src) },
        weight: 0,
        next_heading: 0,
        highlight_now: false,
    };
    let mut doc = Doc { r, events: events.into_iter(), done: false };
    doc.step(FIRST_SCREEN_CHARS);
    doc
}

/// Contagem aproximada de palavras (só para o tempo estimado de leitura).
fn count_words(src: &str) -> usize {
    src.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count()
}

// ---------- Raiz do documento ----------

/// Contêiner do documento (uma `GtkBox` vertical de páginas) que desenha só as
/// páginas próximas da área visível. O renderizador Cairo (escolhido por usar
/// ~50 MB a menos de memória) não descarta o que está fora da tela: sem isto,
/// cada quadro de rolagem num documento grande desenhava o documento inteiro
/// (~50 ms por quadro, ~20 quadros por segundo).
///
/// O descarte fica no `snapshot` deste widget (e não no do pai) para que o GTK
/// continue sabendo quando redesenhar: mudanças dentro do documento (hover,
/// "Copiado!", destaques da busca) invalidam este widget normalmente.
mod doc_root {
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct DocRoot;

        #[glib::object_subclass]
        impl ObjectSubclass for DocRoot {
            const NAME: &'static str = "LeitorDocRoot";
            type Type = super::DocRoot;
            type ParentType = gtk::Box;
        }

        impl ObjectImpl for DocRoot {}
        impl BoxImpl for DocRoot {}

        impl WidgetImpl for DocRoot {
            fn snapshot(&self, snapshot: &gtk::Snapshot) {
                let obj = self.obj();
                // Faixa visível, em coordenadas deste widget (com uma tela de
                // margem acima e duas abaixo, para a rolagem não mostrar vazios).
                let range = obj.ancestor(gtk::Viewport::static_type()).and_then(|vp| {
                    let h = vp.height() as f32;
                    obj.compute_point(&vp, &gtk::graphene::Point::new(0.0, 0.0))
                        .map(|p| (-p.y() - h, -p.y() + 2.0 * h))
                });
                let mut c = obj.first_child();
                while let Some(ch) = c {
                    c = ch.next_sibling();
                    if let (Some((top, bottom)), Some(b)) = (range, ch.compute_bounds(&*obj)) {
                        if b.y() + b.height() < top {
                            continue;
                        }
                        if b.y() > bottom {
                            break;
                        }
                    }
                    obj.snapshot_child(&ch, snapshot);
                }
            }
        }
    }

    glib::wrapper! {
        pub struct DocRoot(ObjectSubclass<imp::DocRoot>)
            @extends gtk::Box, gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
    }

    impl DocRoot {
        pub fn new() -> Self {
            glib::Object::builder().property("orientation", gtk::Orientation::Vertical).build()
        }
    }
}

// ---------- Pré-leitura dos títulos ----------
//
// Lê os eventos do Markdown sem criar widgets e reproduz exatamente as regras
// do renderizador para títulos (texto, títulos vazios ignorados, títulos em
// HTML, âncoras). Assim o sumário fica completo desde o primeiro quadro, e o
// índice de cada título coincide com o do renderizador.

fn prescan(events: &[Event<'static>]) -> (Vec<Heading>, HashMap<String, Anchor>) {
    let mut headings = Vec::new();
    let mut anchors = HashMap::new();
    let mut slugs: HashMap<String, usize> = HashMap::new();
    let mut footnotes: HashMap<String, usize> = HashMap::new();
    let mut in_meta = false;
    let mut in_code = false;
    let mut html: Option<String> = None;
    let mut image: Option<String> = None;
    let mut heading: Option<(u8, Option<String>, String)> = None;

    let mut add = |level: u8, title: String, id: Option<String>, headings: &mut Vec<Heading>| {
        let anchor = id.unwrap_or_else(|| unique_slug(&mut slugs, &title));
        anchors.insert(anchor, Anchor::Heading(headings.len()));
        headings.push(Heading { level, title, widget: None });
    };
    let footnote_num = |fns: &mut HashMap<String, usize>, name: &str| {
        let n = fns.len() + 1;
        *fns.entry(name.to_string()).or_insert(n)
    };

    for ev in events {
        if in_meta {
            in_meta = !matches!(ev, Event::End(TagEnd::MetadataBlock(_)));
            continue;
        }
        if in_code {
            in_code = !matches!(ev, Event::End(TagEnd::CodeBlock));
            continue;
        }
        if let Some(h) = &mut html {
            match ev {
                Event::Html(t) | Event::Text(t) | Event::InlineHtml(t) => h.push_str(t),
                Event::End(TagEnd::HtmlBlock) => {
                    let raw = strip_comments(&html.take().unwrap_or_default());
                    let (text, level) = html_text_and_level(&raw);
                    if let (Some(level), false) = (level, text.is_empty()) {
                        add(level, normalize(&text), attr(&raw, "id"), &mut headings);
                    }
                }
                _ => {}
            }
            continue;
        }
        if let Some(alt) = &mut image {
            match ev {
                Event::Text(t) | Event::Code(t) => alt.push_str(t),
                Event::End(TagEnd::Image) => {
                    let alt = image.take().unwrap_or_default();
                    if let Some((_, _, p)) = &mut heading {
                        p.push_str(&alt);
                    }
                }
                _ => {}
            }
            continue;
        }
        match ev {
            Event::Start(Tag::MetadataBlock(_)) => in_meta = true,
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::Start(Tag::HtmlBlock) => html = Some(String::new()),
            Event::Start(Tag::Image { .. }) => image = Some(String::new()),
            Event::Start(Tag::Heading { level, id, .. }) => {
                heading = Some(((*level as u8).clamp(1, 6), id.as_ref().map(|s| s.to_string()), String::new()))
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, id, p)) = heading.take() {
                    let title = normalize(&p);
                    if !title.is_empty() {
                        add(level, title, id, &mut headings);
                    }
                }
            }
            Event::Start(Tag::FootnoteDefinition(name)) => {
                footnote_num(&mut footnotes, name);
            }
            Event::FootnoteReference(name) => {
                let n = footnote_num(&mut footnotes, name);
                if let Some((_, _, p)) = &mut heading {
                    p.push_str(&format!("[{n}]"));
                }
            }
            Event::Text(t) | Event::Code(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                if let Some((_, _, p)) = &mut heading {
                    p.push_str(t);
                }
            }
            Event::SoftBreak => {
                if let Some((_, _, p)) = &mut heading {
                    p.push(' ');
                }
            }
            Event::HardBreak => {
                if let Some((_, _, p)) = &mut heading {
                    p.push('\n');
                }
            }
            Event::InlineHtml(h) | Event::Html(h) => {
                if let Some((_, _, p)) = &mut heading {
                    p.push_str(&inline_html_plain(h));
                }
            }
            _ => {}
        }
    }
    (headings, anchors)
}

/// Texto puro que um trecho de HTML inline acrescenta a um título
/// (mesmas regras de `Renderer::inline_html` dentro de títulos).
fn inline_html_plain(h: &str) -> String {
    let t = h.trim();
    if t.starts_with("<!--") {
        return String::new();
    }
    let lower = t.to_lowercase();
    if lower.starts_with("<img") {
        let src = attr(t, "src").unwrap_or_default();
        return if src.is_empty() { String::new() } else { attr(t, "alt").unwrap_or_default() };
    }
    let name: String = lower
        .trim_start_matches('<')
        .trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    if name == "br" { "\n".into() } else { String::new() }
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn unique_slug(slugs: &mut HashMap<String, usize>, text: &str) -> String {
    let base = slugify(text);
    let n = slugs.entry(base.clone()).or_insert(0);
    let s = if *n == 0 { base } else { format!("{base}-{n}") };
    *n += 1;
    s
}

/// Texto visível e nível de título (h1…h6) de um bloco HTML.
fn html_text_and_level(raw: &str) -> (String, Option<u8>) {
    let text = decode_entities(&strip_tags(&raw.replace("<br>", "\n").replace("<br/>", "\n")));
    let text = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
    let lower = raw.to_lowercase();
    let first_tag: String = lower
        .trim_start()
        .trim_start_matches('<')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    let level = match first_tag.as_str() {
        "h1" => Some(1u8),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    };
    (text, level)
}

/// Rótulos selecionáveis custam memória e tempo de criação (~20 % num documento
/// grande). Em vez de torná-los todos selecionáveis, um único controlador de
/// movimento ativa a seleção no rótulo que está sob o ponteiro.
fn enable_lazy_selection(root: &gtk::Box) {
    let motion = gtk::EventControllerMotion::new();
    let r = root.clone();
    let mut last: Option<glib::WeakRef<gtk::Label>> = None;
    let mut handle = move |x: f64, y: f64| {
        let mut w = r.pick(x, y, gtk::PickFlags::DEFAULT);
        while let Some(cur) = w {
            if let Some(l) = cur.downcast_ref::<gtk::Label>() {
                if last.as_ref().and_then(|x| x.upgrade()).as_ref() == Some(l) {
                    return;
                }
                if !l.is_selectable() {
                    l.set_selectable(true);
                }
                last = Some(l.downgrade());
                return;
            }
            if cur == *r.upcast_ref::<gtk::Widget>() {
                return;
            }
            w = cur.parent();
        }
    };
    let h = std::rc::Rc::new(std::cell::RefCell::new(move |x, y| handle(x, y)));
    let h2 = h.clone();
    motion.connect_enter(move |_, x, y| (h2.borrow_mut())(x, y));
    motion.connect_motion(move |_, x, y| (h.borrow_mut())(x, y));
    root.add_controller(motion);
}

impl Renderer {
    fn top(&self) -> &gtk::Box {
        self.stack.last().expect("pilha de containers vazia")
    }

    /// Insere um widget no contêiner atual.
    fn append(&mut self, w: &impl IsA<gtk::Widget>) {
        self.top().append(w);
    }

    /// Entre dois blocos de nível superior (nenhum bloco pela metade)?
    fn at_block_boundary(&self) -> bool {
        self.stack.len() == 1
            && self.lists.is_empty()
            && self.heading.is_none()
            && self.code.is_none()
            && self.html.is_none()
            && self.table.is_none()
            && self.image.is_none()
            && self.inl.is_blank()
    }

    /// Associa o widget de um título à próxima entrada do sumário.
    fn bind_heading(&mut self, title: &str, w: gtk::Widget) {
        if let Some(h) = self.doc.headings.get_mut(self.next_heading) {
            if h.title != title && std::env::var_os("LEITOR_MD_DEBUG").is_some() {
                eprintln!("[leitor-md] aviso: título {:?} ≠ pré-leitura {:?}", title, h.title);
            }
            h.widget = Some(w);
        }
        self.next_heading += 1;
    }

    fn push(&mut self, b: gtk::Box) {
        self.append(&b);
        self.stack.push(b);
    }

    fn pop(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
        self.img_row = None;
    }

    fn label(&self, markup: &str, plain: &str, class: &str) -> gtk::Label {
        let l = gtk::Label::new(None);
        l.set_wrap(true);
        l.set_wrap_mode(pango::WrapMode::WordChar);
        // Sem width_chars o GTK calcula a largura mínima diagramando o texto
        // com largura 0 (uma letra por linha), o que tem custo quadrático no
        // tamanho do parágrafo. Com ele, a mínima é calculada em 12 caracteres.
        l.set_width_chars(12);
        l.set_xalign(0.0);
        // Seleção é ativada sob demanda (ver enable_lazy_selection).
        l.add_css_class(class);
        if markup.contains("<a ") {
            // Links precisam do parser de markup do GTK.
            if markup_is_valid(markup) {
                l.set_markup(markup);
            } else {
                l.set_text(plain);
            }
        } else if !markup.contains('<') && !markup.contains('&') {
            l.set_text(markup);
        } else {
            // Um único parse: texto + atributos.
            match pango::parse_markup(markup, '\0') {
                Ok((attrs, text, _)) => {
                    l.set_text(&text);
                    l.set_attributes(Some(&attrs));
                }
                Err(_) => l.set_text(plain),
            }
        }
        if markup.contains("<a ") {
            let h = self.on_link.clone();
            l.connect_activate_link(move |_, uri| {
                h(uri);
                glib::Propagation::Stop
            });
        }
        l
    }

    /// Emite o texto inline acumulado como um parágrafo (ou descarta se vazio).
    fn flush(&mut self, class: &str) {
        if self.inl.is_blank() {
            self.inl.take();
            return;
        }
        let (m, p) = self.inl.take();
        self.weight += p.len();
        let l = self.label(m.trim_end(), p.trim_end(), class);
        self.append(&l);
        self.img_row = None;
    }

    fn footnote_num(&mut self, name: &str) -> usize {
        let n = self.footnotes.len() + 1;
        *self.footnotes.entry(name.to_string()).or_insert(n)
    }


    fn event(&mut self, ev: Event) {
        if self.in_meta {
            if matches!(ev, Event::End(TagEnd::MetadataBlock(_))) {
                self.in_meta = false;
            }
            return;
        }
        if let Some((_, buf)) = &mut self.code {
            match ev {
                Event::Text(t) => return buf.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {}
                _ => return,
            }
        }
        if let Some(h) = &mut self.html {
            match ev {
                Event::Html(t) | Event::Text(t) | Event::InlineHtml(t) => return h.push_str(&t),
                Event::End(TagEnd::HtmlBlock) => {}
                _ => return,
            }
        }
        if let Some((_, alt)) = &mut self.image {
            match &ev {
                Event::Text(t) | Event::Code(t) => return alt.push_str(t),
                Event::End(TagEnd::Image) => {}
                _ => return,
            }
        }

        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                if !t.trim().is_empty() {
                    self.img_row = None;
                }
                self.inl.text(&t);
            }
            Event::Code(t) => {
                self.img_row = None;
                let m = format!(
                    "<span font_family=\"monospace\" background=\"#8c8c8c\" bgalpha=\"22%\">\u{2009}{}\u{2009}</span>",
                    esc(&t)
                );
                self.inl.raw(&m, &t);
            }
            Event::SoftBreak => self.inl.text(" "),
            Event::HardBreak => self.inl.raw("\n", "\n"),
            Event::Rule => {
                self.flush("p");
                let s = gtk::Separator::new(gtk::Orientation::Horizontal);
                s.add_css_class("hr");
                self.append(&s);
            }
            Event::TaskListMarker(done) => {
                if let Some(m) = &self.marker {
                    m.set_text(if done { "☑" } else { "☐" });
                    m.add_css_class("task");
                }
            }
            Event::FootnoteReference(name) => {
                let n = self.footnote_num(&name);
                let m = format!("<sup><a href=\"#fn-{}\">[{n}]</a></sup>", esc(&name));
                self.inl.raw(&m, &format!("[{n}]"));
            }
            Event::InlineHtml(h) | Event::Html(h) => self.inline_html(&h),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.flush("p"),
            Tag::Heading { level, id, .. } => {
                self.flush("p");
                self.heading = Some((level as u8, id.map(|s| s.to_string())));
            }
            Tag::BlockQuote(kind) => {
                self.flush("p");
                let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
                b.add_css_class("blockquote");
                if let Some(k) = kind {
                    let (class, title) = match k {
                        BlockQuoteKind::Note => ("alert-note", "Nota"),
                        BlockQuoteKind::Tip => ("alert-tip", "Dica"),
                        BlockQuoteKind::Important => ("alert-important", "Importante"),
                        BlockQuoteKind::Warning => ("alert-warning", "Atenção"),
                        BlockQuoteKind::Caution => ("alert-caution", "Cuidado"),
                    };
                    b.add_css_class("alert");
                    b.add_css_class(class);
                    let t = gtk::Label::new(Some(title));
                    t.set_xalign(0.0);
                    t.add_css_class("alert-title");
                    b.append(&t);
                }
                self.push(b);
            }
            Tag::CodeBlock(kind) => {
                self.flush("p");
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                        .next()
                        .unwrap_or("")
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::HtmlBlock => {
                self.flush("p");
                self.html = Some(String::new());
            }
            Tag::List(start) => {
                self.flush("p");
                let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
                b.add_css_class("list");
                self.push(b);
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush("p");
                let depth = self.lists.len().max(1);
                let text = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let s = format!("{n}.");
                        *n += 1;
                        s
                    }
                    _ => BULLETS[(depth - 1) % BULLETS.len()].to_string(),
                };
                let marker = gtk::Label::new(Some(&text));
                marker.add_css_class("list-marker");
                marker.set_xalign(1.0);
                let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
                let row = crate::markerrow::MarkerRow::new(&marker, &content);
                row.add_css_class("list-item");
                self.append(&row);
                self.marker = Some(marker);
                self.stack.push(content);
                self.img_row = None;
            }
            Tag::FootnoteDefinition(name) => {
                self.flush("p");
                let n = self.footnote_num(&name);
                let marker = gtk::Label::new(Some(&format!("[{n}]")));
                marker.add_css_class("footnote-marker");
                let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
                let row = crate::markerrow::MarkerRow::new(&marker, &content);
                row.add_css_class("footnote");
                self.append(&row);
                self.doc.anchors.insert(format!("fn-{name}"), Anchor::Widget(row.upcast()));
                self.stack.push(content);
            }
            Tag::Table(aligns) => {
                self.flush("p");
                let grid = gtk::Grid::new();
                grid.add_css_class("md-table");
                grid.set_halign(gtk::Align::Start);
                grid.set_valign(gtk::Align::Start);
                let wrap = crate::hscroll::HScroll::new(&grid);
                wrap.add_css_class("md-table-wrap");
                self.append(&wrap);
                self.table = Some(Table { grid, aligns, row: 0, col: 0, head: false });
            }
            Tag::TableHead => {
                if let Some(t) = &mut self.table {
                    t.head = true;
                    t.col = 0;
                }
            }
            Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.col = 0;
                }
            }
            Tag::TableCell => {
                self.inl.take();
            }
            Tag::Emphasis => self.inl.open("<i>".into(), "</i>"),
            Tag::Strong => self.inl.open("<b>".into(), "</b>"),
            Tag::Strikethrough => self.inl.open("<s>".into(), "</s>"),
            Tag::Superscript => self.inl.open("<sup>".into(), "</sup>"),
            Tag::Subscript => self.inl.open("<sub>".into(), "</sub>"),
            Tag::Link { dest_url, title, .. } => {
                let mut t = format!("<a href=\"{}\"", esc(&dest_url));
                if !title.is_empty() {
                    t.push_str(&format!(" title=\"{}\"", esc(&title)));
                }
                t.push('>');
                self.inl.open(t, "</a>");
                self.links.push(dest_url.to_string());
            }
            Tag::Image { dest_url, .. } => {
                self.image = Some((dest_url.to_string(), String::new()));
            }
            Tag::MetadataBlock(_) => self.in_meta = true,
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush("p"),
            TagEnd::Heading(_) => self.finish_heading(),
            TagEnd::BlockQuote(_) => {
                self.flush("p");
                self.pop();
            }
            TagEnd::CodeBlock => {
                if let Some((lang, code)) = self.code.take() {
                    self.code_block(lang, code);
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(h) = self.html.take() {
                    self.html_block(&h);
                }
            }
            TagEnd::List(_) => {
                self.flush("p");
                self.pop();
                self.lists.pop();
            }
            TagEnd::Item => {
                self.flush("p");
                self.pop();
                self.marker = None;
            }
            TagEnd::FootnoteDefinition => {
                self.flush("p");
                self.pop();
            }
            TagEnd::Table => self.table = None,
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.row += 1;
                    t.head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = &mut self.table {
                    t.row += 1;
                }
            }
            TagEnd::TableCell => self.table_cell(),
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript => self.inl.close(),
            TagEnd::Link => {
                self.inl.close();
                self.links.pop();
            }
            TagEnd::Image => {
                if let Some((url, alt)) = self.image.take() {
                    self.inline_image(&url, &alt);
                }
            }
            _ => {}
        }
    }

    fn finish_heading(&mut self) {
        let (level, id) = self.heading.take().unwrap_or((1, None));
        let (m, p) = self.inl.take();
        let title = p.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.is_empty() {
            return;
        }
        self.weight += title.len();
        let lvl = level.clamp(1, 6);
        let l = self.label(m.trim(), &title, H_CLASSES[lvl as usize - 1]);
        self.append(&l);
        self.img_row = None;
        let _ = id; // âncora já registrada na pré-leitura
        self.bind_heading(&title, l.upcast());
    }

    fn table_cell(&mut self) {
        let (m, p) = self.inl.take();
        let Some((head, row, col, align)) = self
            .table
            .as_ref()
            .map(|t| (t.head, t.row, t.col, t.aligns.get(t.col as usize).copied()))
        else {
            return;
        };
        self.weight += p.len() + 20;
        let l = self.label(m.trim(), p.trim(), if head { "th" } else { "td" });
        l.set_max_width_chars(48);
        l.set_yalign(0.0);
        if !head && row % 2 == 0 {
            l.add_css_class("alt");
        }
        let (x, j) = match align {
            Some(Alignment::Center) => (0.5, gtk::Justification::Center),
            Some(Alignment::Right) => (1.0, gtk::Justification::Right),
            _ => (0.0, gtk::Justification::Left),
        };
        l.set_xalign(x);
        l.set_justify(j);
        if let Some(t) = &mut self.table {
            t.grid.attach(&l, col, row, 1, 1);
            t.col += 1;
        }
    }

    fn code_block(&mut self, lang: String, code: String) {
        let code = code.trim_end_matches('\n').to_string();
        self.weight += code.len() / 2;
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_yalign(0.0);
        label.set_selectable(true);
        label.add_css_class("code");
        if self.highlight_now {
            label.set_markup(&crate::highlight::code_markup(&code, &lang, crate::theme::is_dark()));
        } else {
            // Primeiro trecho: cores aplicadas logo após o primeiro quadro
            // (ver Viewer::rehighlight), para a janela aparecer antes.
            label.set_text(&code);
        }
        let sw = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&label)
            .build();
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add_css_class("code-block");
        frame.append(&code_header(&lang, &code));
        frame.append(&sw);
        self.append(&frame);
        self.img_row = None;
        self.doc.code_blocks.push(CodeBlock { label, code, lang });
    }

    // ---------- Imagens ----------

    fn resolve_local(&self, url: &str) -> Option<PathBuf> {
        let url = url.split(['?', '#']).next().unwrap_or(url);
        let path = if let Some(p) = url.strip_prefix("file://") {
            PathBuf::from(percent_decode(p))
        } else if url.contains("://") || url.starts_with("data:") {
            return None;
        } else {
            let p = PathBuf::from(percent_decode(url));
            match (p.is_absolute(), self.base.as_deref()) {
                (true, _) => p,
                (false, Some(b)) => b.join(p),
                (false, None) => p,
            }
        };
        Some(path)
    }

    fn image_widget(&self, url: &str, alt: &str) -> gtk::Widget {
        if let Some(path) = self.resolve_local(url) {
            if let Ok(tex) = gdk::Texture::from_filename(&path) {
                let pic = gtk::Picture::for_paintable(&tex);
                pic.set_can_shrink(true);
                pic.set_content_fit(gtk::ContentFit::ScaleDown);
                pic.set_halign(gtk::Align::Start);
                pic.add_css_class("md-image");
                if !alt.is_empty() {
                    pic.set_tooltip_text(Some(alt));
                }
                return pic.upcast();
            }
        }
        // Imagem remota (não baixamos nada) ou arquivo ausente: mostra o texto alternativo.
        let shown = if alt.trim().is_empty() { url } else { alt };
        let l = self.label(
            &format!("▣ <a href=\"{}\">{}</a>", esc(url), esc(shown)),
            &format!("▣ {shown}"),
            "img-alt",
        );
        l.set_wrap(false);
        l.set_ellipsize(pango::EllipsizeMode::End);
        l.upcast()
    }

    fn add_image(&mut self, url: &str, alt: &str, center: bool) {
        let w = self.image_widget(url, alt);
        if let Some(target) = self.links.last().cloned() {
            if w.is::<gtk::Picture>() {
                let h = self.on_link.clone();
                let click = gtk::GestureClick::new();
                click.connect_released(move |_, _, _, _| h(&target));
                w.add_controller(click);
                w.set_cursor_from_name(Some("pointer"));
            }
        }
        if self.img_row.is_none() {
            let row = crate::imagerow::ImageRow::new(center);
            row.add_css_class("img-row");
            self.append(&row);
            self.img_row = Some(row);
        }
        if let Some(row) = &self.img_row {
            row.append(&w);
        }
    }

    fn inline_image(&mut self, url: &str, alt: &str) {
        if self.heading.is_some() || self.table.is_some() {
            self.inl.text(alt);
            return;
        }
        // Texto anterior vira um parágrafo; links abertos continuam valendo depois.
        if !self.inl.is_blank() {
            let (m, p) = self.inl.take();
            let l = self.label(m.trim_end(), p.trim_end(), "p-part");
            self.append(&l);
            self.img_row = None;
        } else {
            self.inl.take();
        }
        self.add_image(url, alt, false);
    }

    // ---------- HTML embutido (subconjunto seguro) ----------

    fn inline_html(&mut self, h: &str) {
        let t = h.trim();
        if t.starts_with("<!--") {
            return;
        }
        let lower = t.to_lowercase();
        if lower.starts_with("<img") {
            let src = attr(t, "src").unwrap_or_default();
            let alt = attr(t, "alt").unwrap_or_default();
            if !src.is_empty() {
                self.inline_image(&src, &alt);
            }
            return;
        }
        let closing = lower.starts_with("</");
        let name: String = lower
            .trim_start_matches('<')
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if name == "br" {
            return self.inl.raw("\n", "\n");
        }
        let (open, close): (&str, &'static str) = match name.as_str() {
            "b" | "strong" => ("<b>", "</b>"),
            "i" | "em" | "cite" => ("<i>", "</i>"),
            "u" | "ins" => ("<u>", "</u>"),
            "s" | "del" | "strike" => ("<s>", "</s>"),
            "sub" => ("<sub>", "</sub>"),
            "sup" => ("<sup>", "</sup>"),
            "kbd" | "code" | "tt" | "samp" | "var" => ("<span font_family=\"monospace\">", "</span>"),
            "mark" => ("<span background=\"#ffe066\" bgalpha=\"55%\">", "</span>"),
            _ => return,
        };
        if closing {
            if self.inl.open.last().map(|(_, c)| *c == close).unwrap_or(false) {
                self.inl.close();
            }
        } else if !lower.ends_with("/>") {
            self.inl.open(open.to_string(), close);
        }
    }

    fn html_block(&mut self, raw: &str) {
        let raw = strip_comments(raw);
        let lower = raw.to_lowercase();
        let center = lower.contains("align=\"center\"") || lower.contains("align=center");
        self.img_row = None;

        // Imagens
        let mut rest = raw.as_str();
        while let Some(i) = rest.to_ascii_lowercase().find("<img") {
            let tail = &rest[i..];
            let end = tail.find('>').map(|e| e + 1).unwrap_or(tail.len());
            let tag = &tail[..end];
            if let Some(src) = attr(tag, "src") {
                let alt = attr(tag, "alt").unwrap_or_default();
                self.add_image(&src, &alt, center);
            }
            rest = &tail[end..];
        }

        // Texto (tags removidas). Títulos HTML <h1>…<h6> entram no sumário.
        let (text, level) = html_text_and_level(&raw);
        if text.is_empty() {
            return;
        }
        self.weight += text.len();
        let class = level.map(|l| H_CLASSES[l as usize - 1]).unwrap_or("p");
        let l = self.label(&esc(&text), &text, class);
        if center {
            l.set_xalign(0.5);
            l.set_justify(gtk::Justification::Center);
        }
        self.append(&l);
        if level.is_some() {
            self.bind_heading(&normalize(&text), l.upcast());
        }
    }
}

// ---------- Utilitários ----------

/// Cabeçalho de um bloco de código: linguagem à esquerda e botão "Copiar" à
/// direita. O botão copia o código exatamente como está no arquivo.
fn code_header(lang: &str, code: &str) -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.add_css_class("code-header");
    let tag = gtk::Label::new(Some(&lang.to_lowercase()));
    tag.set_xalign(0.0);
    tag.set_hexpand(true);
    tag.add_css_class("code-lang");
    header.append(&tag);

    let caption = gtk::Label::new(Some("Copiar"));
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let icon = crate::window::themed_image(&["edit-copy-symbolic"]);
    if let Some(img) = &icon {
        content.append(img);
    }
    content.append(&caption);
    let button = gtk::Button::builder()
        .child(&content)
        .tooltip_text("Copiar o código para a área de transferência")
        .valign(gtk::Align::Center)
        .build();
    button.add_css_class("flat");
    button.add_css_class("copy-btn");
    button.set_cursor_from_name(Some("pointer"));
    let code = code.to_string();
    // Ícone de confirmação (✓) do tema, se houver.
    let done_icon = ["object-select-symbolic", "emblem-ok-symbolic", "checkbox-checked-symbolic"]
        .into_iter()
        .find(|n| gtk::gdk::Display::default().is_some_and(|d| gtk::IconTheme::for_display(&d).has_icon(n)));
    let pending: std::rc::Rc<std::cell::Cell<Option<glib::SourceId>>> = Default::default();
    button.connect_clicked(move |b| {
        b.clipboard().set_text(&code);
        caption.set_text("Copiado!");
        if let (Some(img), Some(n)) = (&icon, done_icon) {
            img.set_icon_name(Some(n));
        }
        b.add_css_class("copied");
        // Cliques repetidos reiniciam o tempo da confirmação.
        if let Some(id) = pending.take() {
            id.remove();
        }
        let (cap, btn, img, p2) = (caption.downgrade(), b.downgrade(), icon.as_ref().map(|i| i.downgrade()), pending.clone());
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(1600), move || {
            p2.set(None);
            if let (Some(c), Some(b)) = (cap.upgrade(), btn.upgrade()) {
                c.set_text("Copiar");
                b.remove_css_class("copied");
            }
            if let Some(i) = img.and_then(|i| i.upgrade()) {
                i.set_icon_name(Some("edit-copy-symbolic"));
            }
        });
        pending.set(Some(id));
    });
    header.append(&button);
    header
}

/// Valida markup antes de entregá-lo ao GtkLabel. O Pango não conhece a tag
/// `<a>` (extensão do GTK), então ela é removida apenas para a validação.
fn markup_is_valid(markup: &str) -> bool {
    if !markup.contains("<a ") {
        return pango::parse_markup(markup, '\0').is_ok();
    }
    let mut check = String::with_capacity(markup.len());
    let mut rest = markup;
    while let Some(i) = rest.find('<') {
        check.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail.find('>') else { return false };
        let tag = &tail[..=end];
        if !(tag.starts_with("<a ") || tag == "</a>") {
            check.push_str(tag);
        }
        rest = &tail[end + 1..];
    }
    check.push_str(rest);
    pango::parse_markup(&check, '\0').is_ok()
}

/// Âncora no estilo do GitHub: minúsculas, espaços → '-', remove pontuação.
pub fn slugify(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                Some(c)
            } else if c.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find(name) {
        let i = from + pos;
        from = i + name.len();
        let before_ok = i == 0 || lower.as_bytes()[i - 1].is_ascii_whitespace();
        let after = lower[from..].trim_start();
        if !before_ok || !after.starts_with('=') {
            continue;
        }
        let val_start = tag.len() - after.len() + 1;
        let v = tag[val_start..].trim_start();
        let (q, body) = match v.chars().next() {
            Some(q @ ('"' | '\'')) => (Some(q), &v[1..]),
            _ => (None, v),
        };
        let end = match q {
            Some(q) => body.find(q).unwrap_or(body.len()),
            None => body.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(body.len()),
        };
        return Some(decode_entities(&body[..end]));
    }
    None
}

fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        match rest[i..].find("-->") {
            Some(e) => rest = &rest[i + e + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&copy;", "©")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("Instalação no Linux!"), "instalação-no-linux");
        assert_eq!(slugify("2.1. Abrir arquivos"), "21-abrir-arquivos");
    }

    #[test]
    fn decode() {
        assert_eq!(percent_decode("meu%20arquivo.md"), "meu arquivo.md");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn attrs() {
        assert_eq!(attr("<img src=\"a.png\" alt='x y'>", "src").as_deref(), Some("a.png"));
        assert_eq!(attr("<img src=\"a.png\" alt='x y'>", "alt").as_deref(), Some("x y"));
        assert_eq!(attr("<img data-src=\"b\" src=c.png>", "src").as_deref(), Some("c.png"));
    }
}
