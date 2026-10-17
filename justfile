lint:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test
    # `tr` keeps one translator per process, so each language runs in a process of its own.
    GUI_TEST_LANG=pt_BR cargo test -p gui pt_br_
    # Every string on each shot must come from the translation files.
    GUI_TEST_LANG=qps cargo test -p gui snapshots
    # Alone, so a test that needs a feature only another crate turns on fails here.
    cargo test -p controller-core
    just translations-fresh
    just missing-strings

# Rebuild crates/gui/translations/gui.pot from the source and merge it into each gui.po.
# Needs gettext 0.21 or newer, and `cargo install slint-tr-extractor@1.18.1 xtr@0.1.11`.
translations:
    #!/usr/bin/env bash
    set -euo pipefail
    cd crates/gui
    {
        cat <<'EOF'
    # The English source strings of 8B. Written by `just translations`: do not edit.
    msgid ""
    msgstr ""
    "Project-Id-Version: 8B\n"
    "MIME-Version: 1.0\n"
    "Content-Type: text/plain; charset=UTF-8\n"
    "Content-Transfer-Encoding: 8bit\n"

    #. The language menu names each language in its own words.
    msgctxt "language name"
    msgid "English"
    msgstr ""

    EOF
        # Each extractor writes a header of its own: drop it, the one above stands.
        # C order, so every machine lists the files, and so the strings, the same way.
        find ui -name '*.slint' | LC_ALL=C sort | xargs slint-tr-extractor --no-default-translation-context -o /dev/stdout \
            | sed '1,/^$/d'
        xtr --omit-header --add-location file -o /dev/stdout src/main.rs
        echo
        # The .desktop and metainfo text. packaging/linux/translate-metadata.sh builds the shipped files.
        xgettext --omit-header --add-location=file -o - \
            ../../packaging/linux/io.github.LucasPicoli._8B.desktop \
            ../../packaging/linux/io.github.LucasPicoli._8B.metainfo.xml
        echo
        cargo run --quiet -p controller-core --example description-pot
    } | msguniq --add-location=file | sed '/^#\. #-#-#-#-#/d' > translations/gui.pot
    for po in translations/*/LC_MESSAGES/gui.po; do
        [[ $po == translations/qps/* ]] && continue
        msgmerge --quiet --update --backup=none --no-fuzzy-matching --no-wrap --add-location=file "$po" translations/gui.pot
    done
    # The qps pseudo-locale of the string coverage tests: each msgstr is its msgid in ⟦…⟧,
    # led by a `~` per 2.5 characters, so the text runs about 40 % longer, as pt-BR does.
    # crates/gui/src/string_coverage.rs wraps the Rust text the same way.
    mkdir -p translations/qps/LC_MESSAGES
    msgen translations/gui.pot \
        | sed -e '1s/.*/# The qps pseudo-locale of the string coverage tests. Written by `just translations`: do not edit./' \
            -e 's/^"Content-Transfer-Encoding: 8bit\\n"$/&\n"Language: qps\\n"\n"Plural-Forms: nplurals=2; plural=(n != 1);\\n"/' \
        | msgfilter --keep-header -o translations/qps/LC_MESSAGES/gui.po \
            perl -Mutf8 -CSD -0pe 's/\A(.*)\z/"⟦" . ("~" x int((length($1) * 2 + 4) \/ 5)) . "$1⟧"/se'

# Fail when the translation files are older than the source. It updates them first, so
# commit what it wrote.
translations-fresh:
    #!/usr/bin/env bash
    set -euo pipefail
    sum() { cat crates/gui/translations/gui.pot crates/gui/translations/*/LC_MESSAGES/gui.po | sha256sum; }
    before=$(sum)
    just translations
    if [[ $(sum) != "$before" ]]; then
        echo 'error: the translation files were older than the source. `just translations` updated them: commit the result.' >&2
        exit 1
    fi

# Warn about each string a language has not translated yet. It never fails: English stands in.
missing-strings:
    #!/usr/bin/env bash
    for po in crates/gui/translations/*/LC_MESSAGES/gui.po; do
        [[ $po == */qps/* ]] && continue
        msgcmp -N "$po" crates/gui/translations/gui.pot 2>&1 | grep -v 'this message is not used' || true
    done

# The hardware tests. `regenerate` is ignored too, but it rewrites the golden fixtures:
# run it by hand after an encoder change, never here.
hw:
    cargo test --features hardware --no-fail-fast -- --ignored --skip regenerate

# Build the x86_64 AppImage in ubuntu:22.04 (glibc 2.35), so it runs on that glibc and newer.
appimage:
    $(command -v podman || command -v docker) run --rm -v "{{justfile_directory()}}:/src" -w /src ubuntu:22.04 \
        bash -c 'packaging/linux/prepare-container.sh && packaging/linux/build-appimage.sh x86_64'

# Build 8B-<version>-<arch>.flatpak and its .sha256 into packaging/flatpak/out/. Needs flatpak-builder and uv.
flatpak:
    #!/usr/bin/env bash
    set -euo pipefail
    cd packaging/flatpak
    ./cargo-sources.sh
    version=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "gui") | .version')
    arch=$(uname -m)
    bundle=8B-$version-$arch.flatpak
    mkdir -p out
    flatpak-builder --user --install-deps-from=flathub --force-clean --disable-rofiles-fuse \
        --state-dir=out/state --repo=out/repo --default-branch=stable out/build io.github.LucasPicoli._8B.yml
    flatpak build-bundle out/repo "out/$bundle" --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
        --arch="$arch" io.github.LucasPicoli._8B stable
    (cd out && sha256sum "$bundle" > "$bundle.sha256")
    ls -l out/"$bundle" out/"$bundle.sha256"
