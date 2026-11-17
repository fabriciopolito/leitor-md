#!/usr/bin/env bash
# Gera o AppImage do Leitor Markdown.
#
# Requisitos de compilação (Ubuntu 24.04 / Linux Mint 22):
#   sudo apt install build-essential pkg-config libgtk-4-dev curl file
#   Rust (rustup) >= 1.80
#
# Uso: ./scripts/build-appimage.sh
# Resultado: dist/LeitorMarkdown-<versão>-x86_64.AppImage

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_ID="io.github.leitormd.LeitorMarkdown"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
ARCH="x86_64"
BUILD="$ROOT/build"
TOOLS="$BUILD/tools"
APPDIR="$BUILD/AppDir"
OUT="$ROOT/dist/LeitorMarkdown-$VERSION-$ARCH.AppImage"

# Ferramentas AppImage rodam sem FUSE (útil em contêineres/CI).
export APPIMAGE_EXTRACT_AND_RUN=1

mkdir -p "$TOOLS" "$ROOT/dist"

fetch() { # url destino
    [ -s "$2" ] || { echo ">> baixando $(basename "$2")"; curl -fsSL "$1" -o "$2"; }
    chmod +x "$2"
}
fetch https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage \
      "$TOOLS/linuxdeploy-x86_64.AppImage"
fetch https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh \
      "$TOOLS/linuxdeploy-plugin-gtk.sh"
fetch https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage \
      "$TOOLS/appimagetool-x86_64.AppImage"

echo ">> compilando (release)"
cargo build --release

echo ">> montando AppDir"
rm -rf "$APPDIR"
install -Dm755 target/release/leitor-md "$APPDIR/usr/bin/leitor-md"
install -Dm644 "data/$APP_ID.desktop" "$APPDIR/usr/share/applications/$APP_ID.desktop"
install -Dm644 "data/$APP_ID.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
install -Dm644 "data/$APP_ID.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/$APP_ID.png"

echo ">> copiando bibliotecas (linuxdeploy + plugin GTK4)"
export DEPLOY_GTK_VERSION=4
export PATH="$TOOLS:$PATH"
"$TOOLS/linuxdeploy-x86_64.AppImage" \
    --appdir "$APPDIR" \
    --executable "$APPDIR/usr/bin/leitor-md" \
    --desktop-file "$APPDIR/usr/share/applications/$APP_ID.desktop" \
    --icon-file "data/$APP_ID.svg" \
    --plugin gtk

echo ">> enxugando AppDir"
# Recursos do GTK que um leitor não usa: mídia (GStreamer) e impressão.
rm -rf "$APPDIR/usr/lib/gtk-4.0" "$APPDIR/apprun-hooks"
rm -f "$APPDIR"/usr/lib/libgst*.so* "$APPDIR"/usr/lib/libcups*.so*
rm -rf "$APPDIR/usr/lib/girepository-1.0" "$APPDIR/usr/share/doc"

# Mantém apenas as bibliotecas realmente necessárias: o fecho transitivo das
# dependências (DT_NEEDED) do executável e dos carregadores de imagem.
prune_libs() {
    local lib="$APPDIR/usr/lib"
    declare -A keep=()
    local queue=("$APPDIR/usr/bin/leitor-md")
    # Carregadores do gdk-pixbuf: movidos para usr/lib, onde o RUNPATH ($ORIGIN)
    # encontra suas dependências (librsvg etc.). O AppRun aponta
    # GDK_PIXBUF_MODULEDIR para usr/lib.
    local ld="$lib/gdk-pixbuf-2.0/2.10.0/loaders"
    if [ -d "$ld" ]; then
        for f in "$ld"/*.so; do rm -f "$lib/$(basename "$f")"; mv "$f" "$lib/"; done
        rm -rf "$ld"
    fi
    for f in "$lib"/libpixbufloader-*.so; do
        [ -e "$f" ] && queue+=("$f") && keep["$(basename "$f")"]=1
    done
    while [ ${#queue[@]} -gt 0 ]; do
        local f="${queue[0]}"; queue=("${queue[@]:1}")
        for n in $(readelf -d "$f" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'); do
            if [ -z "${keep[$n]:-}" ] && [ -e "$lib/$n" ]; then
                keep["$n"]=1
                queue+=("$lib/$n")
            fi
        done
    done
    local removed=0
    for f in "$lib"/*.so*; do
        if [ -z "${keep[$(basename "$f")]:-}" ]; then rm -f "$f"; removed=$((removed + 1)); fi
    done
    echo "   bibliotecas mantidas: ${#keep[@]}, removidas: $removed"
}
prune_libs

# Banco de dados MIME (shared-mime-info) compilado: o gdk-pixbuf o usa para
# reconhecer SVG/WebP etc. Quase toda distribuição já o tem; embarcá-lo custa
# pouco e evita falhas em sistemas mínimos.
mkdir -p "$APPDIR/usr/share/mime"
for f in mime.cache globs globs2 magic aliases subclasses types icons generic-icons \
         treemagic XMLnamespaces version; do
    [ -f "/usr/share/mime/$f" ] && cp "/usr/share/mime/$f" "$APPDIR/usr/share/mime/"
done

# Os esquemas devem existir compilados para o seletor de arquivos.
glib-compile-schemas "$APPDIR/usr/share/glib-2.0/schemas" 2>/dev/null || true

# Ícone na raiz do AppDir (usado pelo AppImage e por integradores de desktop).
rm -f "$APPDIR/$APP_ID.svg" "$APPDIR/.DirIcon"
cp "data/$APP_ID.png" "$APPDIR/$APP_ID.png"
cp "data/$APP_ID.png" "$APPDIR/.DirIcon"

# AppRun próprio (ambiente mínimo e restaurável).
rm -f "$APPDIR/AppRun"
install -Dm755 scripts/AppRun "$APPDIR/AppRun"

# Remove a dependência de bibliotecas descartadas acima, se houver.
missing="$(LD_LIBRARY_PATH="$APPDIR/usr/lib" ldd "$APPDIR/usr/bin/leitor-md" | grep 'not found' || true)"
if [ -n "$missing" ]; then
    echo "!! bibliotecas ausentes:"; echo "$missing"; exit 1
fi

echo ">> gerando AppImage"
rm -f "$OUT"
ARCH="$ARCH" VERSION="$VERSION" "$TOOLS/appimagetool-x86_64.AppImage" --no-appstream "$APPDIR" "$OUT"
chmod +x "$OUT"

echo
echo "AppImage gerado: $OUT"
ls -lh "$OUT"
