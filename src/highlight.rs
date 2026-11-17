//! Destaque de sintaxe leve para blocos de código.
//!
//! Um analisador léxico genérico (comentários, strings, números, palavras-chave,
//! tipos e chamadas de função) configurado por linguagem. Não usa gramáticas
//! completas: custa praticamente zero de memória e de tempo de inicialização
//! (uma alternativa baseada em gramáticas do Sublime custava ~40 MB de RAM).

use gtk::glib;
use std::fmt::Write;

/// Blocos maiores que isso são exibidos sem cores.
const MAX_HIGHLIGHT_BYTES: usize = 300_000;

#[derive(Clone, Copy, PartialEq)]
enum Tok {
    Comment,
    Str,
    Num,
    Keyword,
    Type,
    Func,
    Meta,
    Tag,
    Attr,
    Added,
    Removed,
}

fn color(t: Tok, dark: bool) -> (&'static str, &'static str) {
    // (cor, estilo extra)
    match (t, dark) {
        (Tok::Comment, false) => ("#6a737d", " style=\"italic\""),
        (Tok::Comment, true) => ("#7f8a99", " style=\"italic\""),
        (Tok::Str, false) => ("#0a6b2e", ""),
        (Tok::Str, true) => ("#98c379", ""),
        (Tok::Num, false) => ("#b35900", ""),
        (Tok::Num, true) => ("#d19a66", ""),
        (Tok::Keyword, false) => ("#c4204a", " weight=\"bold\""),
        (Tok::Keyword, true) => ("#c678dd", ""),
        (Tok::Type, false) => ("#6f42c1", ""),
        (Tok::Type, true) => ("#e5c07b", ""),
        (Tok::Func, false) => ("#005cc5", ""),
        (Tok::Func, true) => ("#61afef", ""),
        (Tok::Meta, false) => ("#8a6100", ""),
        (Tok::Meta, true) => ("#56b6c2", ""),
        (Tok::Tag, false) => ("#22863a", ""),
        (Tok::Tag, true) => ("#e06c75", ""),
        (Tok::Attr, false) => ("#6f42c1", ""),
        (Tok::Attr, true) => ("#d19a66", ""),
        (Tok::Added, false) => ("#116329", ""),
        (Tok::Added, true) => ("#7ee787", ""),
        (Tok::Removed, false) => ("#b31d28", ""),
        (Tok::Removed, true) => ("#ff7b72", ""),
    }
}

#[derive(Default)]
struct Lang {
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    strings: &'static str,
    /// Strings com aspas triplas (Python) e similares.
    triple: bool,
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    /// Identificadores iniciados por maiúscula são tipos.
    caps_types: bool,
    /// Prefixo de meta/atributo: '#' (C), '@' (Java/Python), '$' (shell)…
    meta: &'static str,
    /// Linguagem de marcação (HTML/XML).
    markup: bool,
    /// Chaves de configuração (YAML/TOML/INI): texto antes de ':' ou '='.
    keys: bool,
    diff: bool,
}

const C_KW: &[&str] = &[
    "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for", "goto", "if",
    "inline", "register", "return", "sizeof", "static", "struct", "switch", "typedef", "union", "volatile", "while",
    "class", "namespace", "template", "typename", "public", "private", "protected", "virtual", "override", "new",
    "delete", "this", "throw", "try", "catch", "using", "constexpr", "nullptr", "true", "false", "NULL", "operator",
    "friend", "noexcept", "static_cast", "dynamic_cast", "reinterpret_cast", "mutable", "explicit",
];
const C_TY: &[&str] = &[
    "void", "char", "short", "int", "long", "float", "double", "signed", "unsigned", "bool", "size_t", "uint8_t",
    "uint16_t", "uint32_t", "uint64_t", "int8_t", "int16_t", "int32_t", "int64_t", "string", "std", "vector", "FILE",
];
const RUST_KW: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
    "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self",
    "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "macro_rules",
];
const RUST_TY: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool",
    "char", "str", "String", "Vec", "Option", "Result", "Box", "Rc", "Arc", "Some", "None", "Ok", "Err",
];
const PY_KW: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except",
    "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass",
    "raise", "return", "try", "while", "with", "yield", "True", "False", "None", "self", "match", "case",
];
const PY_TY: &[&str] = &[
    "int", "float", "str", "bool", "list", "dict", "set", "tuple", "bytes", "object", "type", "print", "len",
    "range", "open", "super", "isinstance", "enumerate", "zip", "map", "filter",
];
const JS_KW: &[&str] = &[
    "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do",
    "else", "export", "extends", "finally", "for", "from", "function", "if", "import", "in", "instanceof", "let",
    "new", "of", "return", "static", "super", "switch", "this", "throw", "try", "typeof", "var", "void", "while",
    "with", "yield", "true", "false", "null", "undefined", "interface", "type", "enum", "implements", "public",
    "private", "protected", "readonly", "as", "declare", "namespace", "abstract", "keyof",
];
const JS_TY: &[&str] = &[
    "string", "number", "boolean", "any", "unknown", "never", "object", "Array", "Promise", "Map", "Set",
    "console", "window", "document", "JSON", "Math", "Object", "Error",
];
const JAVA_KW: &[&str] = &[
    "abstract", "assert", "break", "case", "catch", "class", "const", "continue", "default", "do", "else", "enum",
    "extends", "final", "finally", "for", "goto", "if", "implements", "import", "instanceof", "interface", "native",
    "new", "package", "private", "protected", "public", "return", "static", "strictfp", "super", "switch",
    "synchronized", "this", "throw", "throws", "transient", "try", "volatile", "while", "true", "false", "null",
    "var", "val", "fun", "object", "when", "is", "in", "override", "data", "sealed", "open", "internal", "using",
    "namespace", "readonly", "async", "await", "func", "let", "guard", "struct", "protocol", "extension", "self",
    "init", "where", "get", "set", "lateinit", "companion",
];
const JAVA_TY: &[&str] = &[
    "int", "long", "short", "byte", "char", "float", "double", "boolean", "void", "String", "Integer", "List",
    "Map", "Object", "string", "bool", "decimal", "Int", "Double", "Float", "Boolean", "Unit", "Any",
];
const GO_KW: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go",
    "goto", "if", "import", "interface", "map", "package", "range", "return", "select", "struct", "switch", "type",
    "var", "true", "false", "nil", "iota",
];
const GO_TY: &[&str] = &[
    "bool", "byte", "error", "float32", "float64", "int", "int8", "int16", "int32", "int64", "rune", "string",
    "uint", "uint8", "uint16", "uint32", "uint64", "uintptr", "any", "make", "len", "cap", "append", "new", "panic",
];
const SH_KW: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "while", "until", "do", "done", "in", "function",
    "return", "local", "export", "readonly", "declare", "set", "unset", "source", "exit", "break", "continue",
];
const SH_TY: &[&str] = &[
    "echo", "printf", "cd", "ls", "cp", "mv", "rm", "mkdir", "chmod", "chown", "cat", "grep", "sed", "awk", "find",
    "sudo", "apt", "apt-get", "git", "curl", "wget", "tar", "make", "cargo", "npm", "pip", "python3", "test",
];
const SQL_KW: &[&str] = &[
    "select", "from", "where", "and", "or", "not", "insert", "into", "values", "update", "set", "delete", "create",
    "table", "drop", "alter", "add", "index", "join", "left", "right", "inner", "outer", "on", "as", "group", "by",
    "order", "having", "limit", "offset", "distinct", "union", "all", "null", "is", "in", "like", "between",
    "primary", "key", "foreign", "references", "default", "unique", "case", "when", "then", "else", "end", "asc",
    "desc", "with", "exists", "view", "begin", "commit", "rollback", "true", "false",
];
const SQL_TY: &[&str] = &[
    "int", "integer", "bigint", "smallint", "text", "varchar", "char", "boolean", "date", "timestamp", "float",
    "real", "numeric", "decimal", "serial", "count", "sum", "avg", "min", "max", "coalesce",
];
const RB_KW: &[&str] = &[
    "begin", "end", "def", "class", "module", "if", "elsif", "else", "unless", "while", "until", "for", "in", "do",
    "return", "yield", "self", "nil", "true", "false", "and", "or", "not", "then", "case", "when", "require",
    "include", "attr_accessor", "puts", "local", "function", "repeat", "elseif", "break", "goto",
];
const PHP_KW: &[&str] = &[
    "echo", "function", "return", "if", "else", "elseif", "foreach", "as", "while", "for", "class", "public",
    "private", "protected", "static", "new", "use", "namespace", "true", "false", "null", "array", "require",
    "include", "extends", "implements", "try", "catch", "throw",
];
const CSS_KW: &[&str] = &["important", "media", "import", "keyframes", "from", "to", "root", "hover", "focus", "before", "after"];
const LISP_KW: &[&str] = &["defn", "def", "let", "fn", "if", "do", "cond", "when", "lambda", "define", "ns", "require"];
const DOCKER_KW: &[&str] = &[
    "FROM", "RUN", "CMD", "LABEL", "EXPOSE", "ENV", "ADD", "COPY", "ENTRYPOINT", "VOLUME", "USER", "WORKDIR", "ARG",
    "ONBUILD", "STOPSIGNAL", "HEALTHCHECK", "SHELL", "AS",
];
const LIT_KW: &[&str] = &["true", "false", "null", "yes", "no", "on", "off", "True", "False", "None", "~"];

fn lang_for(name: &str) -> Option<Lang> {
    let n = name.trim().to_lowercase();
    let l = match n.as_str() {
        "rust" | "rs" => Lang {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            strings: "\"",
            keywords: RUST_KW,
            types: RUST_TY,
            caps_types: true,
            meta: "#",
            ..Default::default()
        },
        "c" | "h" | "cpp" | "c++" | "cc" | "cxx" | "hpp" | "objc" | "arduino" | "ino" | "cuda" | "glsl" => Lang {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            strings: "\"'",
            keywords: C_KW,
            types: C_TY,
            meta: "#",
            ..Default::default()
        },
        "python" | "py" | "py3" | "python3" | "pyw" | "gdscript" => Lang {
            line_comments: &["#"],
            strings: "\"'",
            triple: true,
            keywords: PY_KW,
            types: PY_TY,
            caps_types: true,
            meta: "@",
            ..Default::default()
        },
        "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" | "mjs" | "cjs" | "node" | "dart" => Lang {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            strings: "\"'`",
            keywords: JS_KW,
            types: JS_TY,
            caps_types: true,
            meta: "@",
            ..Default::default()
        },
        "java" | "kotlin" | "kt" | "kts" | "scala" | "cs" | "c#" | "csharp" | "swift" | "groovy" | "gradle" => Lang {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            strings: "\"'",
            keywords: JAVA_KW,
            types: JAVA_TY,
            caps_types: true,
            meta: "@",
            ..Default::default()
        },
        "go" | "golang" => Lang {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            strings: "\"'`",
            keywords: GO_KW,
            types: GO_TY,
            ..Default::default()
        },
        "sh" | "bash" | "shell" | "zsh" | "fish" | "console" | "terminal" | "shellscript" | "ksh" | "make"
        | "makefile" | "cmake" | "nginx" | "conf" | "apache" | "dotenv" | "env" => Lang {
            line_comments: &["#"],
            strings: "\"'",
            keywords: SH_KW,
            types: SH_TY,
            meta: "$",
            ..Default::default()
        },
        "powershell" | "ps1" | "pwsh" => Lang {
            line_comments: &["#"],
            block_comment: Some(("<#", "#>")),
            strings: "\"'",
            keywords: SH_KW,
            meta: "$",
            ..Default::default()
        },
        "sql" | "mysql" | "postgresql" | "postgres" | "sqlite" | "plsql" => Lang {
            line_comments: &["--"],
            block_comment: Some(("/*", "*/")),
            strings: "'\"",
            keywords: SQL_KW,
            types: SQL_TY,
            ..Default::default()
        },
        "json" | "jsonc" | "json5" | "geojson" => Lang {
            line_comments: &["//"],
            strings: "\"",
            keywords: LIT_KW,
            keys: true,
            ..Default::default()
        },
        "yaml" | "yml" | "toml" | "ini" | "cfg" | "properties" | "desktop" | "editorconfig" => Lang {
            line_comments: &["#", ";"],
            strings: "\"'",
            keywords: LIT_KW,
            keys: true,
            ..Default::default()
        },
        "ruby" | "rb" | "lua" | "elixir" | "ex" | "crystal" => Lang {
            line_comments: &["#", "--"],
            strings: "\"'",
            keywords: RB_KW,
            caps_types: true,
            meta: "@",
            ..Default::default()
        },
        "php" => Lang {
            line_comments: &["//", "#"],
            block_comment: Some(("/*", "*/")),
            strings: "\"'",
            keywords: PHP_KW,
            meta: "$",
            ..Default::default()
        },
        "css" | "scss" | "sass" | "less" => Lang {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            strings: "\"'",
            keywords: CSS_KW,
            meta: "@",
            keys: true,
            ..Default::default()
        },
        "html" | "htm" | "xml" | "svg" | "xhtml" | "vue" | "svelte" | "xaml" | "plist" | "ui" => Lang {
            block_comment: Some(("<!--", "-->")),
            strings: "\"'",
            markup: true,
            ..Default::default()
        },
        "clojure" | "clj" | "lisp" | "scheme" | "elisp" | "racket" => Lang {
            line_comments: &[";"],
            strings: "\"",
            keywords: LISP_KW,
            ..Default::default()
        },
        "dockerfile" | "docker" | "containerfile" => Lang {
            line_comments: &["#"],
            strings: "\"'",
            keywords: DOCKER_KW,
            meta: "$",
            ..Default::default()
        },
        "haskell" | "hs" | "elm" => Lang {
            line_comments: &["--"],
            block_comment: Some(("{-", "-}")),
            strings: "\"",
            keywords: &["module", "where", "import", "data", "type", "class", "instance", "let", "in", "case", "of", "if", "then", "else", "do", "deriving"],
            caps_types: true,
            ..Default::default()
        },
        "diff" | "patch" => Lang { diff: true, ..Default::default() },
        _ => return None,
    };
    Some(l)
}

struct Out<'a> {
    s: String,
    dark: bool,
    _p: std::marker::PhantomData<&'a ()>,
}

impl Out<'_> {
    fn plain(&mut self, t: &str) {
        self.s.push_str(&glib::markup_escape_text(t));
    }
    fn tok(&mut self, kind: Tok, t: &str) {
        let (c, extra) = color(kind, self.dark);
        let _ = write!(self.s, "<span foreground=\"{c}\"{extra}>{}</span>", glib::markup_escape_text(t));
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn highlight_diff(code: &str, out: &mut Out) {
    for line in code.split_inclusive('\n') {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
            out.tok(Tok::Meta, line);
        } else if line.starts_with('+') {
            out.tok(Tok::Added, line);
        } else if line.starts_with('-') {
            out.tok(Tok::Removed, line);
        } else {
            out.plain(line);
        }
    }
}

fn highlight_with(code: &str, lang: &Lang, out: &mut Out) {
    let chars: Vec<(usize, char)> = code.char_indices().collect();
    let n = chars.len();
    let at = |i: usize| -> usize { if i < n { chars[i].0 } else { code.len() } };
    let starts = |i: usize, s: &str| code[at(i)..].starts_with(s);
    let mut i = 0;
    let mut line_start = true;
    let mut in_tag = false;

    while i < n {
        let c = chars[i].1;

        // Comentários
        if let Some((open, close)) = lang.block_comment {
            if starts(i, open) {
                let from = at(i);
                let end = code[from + open.len()..].find(close).map(|e| from + open.len() + e + close.len()).unwrap_or(code.len());
                out.tok(Tok::Comment, &code[from..end]);
                while i < n && at(i) < end {
                    i += 1;
                }
                continue;
            }
        }
        if !in_tag && lang.line_comments.iter().any(|lc| starts(i, lc)) {
            // '#' em shell só é comentário no início ou após espaço
            let ok = c != '#' || i == 0 || chars[i - 1].1.is_whitespace() || !lang.meta.contains('$');
            // '#' em C é diretiva, não comentário
            if ok && !(lang.meta == "#" && c == '#') {
                let from = at(i);
                let end = code[from..].find('\n').map(|e| from + e).unwrap_or(code.len());
                out.tok(Tok::Comment, &code[from..end]);
                while i < n && at(i) < end {
                    i += 1;
                }
                continue;
            }
        }

        // Marcação (HTML/XML)
        if lang.markup {
            if c == '<' {
                let from = at(i);
                let mut j = i + 1;
                if j < n && (chars[j].1 == '/' || chars[j].1 == '!' || chars[j].1 == '?') {
                    j += 1;
                }
                while j < n && (is_ident(chars[j].1) || chars[j].1 == '-' || chars[j].1 == ':') {
                    j += 1;
                }
                out.tok(Tok::Tag, &code[from..at(j)]);
                in_tag = true;
                i = j;
                continue;
            }
            if in_tag && (c == '>' || starts(i, "/>")) {
                let len = if c == '>' { 1 } else { 2 };
                out.tok(Tok::Tag, &code[at(i)..at(i + len)]);
                in_tag = false;
                i += len;
                continue;
            }
            if in_tag && is_ident_start(c) {
                let from = at(i);
                let mut j = i;
                while j < n && (is_ident(chars[j].1) || chars[j].1 == '-' || chars[j].1 == ':') {
                    j += 1;
                }
                out.tok(Tok::Attr, &code[from..at(j)]);
                i = j;
                continue;
            }
            if !in_tag && c == '&' {
                let from = at(i);
                if let Some(e) = code[from..].find(';').filter(|e| *e < 10) {
                    out.tok(Tok::Meta, &code[from..from + e + 1]);
                    while i < n && at(i) < from + e + 1 {
                        i += 1;
                    }
                    continue;
                }
            }
            if !in_tag {
                let from = at(i);
                let end = code[from..].find(['<', '&']).map(|e| from + e.max(1)).unwrap_or(code.len());
                out.plain(&code[from..end]);
                while i < n && at(i) < end {
                    i += 1;
                }
                continue;
            }
        }

        // Strings
        if lang.strings.contains(c) {
            let from = at(i);
            let triple: String = std::iter::repeat_n(c, 3).collect();
            let (delim, mut j) = if lang.triple && starts(i, &triple) { (triple.as_str(), i + 3) } else { ("", i + 1) };
            loop {
                if j >= n {
                    break;
                }
                let d = chars[j].1;
                if d == '\\' {
                    j += 2;
                    continue;
                }
                if delim.is_empty() {
                    if d == c {
                        j += 1;
                        break;
                    }
                    if d == '\n' && c != '`' {
                        break;
                    }
                } else if starts(j, delim) {
                    j += 3;
                    break;
                }
                j += 1;
            }
            let j = j.min(n);
            let s = &code[from..at(j)];
            // Em JSON/YAML, string seguida de ':' é chave.
            let rest = code[at(j)..].trim_start_matches([' ', '\t']);
            let kind = if lang.keys && rest.starts_with(':') { Tok::Attr } else { Tok::Str };
            out.tok(kind, s);
            i = j;
            line_start = false;
            continue;
        }

        // Números
        if c.is_ascii_digit() && (i == 0 || !is_ident(chars[i - 1].1)) {
            let from = at(i);
            let mut j = i;
            while j < n && (chars[j].1.is_ascii_alphanumeric() || chars[j].1 == '.' || chars[j].1 == '_') {
                if chars[j].1 == '.' && !(j + 1 < n && chars[j + 1].1.is_ascii_digit()) {
                    break;
                }
                j += 1;
            }
            out.tok(Tok::Num, &code[from..at(j)]);
            i = j;
            line_start = false;
            continue;
        }

        // Meta: atributos (#[...]), diretivas (#include), decoradores (@x), variáveis ($x)
        if !lang.meta.is_empty() && lang.meta.contains(c) {
            let from = at(i);
            let mut j = i + 1;
            if c == '#' && j < n && (chars[j].1 == '[' || chars[j].1 == '!') {
                // atributo Rust até ']'
                let end = code[from..].find(']').map(|e| from + e + 1).unwrap_or(code.len());
                let end = end.min(code[from..].find('\n').map(|e| from + e).unwrap_or(code.len()));
                out.tok(Tok::Meta, &code[from..end]);
                while i < n && at(i) < end {
                    i += 1;
                }
                continue;
            }
            if c == '$' && j < n && chars[j].1 == '{' {
                let end = code[from..].find('}').map(|e| from + e + 1).unwrap_or(code.len());
                out.tok(Tok::Meta, &code[from..end]);
                while i < n && at(i) < end {
                    i += 1;
                }
                continue;
            }
            while j < n && (is_ident(chars[j].1) || chars[j].1 == '.') {
                j += 1;
            }
            if j > i + 1 || c == '$' {
                out.tok(Tok::Meta, &code[from..at(j)]);
                i = j;
                continue;
            }
        }

        // Identificadores
        if is_ident_start(c) {
            let from = at(i);
            let mut j = i;
            while j < n && (is_ident(chars[j].1) || (lang.keys && chars[j].1 == '-')) {
                j += 1;
            }
            let word = &code[from..at(j)];
            let rest = code[at(j)..].trim_start_matches([' ', '\t']);
            let lower = word.to_lowercase();
            let kw_match = |list: &[&str]| {
                if std::ptr::eq(list, SQL_KW) || std::ptr::eq(list, SQL_TY) {
                    list.contains(&lower.as_str())
                } else {
                    list.contains(&word)
                }
            };
            let kind = if lang.keys && line_start && (rest.starts_with(':') || rest.starts_with('=')) {
                Some(Tok::Attr)
            } else if kw_match(lang.keywords) {
                Some(Tok::Keyword)
            } else if kw_match(lang.types) {
                Some(Tok::Type)
            } else if rest.starts_with('(') || rest.starts_with('!') && lang.meta == "#" {
                Some(Tok::Func)
            } else if lang.caps_types && word.chars().next().is_some_and(char::is_uppercase) {
                Some(Tok::Type)
            } else {
                None
            };
            match kind {
                Some(k) => out.tok(k, word),
                None => out.plain(word),
            }
            i = j;
            line_start = false;
            continue;
        }

        // Qualquer outro caractere
        let from = at(i);
        out.plain(&code[from..at(i + 1)]);
        if c == '\n' {
            line_start = true;
        } else if !c.is_whitespace() && c != '-' {
            line_start = false;
        }
        i += 1;
    }
}

/// Retorna markup Pango com cores, ou `None` se a linguagem for desconhecida.
pub fn highlight(code: &str, lang: &str, dark: bool) -> Option<String> {
    if code.len() > MAX_HIGHLIGHT_BYTES {
        return None;
    }
    let l = lang_for(lang)?;
    let mut out = Out { s: String::with_capacity(code.len() * 3), dark, _p: Default::default() };
    if l.diff {
        highlight_diff(code, &mut out);
    } else {
        highlight_with(code, &l, &mut out);
    }
    Some(out.s)
}

/// Markup final para um bloco de código (com ou sem cores).
pub fn code_markup(code: &str, lang: &str, dark: bool) -> String {
    highlight(code, lang, dark).unwrap_or_else(|| glib::markup_escape_text(code).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(m: &str) -> bool {
        gtk::pango::parse_markup(m, '\0').is_ok()
    }

    #[test]
    fn produz_markup_valido() {
        let samples = [
            ("rust", "#[derive(Debug)]\nfn main() { let s = \"olá\\\"\"; // c\n println!(\"{}\", 1.5e3); }"),
            ("python", "@dec\ndef f(x):\n    \"\"\"doc <b>\"\"\"\n    return x # c"),
            ("bash", "#!/bin/bash\necho \"$HOME\" ${VAR} # comentário\nls -la | grep x"),
            ("json", "{\"a\": [1, 2.5, true, null], \"b\": \"x<y&z\"}"),
            ("html", "<!-- c --><div class=\"a\">Olá &amp; <b>mundo</b></div>"),
            ("yaml", "chave: valor # c\nlista:\n  - item: 'x'"),
            ("sql", "SELECT * FROM t WHERE id = 3 -- c"),
            ("diff", "--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y\n"),
            ("c", "#include <stdio.h>\nint main(void) { /* c */ return 0; }"),
            ("ts", "const x: string = `a ${b}`; // c"),
        ];
        for (l, code) in samples {
            let m = highlight(code, l, false).expect(l);
            assert!(valid(&m), "markup inválido para {l}: {m}");
            // O texto visível deve ser idêntico ao original.
            let (_, text, _) = gtk::pango::parse_markup(&m, '\0').unwrap();
            assert_eq!(text.as_str(), code, "texto alterado para {l}");
        }
    }

    #[test]
    fn linguagem_desconhecida() {
        assert!(highlight("x", "brainfuck", false).is_none());
        assert!(highlight("x", "", false).is_none());
    }
}
