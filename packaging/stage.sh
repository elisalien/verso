#!/usr/bin/env bash
# Copies a built Verso binary and the files it loads at run time into one
# folder, ready to archive.
#
#   packaging/stage.sh <binary> <version> <destination>
#
# Run from the repository root. Verso finds effects/ and presets/milkdrop/
# next to its executable, so they sit beside it in every package.
set -euo pipefail

binary=$1
version=$2
dest=$3

mkdir -p "$dest"
cp "$binary" "$dest/"
cp -r effects presets scripts "$dest/"
cp LICENSE "$dest/LICENSE.txt"
sed "s/@VERSION@/$version/" packaging/README.txt > "$dest/README.txt"
