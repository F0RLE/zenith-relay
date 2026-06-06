# Platform Support

Zenith Codex is built for all major desktop platforms and architectures.

## Supported Platforms

| Platform | Architecture | Formats | Auto-Updates |
|----------|-------------|---------|--------------|
| **Windows** | x64 (64-bit) | `.exe` (portable), `-setup.exe` (installer), `.msi` | ✅ Yes |
| **Windows** | ARM64 | `.exe` (portable), `-setup.exe` (installer), `.msi` | ✅ Yes |
| **macOS** | Apple Silicon (ARM64) | `.dmg`, `.app.tar.gz` | ✅ Yes |
| **macOS** | Intel (x64) | `.dmg`, `.app.tar.gz` | ✅ Yes |
| **Linux** | x64 (64-bit) | `.AppImage`, `.deb`, `.rpm` | ✅ Yes |
| **Linux** | ARM64 | `.AppImage`, `.deb`, `.rpm` | ✅ Yes |

## Installation

### Windows

**Option 1: Setup Installer (Recommended)**
- Download `zenith-codex-windows-{arch}-setup.exe`
- Run the installer
- Supports automatic updates

**Option 2: Portable**
- Download `zenith-codex-windows-{arch}.exe`
- Run directly, no installation needed
- Updates require manual download

**Option 3: MSI**
- Download `zenith-codex-windows-{arch}.msi`
- Install via Windows Installer
- Suitable for enterprise deployment

### macOS

**DMG (Recommended)**
- Download `zenith-codex-macos-{arch}.dmg`
- Open DMG and drag to Applications
- First launch: Right-click → Open (bypass Gatekeeper)
- Supports automatic updates

**Archive**
- Download `zenith-codex-macos-{arch}.app.tar.gz`
- Extract and move to Applications
- Same Gatekeeper process as DMG

### Linux

**AppImage (Recommended)**
- Download `zenith-codex-linux-{arch}.AppImage`
- Make executable: `chmod +x zenith-codex-linux-{arch}.AppImage`
- Run: `./zenith-codex-linux-{arch}.AppImage`
- No installation, self-contained
- Supports automatic updates

**DEB (Debian/Ubuntu)**
```bash
sudo dpkg -i zenith-codex-linux-{arch}.deb
sudo apt-get install -f  # Fix dependencies if needed
```

**RPM (Fedora/RHEL)**
```bash
sudo rpm -i zenith-codex-linux-{arch}.rpm
```

## Build Matrix

GitHub Actions automatically builds for all platforms:

```yaml
- windows-x64      (windows-latest)
- windows-arm64    (windows-11-arm)
- macos-arm64      (macos-latest)
- macos-intel      (macos-15-intel)
- linux-x64        (ubuntu-latest)
- linux-arm64      (ubuntu-24.04-arm)
```

## Requirements

### Windows
- Windows 10 or later
- WebView2 (automatically installed if missing)

### macOS
- macOS 11 Big Sur or later
- Apple Silicon or Intel processor

### Linux
- Modern Linux distribution (Ubuntu 20.04+, Fedora 38+, etc.)
- GTK 3, WebKit2GTK 4.1
- For AppImage: FUSE or `--appimage-extract-and-run`

## Troubleshooting

### macOS: "App can't be opened"
Right-click the app → Open → Open anyway

### Linux: AppImage won't run
```bash
# Option 1: Install FUSE
sudo apt install fuse libfuse2  # Ubuntu/Debian
sudo dnf install fuse fuse-libs  # Fedora

# Option 2: Extract and run
./zenith-codex-linux-{arch}.AppImage --appimage-extract-and-run
```

### Updates not working?
See [docs/UPDATES.md](docs/UPDATES.md) for troubleshooting.
