#!/bin/bash

# Material 3 File Manager Installer

APP_NAME="Materials"
INSTALL_DIR="/opt/$APP_NAME"
ICON_DIR="/usr/share/icons/hicolor/512x512/apps"
DESKTOP_FILE="/usr/share/applications/$APP_NAME.desktop"
APP_IMAGE_SOURCE="./release/Materials-0.0.0.AppImage"
ICON_SOURCE="./assets/icon.png"

if [ "$EUID" -ne 0 ]; then
  echo "Please run as root"
  exit 1
fi

echo "Installing $APP_NAME..."

# Create directories
mkdir -p "$INSTALL_DIR"
mkdir -p "$ICON_DIR"

# Copy AppImage
if [ -f "$APP_IMAGE_SOURCE" ]; then
    cp "$APP_IMAGE_SOURCE" "$INSTALL_DIR/$APP_NAME.AppImage"
    chmod +x "$INSTALL_DIR/$APP_NAME.AppImage"
    echo "AppImage installed to $INSTALL_DIR/$APP_NAME.AppImage"
else
    echo "Error: AppImage not found at $APP_IMAGE_SOURCE. Please build the app first with 'npm run electron:build'."
    exit 1
fi

# Copy Icon
if [ -f "$ICON_SOURCE" ]; then
    cp "$ICON_SOURCE" "$ICON_DIR/$APP_NAME.png"
    echo "Icon installed to $ICON_DIR/$APP_NAME.png"
else
    echo "Warning: Icon not found at $ICON_SOURCE"
fi

# Create Desktop Entry
cat > "$DESKTOP_FILE" << EOF
[Desktop Entry]
Name=$APP_NAME
Exec="$INSTALL_DIR/$APP_NAME.AppImage"
Icon=$APP_NAME
Type=Application
Categories=Utility;FileManager;
Comment=A modern Material Design 3 File Manager
Terminal=false
StartupWMClass=Materials
EOF

echo "Desktop entry created at $DESKTOP_FILE"
echo "Installation complete!"
