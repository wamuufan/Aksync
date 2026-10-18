#!/usr/bin/env bash
set -e

LIB_DIR="/home/user-0/Şablonlar/AkSync/.pkgconfig/lib"
mkdir -p "$LIB_DIR"

link_lib() {
    local name="$1"
    local pattern="$2"
    for dir in /usr/lib64 /lib64 /usr/lib /lib; do
        for file in "$dir"/$pattern; do
            if [ -f "$file" ]; then
                ln -sf "$file" "$LIB_DIR/$name"
                return 0
            fi
        done
    done
}

link_lib "libglib-2.0.so" "libglib-2.0.so*"
link_lib "libgobject-2.0.so" "libgobject-2.0.so*"
link_lib "libgio-2.0.so" "libgio-2.0.so*"
link_lib "libgmodule-2.0.so" "libgmodule-2.0.so*"
link_lib "libgtk-3.so" "libgtk-3.so*"
link_lib "libgdk-3.so" "libgdk-3.so*"
link_lib "libsoup-3.0.so" "libsoup-3.0.so*"
link_lib "liblibsoup-3.0.so" "libsoup-3.0.so*"
link_lib "libatk-1.0.so" "libatk-1.0.so*"
link_lib "libpango-1.0.so" "libpango-1.0.so*"
link_lib "libpangocairo-1.0.so" "libpangocairo-1.0.so*"
link_lib "libpangoft2-1.0.so" "libpangoft2-1.0.so*"
link_lib "libcairo.so" "libcairo.so*"
link_lib "libcairo-gobject.so" "libcairo-gobject.so*"
link_lib "libgdk_pixbuf-2.0.so" "libgdk_pixbuf-2.0.so*"
link_lib "libappindicator3.so" "libappindicator3.so*"
link_lib "libharfbuzz.so" "libharfbuzz.so*"
link_lib "libepoxy.so" "libepoxy.so*"
link_lib "libfontconfig.so" "libfontconfig.so*"
link_lib "libfreetype.so" "libfreetype.so*"

# If webkit/javascriptcore runtime libraries are not in system path, provide stub shared objects for compile-time linking
if [ ! -f "$LIB_DIR/libwebkit2gtk-4.1.so" ]; then
    gcc -shared -x c /dev/null -o "$LIB_DIR/libwebkit2gtk-4.1.so"
fi

if [ ! -f "$LIB_DIR/libjavascriptcoregtk-4.1.so" ]; then
    gcc -shared -x c /dev/null -o "$LIB_DIR/libjavascriptcoregtk-4.1.so"
fi

if [ ! -f "$LIB_DIR/libayatana-appindicator3.so" ]; then
    gcc -shared -x c /dev/null -o "$LIB_DIR/libayatana-appindicator3.so"
fi

if [ ! -f "$LIB_DIR/libappindicator3.so" ]; then
    gcc -shared -x c /dev/null -o "$LIB_DIR/libappindicator3.so"
fi

ln -sf "$LIB_DIR/libsoup-3.0.so" "$LIB_DIR/libibsoup-3.0.so"

echo "Symlinks and stubs ready in $LIB_DIR"
