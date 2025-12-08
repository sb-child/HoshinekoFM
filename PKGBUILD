# Maintainer: Bhima <bhima.work@gmail.com>
pkgname=material-3-file-manager-bin
pkgver=0.1.0
pkgrel=1
pkgdesc="A modern Material Design 3 File Manager"
arch=('x86_64')
url="https://github.com/bhimio1/material-3-file-explorer"
license=('MIT')
depends=('fuse2' 'gtk3' 'zlib' 'nss')
provides=('material-3-file-manager')
conflicts=('material-3-file-manager')
options=('!strip')
_appimage="Materials-${pkgver}.AppImage"

# Using GitHub Release source
source=("${_appimage}::https://github.com/bhimio1/material-3-file-explorer/releases/download/v${pkgver}/${_appimage}"
        "https://raw.githubusercontent.com/bhimio1/material-3-file-explorer/v${pkgver}/assets/icon.png")
sha256sums=('SKIP' 'SKIP')

package() {
    install -Dm755 "${srcdir}/${_appimage}" "${pkgdir}/opt/${pkgname}/${pkgname}.AppImage"
    install -Dm644 "${srcdir}/icon.png" "${pkgdir}/usr/share/icons/hicolor/512x512/apps/${pkgname}.png"

    # Create desktop entry
    mkdir -p "${pkgdir}/usr/share/applications"
    cat > "${pkgdir}/usr/share/applications/${pkgname}.desktop" << EOF
[Desktop Entry]
Name=Materials
Exec=/opt/${pkgname}/${pkgname}.AppImage
Icon=${pkgname}
Type=Application
Categories=Utility;FileManager;
Comment=A modern Material Design 3 File Manager
Terminal=false
StartupWMClass=Materials
EOF
}
