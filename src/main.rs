//! Leitor Markdown — visualizador de arquivos .md leve, nativo (GTK4) e só de leitura.

mod highlight;
mod hscroll;
mod imagerow;
mod markerrow;
mod render;
mod search;
mod theme;
mod window;

use gtk::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "io.github.leitormd.LeitorMarkdown";
pub const APP_NAME: &str = "Leitor Markdown";

/// Variáveis que o AppRun do AppImage altera (o original é guardado em
/// `LEITOR_MD_ORIG_<VAR>` e restaurado ao abrir links em outros programas).
pub const APPIMAGE_ENV_VARS: [&str; 9] = [
    "LD_LIBRARY_PATH",
    "XDG_DATA_DIRS",
    "GSETTINGS_SCHEMA_DIR",
    "GDK_PIXBUF_MODULE_FILE",
    "GTK_PATH",
    "GTK_EXE_PREFIX",
    "GTK_DATA_PREFIX",
    "GIO_MODULE_DIR",
    "GTK_IM_MODULE_FILE",
];

const STYLE: &str = include_str!("style.css");

/// Instante em que o processo começou (para medir o tempo até a janela aparecer).
static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Com LEITOR_MD_DEBUG, informa quanto tempo levou até o primeiro quadro da
/// janela; com LEITOR_MD_BENCH, além disso fecha o leitor logo em seguida
/// (permite medir a abertura completa com `time`, inclusive a do AppImage).
fn report_first_frame(window: &gtk::ApplicationWindow) {
    // LEITOR_MD_FRAMES: imprime o intervalo entre quadros (diagnóstico de fluidez).
    if std::env::var_os("LEITOR_MD_FRAMES").is_some() {
        window.connect_realize(|w| {
            let Some(fc) = w.frame_clock() else { return };
            let last = std::cell::Cell::new(0i64);
            fc.connect_after_paint(move |fc| {
                let t = fc.frame_time();
                if last.get() > 0 {
                    eprintln!("[leitor-md] quadro {:.1} ms", (t - last.get()) as f64 / 1000.0);
                }
                last.set(t);
            });
        });
    }
    let debug = std::env::var_os("LEITOR_MD_DEBUG").is_some();
    let bench = std::env::var_os("LEITOR_MD_BENCH").is_some();
    if !debug && !bench {
        return;
    }
    // Mede no fim do primeiro quadro desenhado (sinal after-paint do relógio de quadros).
    window.connect_realize(move |w| {
        let Some(fc) = w.frame_clock() else { return };
        let w = w.clone();
        let id: std::rc::Rc<std::cell::Cell<Option<glib::SignalHandlerId>>> = Default::default();
        let id2 = id.clone();
        let fc2 = fc.clone();
        let h = fc.connect_after_paint(move |_| {
            if let Some(t) = START.get() {
                eprintln!(
                    "[leitor-md] janela pronta em {:.0} ms (desde o início do processo)",
                    t.elapsed().as_secs_f64() * 1000.0
                );
            }
            if let Some(h) = id2.take() {
                fc2.disconnect(h);
            }
            if bench {
                if let Some(app) = w.application() {
                    app.quit();
                }
            }
        });
        id.set(Some(h));
    });
}

fn usage() {
    println!(
        "{APP_NAME} {ver}\n\n\
         Uso: leitor-md [ARQUIVO.md ...]\n\n\
         Atalhos:\n  \
         Ctrl+O        abrir arquivo\n  \
         Ctrl+ / Ctrl- aumentar / diminuir zoom\n  \
         Ctrl+0        zoom padrão\n  \
         Ctrl+L        largura total / coluna confortável\n  \
         F9 / Ctrl+B   mostrar/ocultar sumário\n  \
         F5 / Ctrl+R   recarregar arquivo\n  \
         Ctrl+F        buscar no documento (F3 / Shift+F3: próxima / anterior)\n  \
         F1            lista de atalhos\n  \
         Ctrl+W        fechar janela\n  \
         Ctrl+Q        sair",
        ver = env!("CARGO_PKG_VERSION")
    );
}

fn main() -> glib::ExitCode {
    START.get_or_init(std::time::Instant::now);
    let args: Vec<String> = std::env::args().collect();
    if args.iter().skip(1).any(|a| a == "--help" || a == "-h") {
        usage();
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().skip(1).any(|a| a == "--version" || a == "-V") {
        println!("{APP_NAME} {}", env!("CARGO_PKG_VERSION"));
        return glib::ExitCode::SUCCESS;
    }

    // Renderizador por software (Cairo): para um leitor de texto é rápido o
    // bastante e consome bem menos memória que o renderizador OpenGL
    // (medido: ~110 MB vs ~165 MB de RSS). Pode ser trocado com GSK_RENDERER=ngl.
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", "cairo");
    }

    glib::set_application_name(APP_NAME);
    glib::set_prgname(Some("leitor-md"));

    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_startup(|app| {
        let css = gtk::CssProvider::new();
        css.load_from_string(STYLE);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        gtk::Window::set_default_icon_name(APP_ID);
        theme::init();

        let quit = gio::SimpleAction::new("quit", None);
        let a = app.clone();
        quit.connect_activate(move |_, _| a.quit());
        app.add_action(&quit);

        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action("win.close", &["<Control>w"]);
        app.set_accels_for_action("win.open", &["<Control>o"]);
        app.set_accels_for_action("win.reload", &["F5", "<Control>r"]);
        app.set_accels_for_action(
            "win.zoom-in",
            &["<Control>plus", "<Control>equal", "<Control>KP_Add", "<Control><Shift>plus"],
        );
        app.set_accels_for_action("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract", "<Control>underscore"]);
        app.set_accels_for_action("win.zoom-reset", &["<Control>0", "<Control>KP_0"]);
        app.set_accels_for_action("win.toggle-width", &["<Control>l"]);
        app.set_accels_for_action("win.toggle-sidebar", &["F9", "<Control>b"]);
        app.set_accels_for_action("win.toggle-theme", &["<Control><Shift>l"]);
        app.set_accels_for_action("win.find", &["<Control>f"]);
        app.set_accels_for_action("win.shortcuts", &["F1", "<Control>question"]);
        app.set_accels_for_action("win.find-next", &["F3", "<Control>g"]);
        app.set_accels_for_action("win.find-prev", &["<Shift>F3", "<Control><Shift>g"]);
    });

    app.connect_activate(|app| {
        if let Some(w) = app.active_window() {
            w.present();
        } else {
            let v = window::Viewer::new(app);
            report_first_frame(&v.window);
            v.window.present();
        }
    });

    app.connect_open(|app, files, _| {
        for f in files {
            let v = window::Viewer::new(app);
            match f.path() {
                Some(p) => v.load(&p),
                None => v.load(std::path::Path::new(&f.uri().to_string())),
            }
            report_first_frame(&v.window);
            v.window.present();
        }
    });

    app.run()
}
