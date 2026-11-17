#!/usr/bin/env bash
# Builds the .deb and .rpm packages from a staged folder (see stage.sh).
#
#   packaging/linux/build-packages.sh <version> <staged folder> <output folder>
#
# Layout: the program and its data in /usr/lib/verso (Verso looks for its
# effects and presets next to the executable, and /proc/self/exe resolves the
# /usr/bin symlink), a launcher in /usr/share/applications, a scalable icon.
# FFmpeg is not bundled: the distribution's package is recommended instead.
set -euo pipefail

version=$1
stage=$(realpath "$2")
out=$(realpath "$3")
here=$(dirname "$(realpath "$0")")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

root=$work/root
mkdir -p "$root/usr/lib/verso" "$root/usr/bin" "$root/usr/share/applications" \
    "$root/usr/share/icons/hicolor/scalable/apps" "$root/usr/share/doc/verso"
cp "$stage/verso" "$root/usr/lib/verso/"
cp -r "$stage/effects" "$stage/presets" "$stage/scripts" "$root/usr/lib/verso/"
ln -s ../lib/verso/verso "$root/usr/bin/verso"
cp "$here/verso.desktop" "$root/usr/share/applications/"
cp "$here/../verso.svg" "$root/usr/share/icons/hicolor/scalable/apps/"
cp "$stage/LICENSE.txt" "$stage/README.txt" "$root/usr/share/doc/verso/"

summary="Free and open source VJ software"
description="Clip grid of layers and columns, GPU compositing with blend modes,
effects (WGSL and ISF), native MilkDrop presets, Advanced Output with slices
and warping, cue list, OSC, MIDI, NDI output and Lua scripts."

# Debian / Ubuntu
deb=$work/deb
cp -a "$root" "$deb"
mkdir -p "$deb/DEBIAN"
cat > "$deb/DEBIAN/control" <<EOF
Package: verso
Version: $version
Section: video
Priority: optional
Architecture: amd64
Maintainer: Verso <https://github.com/elisalien/verso>
Depends: libc6 (>= 2.35), libasound2t64 | libasound2, libvulkan1, libxkbcommon0
Recommends: ffmpeg
Homepage: https://elisalien.github.io/verso/
Description: $summary
$(echo "$description" | sed 's/^/ /')
EOF
dpkg-deb --build --root-owner-group "$deb" "$out/verso_${version}_amd64.deb"

# Fedora / openSUSE. Built with rpmbuild on any distribution: the
# distribution-specific post-install scripts and dependency scanners are off,
# so the requirements are the ones written here.
rpmtop=$work/rpm
mkdir -p "$rpmtop"/{BUILD,RPMS,SPECS}
cat > "$rpmtop/SPECS/verso.spec" <<EOF
Name:           verso
Version:        $version
Release:        1
Summary:        $summary
License:        GPL-3.0-or-later
URL:            https://elisalien.github.io/verso/
AutoReqProv:    no
Requires:       alsa-lib, vulkan-loader, libxkbcommon
Recommends:     /usr/bin/ffmpeg
%global debug_package %{nil}
%global __os_install_post %{nil}
%define _build_id_links none

%description
$description

%install
cp -a $root/. %{buildroot}/

%files
/usr/lib/verso
/usr/bin/verso
/usr/share/applications/verso.desktop
/usr/share/icons/hicolor/scalable/apps/verso.svg
%doc /usr/share/doc/verso
EOF
rpmbuild --define "_topdir $rpmtop" --target x86_64 -bb "$rpmtop/SPECS/verso.spec"
cp "$rpmtop"/RPMS/x86_64/*.rpm "$out/"
