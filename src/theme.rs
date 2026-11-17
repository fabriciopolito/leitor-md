//! Tema: usa o tema GTK do sistema (no Linux Mint, Mint-Y / Mint-L / Mint-X),
//! com alternância claro/escuro.
//!
//! - Se o tema do sistema tem versão para GTK4 (pasta `gtk-4.0`), ele é usado,
//!   e mudanças feitas nas configurações do sistema são seguidas automaticamente.
//!   O botão claro/escuro troca para a variante correspondente
//!   (ex.: `Mint-Y-Aqua` ↔ `Mint-Y-Dark-Aqua`), ou usa o `gtk-dark.css` do tema.
//! - Se não tem, cai para o tema embutido do GTK4 ("Default"), e a preferência
//!   claro/escuro vem do portal XDG (`color-scheme`) ou do nome do tema.

use gtk::glib::Variant;
use gtk::gio;
use gtk::prelude::*;
use std::cell::Cell;
use std::path::PathBuf;

thread_local! {
    /// Depois que o usuário alterna manualmente, deixamos de seguir o sistema.
    static USER_OVERRIDE: Cell<bool> = const { Cell::new(false) };
    /// Usando o tema embutido do GTK (o tema do sistema não suporta GTK4).
    static FALLBACK: Cell<bool> = const { Cell::new(false) };
}

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_IFACE: &str = "org.freedesktop.portal.Settings";

/// Temas que o próprio GTK4 traz embutidos.
const BUILTIN: [&str; 4] = ["Default", "Adwaita", "HighContrast", "HighContrastInverse"];

fn settings() -> Option<gtk::Settings> {
    gtk::Settings::default()
}

fn current_name() -> String {
    settings().and_then(|s| s.gtk_theme_name()).map(|s| s.to_string()).unwrap_or_default()
}

fn name_looks_dark(name: &str) -> bool {
    name.to_lowercase().contains("dark")
}

/// Pastas onde o GTK procura temas.
fn theme_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(h) = &home {
        dirs.push(h.join(".themes"));
    }
    match std::env::var_os("XDG_DATA_HOME") {
        Some(d) => dirs.push(PathBuf::from(d).join("themes")),
        None => {
            if let Some(h) = &home {
                dirs.push(h.join(".local/share/themes"));
            }
        }
    }
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    for d in data.split(':').filter(|d| !d.is_empty()) {
        dirs.push(PathBuf::from(d).join("themes"));
    }
    dirs.push(PathBuf::from("/usr/share/themes"));
    dirs
}

fn theme_file(name: &str, file: &str) -> bool {
    !name.is_empty() && theme_dirs().iter().any(|d| d.join(name).join("gtk-4.0").join(file).is_file())
}

/// O tema existe e tem versão para GTK4?
fn has_gtk4(name: &str) -> bool {
    BUILTIN.contains(&name) || theme_file(name, "gtk.css")
}

/// Variante escura de um tema (ex.: Mint-Y-Aqua → Mint-Y-Dark-Aqua).
fn dark_variant(name: &str) -> Option<String> {
    let mut candidates = Vec::new();
    // Temas do Mint: "Mint-Y[-Cor]" → "Mint-Y-Dark[-Cor]"
    let parts: Vec<&str> = name.splitn(3, '-').collect();
    if parts.len() >= 2 && parts[0] == "Mint" {
        let base = format!("{}-{}-Dark", parts[0], parts[1]);
        candidates.push(match parts.get(2) {
            Some(rest) => format!("{base}-{rest}"),
            None => base,
        });
    }
    candidates.push(format!("{name}-Dark"));
    candidates.push(format!("{name}-dark"));
    candidates.into_iter().find(|c| has_gtk4(c))
}

/// Variante clara de um tema escuro (ex.: Mint-Y-Dark-Aqua → Mint-Y-Aqua).
fn light_variant(name: &str) -> Option<String> {
    ["-Dark", "-dark", "Dark", "dark"]
        .iter()
        .find(|tag| name.contains(*tag))
        .map(|tag| name.replacen(tag, "", 1).trim_matches('-').to_string())
        .filter(|c| has_gtk4(c))
}

/// O tema em uso está escuro?
pub fn is_dark() -> bool {
    let Some(s) = settings() else { return false };
    s.is_gtk_application_prefer_dark_theme() || name_looks_dark(&current_name())
}

fn set_prefer_dark(dark: bool) {
    if let Some(s) = settings() {
        if s.is_gtk_application_prefer_dark_theme() != dark {
            s.set_gtk_application_prefer_dark_theme(dark);
        }
    }
}

fn set_theme(name: &str) {
    if let Some(s) = settings() {
        s.set_gtk_theme_name(Some(name));
    }
}

/// Aplica o modo claro/escuro da forma mais nativa possível.
fn apply(dark: bool) {
    if is_dark() == dark {
        return;
    }
    if !FALLBACK.with(Cell::get) {
        let cur = current_name();
        let variant = if dark { dark_variant(&cur) } else { light_variant(&cur) };
        if let Some(v) = variant {
            set_prefer_dark(false);
            set_theme(&v);
            return;
        }
        // Sem variante: usa o gtk-dark.css do próprio tema, se existir.
        if !dark && !name_looks_dark(&cur) {
            return set_prefer_dark(false);
        }
        if dark && theme_file(&cur, "gtk-dark.css") {
            return set_prefer_dark(true);
        }
        // Último recurso: tema embutido do GTK no modo pedido.
        FALLBACK.with(|f| f.set(true));
        set_theme("Default");
    }
    set_prefer_dark(dark);
}

/// Alternância manual (botão da barra de ferramentas).
pub fn toggle() {
    USER_OVERRIDE.with(|o| o.set(true));
    apply(!is_dark());
}

/// Chamado uma vez na inicialização do aplicativo.
pub fn init() {
    let Some(settings) = settings() else { return };
    let system = current_name();
    let env_theme = std::env::var("GTK_THEME").unwrap_or_default();

    if !env_theme.is_empty() || has_gtk4(&system) {
        // Tema do sistema com suporte a GTK4: usa-o como está. O GTK acompanha
        // sozinho as mudanças feitas nas configurações do sistema.
        return;
    }

    // Tema do sistema sem versão GTK4: usa o tema embutido, imitando claro/escuro.
    FALLBACK.with(|f| f.set(true));
    let mut dark = name_looks_dark(&system);
    settings.set_gtk_theme_name(Some("Default"));
    if let Ok(conn) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        match portal_color_scheme(&conn) {
            Some(1) => dark = true,
            Some(2) => dark = false,
            _ => {}
        }
        watch_portal(&conn);
    }
    set_prefer_dark(dark);
}

// ---------- Portal XDG (só no modo de tema embutido) ----------

fn find_u32(v: &Variant) -> Option<u32> {
    if let Some(u) = v.get::<u32>() {
        return Some(u);
    }
    if let Some(inner) = v.as_variant() {
        return find_u32(&inner);
    }
    if v.is_container() && v.n_children() > 0 {
        return find_u32(&v.child_value(0));
    }
    None
}

/// 0 = sem preferência, 1 = escuro, 2 = claro.
fn portal_color_scheme(conn: &gio::DBusConnection) -> Option<u32> {
    let call = |method: &str| {
        conn.call_sync(
            Some(PORTAL_BUS),
            PORTAL_PATH,
            PORTAL_IFACE,
            method,
            Some(&("org.freedesktop.appearance", "color-scheme").to_variant()),
            None,
            gio::DBusCallFlags::NO_AUTO_START,
            250,
            gio::Cancellable::NONE,
        )
    };
    let reply = call("ReadOne").or_else(|_| call("Read")).ok()?;
    find_u32(&reply)
}

#[allow(deprecated)]
fn watch_portal(conn: &gio::DBusConnection) {
    let _id = conn.signal_subscribe(
        Some(PORTAL_BUS),
        Some(PORTAL_IFACE),
        Some("SettingChanged"),
        Some(PORTAL_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |_conn, _sender, _path, _iface, _signal, params| {
            if params.n_children() < 3 || USER_OVERRIDE.with(Cell::get) {
                return;
            }
            let ns = params.child_value(0).get::<String>().unwrap_or_default();
            let key = params.child_value(1).get::<String>().unwrap_or_default();
            if ns != "org.freedesktop.appearance" || key != "color-scheme" {
                return;
            }
            match find_u32(&params.child_value(2)) {
                Some(1) => set_prefer_dark(true),
                Some(2) | Some(0) => set_prefer_dark(false),
                _ => {}
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variantes_do_mint() {
        // Usa os temas do Mint instalados (se presentes) para validar os nomes.
        if has_gtk4("Mint-Y-Aqua") && has_gtk4("Mint-Y-Dark-Aqua") {
            assert_eq!(dark_variant("Mint-Y-Aqua").as_deref(), Some("Mint-Y-Dark-Aqua"));
            assert_eq!(light_variant("Mint-Y-Dark-Aqua").as_deref(), Some("Mint-Y-Aqua"));
        }
        if has_gtk4("Mint-Y") && has_gtk4("Mint-Y-Dark") {
            assert_eq!(dark_variant("Mint-Y").as_deref(), Some("Mint-Y-Dark"));
            assert_eq!(light_variant("Mint-Y-Dark").as_deref(), Some("Mint-Y"));
        }
        assert!(has_gtk4("Default"));
        assert!(!has_gtk4("Tema-Que-Nao-Existe"));
    }
}
