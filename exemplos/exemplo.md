---
titulo: Documento de exemplo
autor: Leitor Markdown
---

# Leitor Markdown — documento de exemplo

Este arquivo demonstra **todos os elementos** suportados pelo leitor: *ênfase*, **negrito**, ***ambos***, ~~tachado~~, `código inline`, acentuação (ação, coração, pão, Ícaro, você) e símbolos — “aspas”, travessão, €, ©, ✓.

## Introdução

Um parágrafo comum com um [link para o site do GTK](https://www.gtk.org "Site oficial do GTK") e um link interno para a [seção de tabelas](#tabelas). Também há uma nota de rodapé[^1] e outra nota[^longa].

Linha com quebra forçada  
continua aqui na linha seguinte.

## Instalação

### Requisitos

1. Linux Mint 22.x ou compatível
2. Nenhuma dependência extra
3. Um arquivo `.md` para ler
   - Listas podem ser aninhadas
   - Em vários níveis
     - Inclusive três níveis

### Linux

- Baixe o arquivo `.AppImage`
- Torne-o executável:

  ```bash
  chmod +x LeitorMarkdown-x86_64.AppImage
  ./LeitorMarkdown-x86_64.AppImage documento.md
  ```

- Pronto!

#### Lista de tarefas

- [x] Renderizar Markdown
- [x] Sumário navegável
- [ ] Ser um editor (não, de propósito)

## Configuração

> Citações aparecem com uma barra lateral discreta.
>
> Podem ter vários parágrafos e **formatação**.

> [!NOTE]
> Alertas no estilo do GitHub também são suportados.

> [!WARNING]
> Este é um alerta de atenção.

## Código

```rust
use std::collections::HashMap;

/// Conta palavras em um texto.
fn contar(texto: &str) -> HashMap<&str, usize> {
    let mut mapa = HashMap::new();
    for palavra in texto.split_whitespace() {
        *mapa.entry(palavra).or_insert(0) += 1;
    }
    mapa // retorna o mapa
}
```

```python
def saudacao(nome: str) -> str:
    """Retorna uma saudação."""
    return f"Olá, {nome}!"

print(saudacao("mundo"))
```

```json
{ "nome": "leitor-md", "versao": "1.0.0", "leve": true }
```

    Bloco de código indentado (sem linguagem)
    com duas linhas.

Uma linha de código muito longa para testar a rolagem horizontal do bloco:

```text
00000000001111111111222222222233333333334444444444555555555566666666667777777777888888888899999999990000000000111111111122222222223333333333
```

## Tabelas

| Recurso          | Suporte | Observação                                      |
|:-----------------|:-------:|------------------------------------------------:|
| Títulos          |   ✓     | H1 a H6, todos no sumário                       |
| Tabelas          |   ✓     | Com alinhamento por coluna                      |
| Imagens locais   |   ✓     | PNG, JPEG, SVG, WebP*                           |
| Imagens remotas  |   —     | Exibidas como link (o leitor não acessa a rede) |
| `código` em célula | ✓     | **negrito** e *itálico* também                  |

## Imagens

Imagem local relativa ao documento:

![Logotipo do leitor](imagem.png)

Imagem SVG (via gdk-pixbuf/librsvg) ao lado de uma PNG:

![SVG](icone.svg) ![PNG](imagem.png)

Imagem remota (mostrada como link):

![Badge remoto](https://img.shields.io/badge/exemplo-badge-blue)

<p align="center">
  <img src="imagem.png" alt="Imagem via HTML" width="64">
</p>

---

## Conclusão

Texto final com <kbd>Ctrl</kbd>+<kbd>O</kbd> para abrir arquivos, H<sub>2</sub>O e x<sup>2</sup>.

Link para [outro documento](outro.md) na mesma pasta.

[^1]: Esta é a primeira nota de rodapé.
[^longa]: Uma nota de rodapé mais longa, com `código` e **ênfase**, para testar a quebra de linha dentro da nota.
