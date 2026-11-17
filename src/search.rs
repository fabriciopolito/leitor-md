//! Busca no texto do documento (Ctrl+F).
//!
//! A busca ignora maiúsculas/minúsculas e acentos ("acao" encontra "Ação").
//! As ocorrências são destacadas com atributos Pango nos próprios rótulos;
//! os atributos originais de cada rótulo são guardados e restaurados ao limpar.

use gtk::pango;
use gtk::prelude::*;

/// Uma ocorrência: rótulo e intervalo em bytes do texto do rótulo.
pub struct Match {
    pub label: gtk::Label,
    pub start: usize,
    pub end: usize,
}

pub struct Search {
    /// Rótulos alterados, com os atributos que tinham antes da busca.
    touched: Vec<(gtk::Label, Option<pango::AttrList>)>,
    pub matches: Vec<Match>,
    pub current: usize,
}

/// Rótulos que não fazem parte do texto (marcadores de lista etc.).
const SKIP_CLASSES: [&str; 4] = ["list-marker", "footnote-marker", "code-lang", "alert-title"];

/// Normaliza um caractere: minúsculo e sem acento (um caractere vira um caractere,
/// para que as posições continuem correspondendo às do texto original).
pub fn fold(c: char) -> char {
    let l = c.to_lowercase().next().unwrap_or(c);
    match l {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

/// Todas as ocorrências (sem sobreposição) de `needle` (já normalizado) em `text`,
/// como intervalos em bytes de `text`.
pub fn find_all(text: &str, needle: &[char]) -> Vec<(usize, usize)> {
    let n = needle.len();
    if n == 0 {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().map(|(i, c)| (i, fold(c))).collect();
    let mut out = Vec::new();
    let mut k = 0;
    while k + n <= chars.len() {
        if chars[k..k + n].iter().map(|(_, c)| *c).eq(needle.iter().copied()) {
            let end = chars.get(k + n).map(|(i, _)| *i).unwrap_or(text.len());
            out.push((chars[k].0, end));
            k += n;
        } else {
            k += 1;
        }
    }
    out
}

fn collect_labels(w: &gtk::Widget, out: &mut Vec<gtk::Label>) {
    if let Some(l) = w.downcast_ref::<gtk::Label>() {
        if !SKIP_CLASSES.iter().any(|c| l.has_css_class(c)) {
            out.push(l.clone());
        }
        return;
    }
    let mut c = w.first_child();
    while let Some(ch) = c {
        collect_labels(&ch, out);
        c = ch.next_sibling();
    }
}

fn background(list: &pango::AttrList, start: usize, end: usize, current: bool) {
    let (r, g, b, a) = if current {
        (0xffff, 0x9800, 0x0000, 0xd000) // laranja: ocorrência atual
    } else {
        (0xffff, 0xd500, 0x4f00, 0x8000) // amarelo: demais ocorrências
    };
    let mut c = pango::AttrColor::new_background(r, g, b);
    c.set_start_index(start as u32);
    c.set_end_index(end as u32);
    list.insert(c);
    let mut al = pango::AttrInt::new_background_alpha(a);
    al.set_start_index(start as u32);
    al.set_end_index(end as u32);
    list.insert(al);
}

impl Search {
    /// Procura `query` em todos os rótulos dentro de `root` e destaca o resultado.
    pub fn run(root: &gtk::Widget, query: &str) -> Search {
        let needle: Vec<char> = query.chars().map(fold).collect();
        let mut labels = Vec::new();
        collect_labels(root, &mut labels);
        let mut s = Search { touched: Vec::new(), matches: Vec::new(), current: 0 };
        for l in labels {
            let text = l.text();
            let found = find_all(&text, &needle);
            if found.is_empty() {
                continue;
            }
            s.touched.push((l.clone(), l.attributes()));
            for (start, end) in found {
                s.matches.push(Match { label: l.clone(), start, end });
            }
        }
        for i in 0..s.touched.len() {
            s.paint(i);
        }
        s
    }

    /// Redesenha os destaques de um rótulo (índice em `touched`).
    fn paint(&self, i: usize) {
        let (label, orig) = &self.touched[i];
        let list = orig.as_ref().map(|a| a.copy().unwrap_or_default()).unwrap_or_default();
        for (k, m) in self.matches.iter().enumerate() {
            if &m.label == label {
                background(&list, m.start, m.end, k == self.current);
            }
        }
        label.set_attributes(Some(&list));
    }

    fn repaint_label(&self, label: &gtk::Label) {
        if let Some(i) = self.touched.iter().position(|(l, _)| l == label) {
            self.paint(i);
        }
    }

    /// Avança (+1) ou volta (−1) para outra ocorrência, com volta ao início/fim.
    pub fn step(&mut self, dir: i32) {
        if self.matches.is_empty() {
            return;
        }
        let old = self.matches[self.current].label.clone();
        let n = self.matches.len() as i32;
        self.current = ((self.current as i32 + dir).rem_euclid(n)) as usize;
        let new = self.matches[self.current].label.clone();
        self.repaint_label(&old);
        if new != old {
            self.repaint_label(&new);
        }
    }

    /// Faz da primeira ocorrência abaixo de `y` (posição no documento) a atual.
    pub fn select_from(&mut self, y_of: impl Fn(&Match) -> Option<f64>, y: f64) {
        if self.matches.is_empty() {
            return;
        }
        let idx = self
            .matches
            .iter()
            .position(|m| y_of(m).is_some_and(|my| my >= y))
            .unwrap_or(0);
        if idx != self.current {
            let old = self.matches[self.current].label.clone();
            self.current = idx;
            self.repaint_label(&old);
            let new = self.matches[idx].label.clone();
            self.repaint_label(&new);
        }
    }

    /// Remove todos os destaques, restaurando os atributos originais.
    pub fn clear(&self) {
        for (label, orig) in &self.touched {
            label.set_attributes(orig.as_ref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn needle(s: &str) -> Vec<char> {
        s.chars().map(fold).collect()
    }

    #[test]
    fn ignora_acentos_e_caixa() {
        let t = "Ação e AÇÃO; coração.";
        let r = find_all(t, &needle("acao"));
        assert_eq!(r.len(), 3);
        assert_eq!(&t[r[0].0..r[0].1], "Ação");
        assert_eq!(&t[r[1].0..r[1].1], "AÇÃO");
        assert_eq!(&t[r[2].0..r[2].1], "ação");
    }

    #[test]
    fn sem_sobreposicao_e_vazio() {
        assert_eq!(find_all("aaaa", &needle("aa")).len(), 2);
        assert!(find_all("texto", &needle("")).is_empty());
        assert!(find_all("", &needle("x")).is_empty());
    }
}
