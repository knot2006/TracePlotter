#!/usr/bin/env bash
# Builds the Debian and RPM packages from the native release executable, with dpkg-deb and
# rpmbuild. Run after `cargo build --release`; needs the dpkg and rpm tools.
# The package names, trace-plotter and TracePlotter, are those of earlier releases, so that
# package managers take a new release as an upgrade.
set -euo pipefail
cd "$(dirname "$0")/.."
field() { sed -n "s/^$1 = \"\(.*\)\"/\1/p" Cargo.toml | head -n1; }
version=$(field version)
description=$(field description)
license=$(field license)
url=$(field repository)
binary=target/release/traceplotter
[ -x "$binary" ] || { echo "Build the release executable first: cargo build --release" >&2; exit 1; }
output="$PWD/target/release/bundle"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The files both packages install.
root="$work/root"
install -Dm755 "$binary" "$root/usr/bin/traceplotter"
install -Dm644 icons/32x32.png "$root/usr/share/icons/hicolor/32x32/apps/traceplotter.png"
install -Dm644 icons/128x128.png "$root/usr/share/icons/hicolor/128x128/apps/traceplotter.png"
install -Dm644 icons/128x128@2x.png "$root/usr/share/icons/hicolor/256x256@2/apps/traceplotter.png"
install -Dm644 /dev/stdin "$root/usr/share/applications/TracePlotter.desktop" <<END
[Desktop Entry]
Type=Application
Name=TracePlotter
Comment=$description
Exec=traceplotter
Icon=traceplotter
StartupWMClass=traceplotter
Categories=Network;Monitor;
Terminal=false
END

# Debian and Ubuntu.
deb="$work/deb"
cp -a "$root" "$deb"
install -Dm755 linux/postinst.sh "$deb/DEBIAN/postinst"
cat > "$deb/DEBIAN/control" <<END
Package: trace-plotter
Version: $version
Architecture: amd64
Installed-Size: $(du -sk "$root" | cut -f1)
Maintainer: TracePlotter contributors
Priority: optional
Section: net
Homepage: $url
Depends: libcap2-bin, libxkbcommon0, libwayland-client0, libwayland-cursor0, libx11-6, libxcursor1, libxrandr2, libxi6, libxcb1, libegl1, libgl1, libvulkan1, libdbus-1-3
Description: $description
END
mkdir -p "$output/deb"
dpkg-deb --root-owner-group --build "$deb" "$output/deb/TracePlotter_${version}_amd64.deb"

# Fedora and openSUSE. The dependencies are the libraries' sonames, which every RPM
# distribution provides under the same names.
cat > "$work/TracePlotter.spec" <<END
Name: TracePlotter
Version: $version
Release: 1
Summary: $description
License: $license
URL: $url
Requires: libcap
Requires: libxkbcommon.so.0()(64bit), libwayland-client.so.0()(64bit), libwayland-cursor.so.0()(64bit)
Requires: libX11.so.6()(64bit), libXcursor.so.1()(64bit), libXrandr.so.2()(64bit), libXi.so.6()(64bit)
Requires: libxcb.so.1()(64bit), libEGL.so.1()(64bit), libGL.so.1()(64bit), libvulkan.so.1()(64bit)
Requires: libdbus-1.so.3()(64bit)
AutoReqProv: no

%description
$description

%install
cp -a "$root/." %{buildroot}/

%post
$(tail -n +2 linux/postinst.sh)

%files
/usr/bin/traceplotter
/usr/share/applications/TracePlotter.desktop
/usr/share/icons/hicolor/32x32/apps/traceplotter.png
/usr/share/icons/hicolor/128x128/apps/traceplotter.png
/usr/share/icons/hicolor/256x256@2/apps/traceplotter.png
END
rpmbuild -bb --target x86_64 \
  --define "_topdir $work/rpmbuild" \
  --define "_rpmdir $output/rpm" \
  --define "_build_name_fmt %%{NAME}-%%{VERSION}-%%{RELEASE}.%%{ARCH}.rpm" \
  --define "debug_package %{nil}" \
  "$work/TracePlotter.spec"
