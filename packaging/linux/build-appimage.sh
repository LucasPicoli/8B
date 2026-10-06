#!/usr/bin/env bash
# Builds 8B-<version>-<arch>.AppImage, its .zsync and its .sha256 into packaging/linux/out/.
# Usage: packaging/linux/build-appimage.sh x86_64|aarch64
# Run it where glibc is old (ubuntu:22.04, see `just appimage`): the binary runs on that glibc and newer.
set -euo pipefail

arch=${1:?usage: build-appimage.sh x86_64|aarch64}
[[ $arch == "$(uname -m)" ]] || { echo "built for $(uname -m), not $arch" >&2; exit 1; }

# Bump a version and its checksum together. Both come from the GitHub release assets.
APPIMAGETOOL_URL=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$arch.AppImage
RUNTIME_URL=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-$arch
case $arch in
    x86_64)
        APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
        RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
    aarch64)
        APPIMAGETOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
        RUNTIME_SHA256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
    *) echo "unknown arch: $arch" >&2; exit 1 ;;
esac
MAX_GLIBC=2.35
APP_ID=io.github.LucasPicoli.8B

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=$here/out
appdir=$out/AppDir
cd "$root"
[[ -f $HOME/.cargo/env ]] && . "$HOME/.cargo/env"

fetch() { # url sha256 dest
    [[ -f $3 ]] && echo "$2  $3" | sha256sum -c --quiet - && return 0
    curl --proto '=https' --tlsv1.2 -sSfL -o "$3" "$1"
    echo "$2  $3" | sha256sum -c --quiet -
}

mkdir -p "$out/tools"
fetch "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA256" "$out/tools/appimagetool-$arch.AppImage"
fetch "$RUNTIME_URL" "$RUNTIME_SHA256" "$out/tools/runtime-$arch"
chmod +x "$out/tools/appimagetool-$arch.AppImage"

version=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "gui") | .version')
CARGO_TARGET_DIR=$out/target cargo build --release --locked -p gui
bin=$out/target/release/8b

# Gate: the oldest glibc we promise to run on.
top=$(readelf --dyn-syms -W "$bin" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -Vu | tail -n1)
echo "highest GLIBC symbol: $top (limit $MAX_GLIBC)"
[[ $(printf '%s\n%s\n' "$top" "$MAX_GLIBC" | sort -V | tail -n1) == "$MAX_GLIBC" ]] \
    || { echo "binary needs glibc $top, above $MAX_GLIBC" >&2; exit 1; }
echo "NEEDED libraries:"
readelf -d "$bin" | grep NEEDED

rm -rf "$appdir"
install -Dm755 "$bin" "$appdir/usr/bin/8b"
install -Dm644 "$here/$APP_ID.desktop" "$appdir/$APP_ID.desktop"
install -Dm644 "$here/$APP_ID.desktop" "$appdir/usr/share/applications/$APP_ID.desktop"
install -m644 "$here/$APP_ID.png" "$appdir/$APP_ID.png"
install -Dm644 "$here/$APP_ID-16.png" "$appdir/usr/share/icons/hicolor/16x16/apps/$APP_ID.png"
install -Dm644 "$here/$APP_ID-32.png" "$appdir/usr/share/icons/hicolor/32x32/apps/$APP_ID.png"
install -Dm644 "$here/$APP_ID.png" "$appdir/usr/share/icons/hicolor/256x256/apps/$APP_ID.png"
install -Dm644 "$here/$APP_ID.svg" "$appdir/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
ln -s usr/bin/8b "$appdir/AppRun"

# Gate: the host supplies the display and GL libraries; bundling them breaks EGL on new Mesa.
if find "$appdir" \( -name 'libwayland-*' -o -name 'libxkbcommon*' -o -name 'libxcb-*' \) | grep -q .; then
    echo "AppDir bundles a display library" >&2
    exit 1
fi

image=8B-$version-$arch.AppImage
cd "$out"
# The tool is itself an AppImage; run it unpacked so the container needs no FUSE.
ARCH=$arch APPIMAGE_EXTRACT_AND_RUN=1 "tools/appimagetool-$arch.AppImage" \
    --no-appstream --runtime-file "tools/runtime-$arch" \
    -u "gh-releases-zsync|LucasPicoli|8B|latest|8B-*-$arch.AppImage.zsync" \
    AppDir "$image"
sha256sum "$image" > "$image.sha256"
ls -l "$image" "$image.zsync" "$image.sha256"
