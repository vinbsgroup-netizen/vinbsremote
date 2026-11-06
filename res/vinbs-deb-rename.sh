#!/bin/bash
# Renames the stock rustdesk deb to match APP_NAME: the Linux code builds its systemctl calls,
# process lookups and desktop file names from that name, so a package still called rustdesk would
# leave them pointing at nothing. Icons keep their name because flutter/linux/my_application.cc
# loads "rustdesk" from the icon theme. /usr/lib/rustdesk stays too: drmtap_dl.rs loads libdrmtap
# from that absolute path.
set -euo pipefail

src=$1
dst=$2
app=vinbsremote
pkg=vinbs-deb-root

rm -rf "$pkg"
dpkg-deb -R "$src" "$pkg"

mv "$pkg/usr/share/rustdesk" "$pkg/usr/share/$app"
mv "$pkg/usr/share/$app/rustdesk" "$pkg/usr/share/$app/$app"
mv "$pkg/usr/share/$app/files/systemd/rustdesk.service" "$pkg/usr/share/$app/files/systemd/$app.service"
mv "$pkg/usr/share/applications/rustdesk.desktop" "$pkg/usr/share/applications/$app.desktop"
mv "$pkg/usr/share/applications/rustdesk-link.desktop" "$pkg/usr/share/applications/$app-link.desktop"

sed -i -e 's/^Description=RustDesk$/Description=VinBS Remote Desktop/' -e "s/rustdesk/$app/g" \
    "$pkg/usr/share/$app/files/systemd/$app.service"
sed -i -E "s/^(Exec|TryExec|StartupWMClass)=rustdesk/\1=$app/" \
    "$pkg/usr/share/applications/$app.desktop" "$pkg/usr/share/applications/$app-link.desktop"
sed -i "s/rustdesk/$app/g" "$pkg"/DEBIAN/{preinst,postinst,prerm,postrm}
sed -i -e "s/^Package: .*/Package: $app/" \
    -e 's/^Maintainer: .*/Maintainer: VINBS <VinbsGroup@gmail.com>/' \
    -e 's|^Homepage: .*|Homepage: https://vi-sos.com/remote-desktop|' \
    -e 's/^Description: .*/Description: VinBS Remote Desktop/' \
    "$pkg/DEBIAN/control"
(cd "$pkg" && find usr -type f -exec md5sum {} + > DEBIAN/md5sums)

dpkg-deb -b "$pkg" "$dst"
rm -rf "$pkg" "$src"
