#!/usr/bin/env bash
# Writes the .desktop and metainfo files, with every language in crates/gui/translations, into <dir>.
# Usage: packaging/linux/translate-metadata.sh <dir>
# The files next to this script hold the English source; `just translations` extracts their text.
set -euo pipefail

dir=${1:?usage: translate-metadata.sh <dir>}
APP_ID=io.github.LucasPicoli._8B
here=$(cd "$(dirname "$0")" && pwd)
translations=$here/../../crates/gui/translations

# msgfmt reads <podir>/LINGUAS and one <lang>.po per language.
podir=$(mktemp -d)
trap 'rm -rf "$podir"' EXIT
for po in "$translations"/*/LC_MESSAGES/gui.po; do
    lang=$(basename "$(dirname "$(dirname "$po")")")
    cp "$po" "$podir/$lang.po"
    echo "$lang" >> "$podir/LINGUAS"
done

mkdir -p "$dir"
msgfmt --desktop --template "$here/$APP_ID.desktop" -d "$podir" -o "$dir/$APP_ID.desktop"
msgfmt --xml --template "$here/$APP_ID.metainfo.xml" -d "$podir" -o "$dir/$APP_ID.metainfo.xml"
