#!/usr/bin/env nu
# Package fffinder for the current platform into dist/.
#
#   macOS:   dist/fffinder.app (fff.nu inside Contents/Resources)
#   Linux:   dist/fffinder-<version>-linux-<arch>/ with a .desktop file
#            (--install copies it into ~/.local)
#   Windows: dist/fffinder-<version>-windows-<arch>.zip

use common.nu *

def main [
    --install   # Linux: install the desktop entry and icon for this user
    --skip-build # Package existing release binaries
    --term      # Include terminal mode, `fffinder --term`
] {
    let root = repo-root
    let target = if $skip_build { target-dir | path join release } else { build --term=$term }
    let dist = $root | path join dist
    mkdir $dist
    match $nu.os-info.name {
        'macos' => (bundle-macos $root $target $dist)
        'windows' => (bundle-windows $root $target $dist)
        _ => (bundle-linux $root $target $dist --install=$install)
    }
}

def bundle-macos [root: path, target: path, dist: path]: nothing -> path {
    let app = $dist | path join fffinder.app
    rm --recursive --force $app
    let contents = $app | path join Contents
    mkdir ($contents | path join MacOS) ($contents | path join Resources)
    cp ($target | path join fffinder) ($contents | path join MacOS)
    cp ($root | path join nu fff.nu) ($contents | path join Resources)
    make-icns ($root | path join assets app-icon.svg) ($contents | path join Resources fffinder.icns)
    info-plist (version) | save --force ($contents | path join Info.plist)
    # Ad-hoc signature so Gatekeeper and Full Disk Access can track the app.
    ^codesign --force --deep --sign - $app
    print $"Built ($app). Drag it to /Applications; grant Full Disk Access to search protected folders."
    $app
}

# Render the SVG with Quick Look, then build an iconset with sips and iconutil.
def make-icns [svg: path, out: path] {
    let work = mktemp --directory
    let png = render-svg $svg $work
    let iconset = $work | path join fffinder.iconset
    mkdir $iconset
    for size in [16 32 128 256 512] {
        ^sips -z $size $size $png --out ($iconset | path join $'icon_($size)x($size).png') | ignore
        let double = $size * 2
        ^sips -z $double $double $png --out ($iconset | path join $'icon_($size)x($size)@2x.png') | ignore
    }
    ^iconutil --convert icns --output $out $iconset
    rm --recursive --force $work
}

def info-plist [version: string]: nothing -> string {
    $'<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>fffinder</string>
  <key>CFBundleDisplayName</key><string>fffinder</string>
  <key>CFBundleIdentifier</key><string>com.nushell.fffinder</string>
  <key>CFBundleExecutable</key><string>fffinder</string>
  <key>CFBundleIconFile</key><string>fffinder</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>($version)</string>
  <key>CFBundleVersion</key><string>($version)</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSAppleEventsUsageDescription</key><string>fffinder asks your terminal, such as Ghostty, to open a window running Nushell in a folder.</string>
</dict>
</plist>
'
}

def bundle-linux [root: path, target: path, dist: path, --install]: nothing -> path {
    let name = $'fffinder-(version)-linux-($nu.os-info.arch)'
    let out = $dist | path join $name
    rm --recursive --force $out
    mkdir $out
    cp ($target | path join fffinder) ($root | path join nu fff.nu) $out
    cp ($root | path join assets app-icon.svg) ($out | path join fffinder.svg)
    desktop-entry fffinder | save --force ($out | path join fffinder.desktop)
    if $install {
        let bin = $nu.home-dir | path join .local bin
        mkdir $bin
        cp ($out | path join fffinder) $bin
        install-desktop-entry fffinder
        print $"Installed to ($bin) and the desktop menu."
    }
    $out
}

def bundle-windows [root: path, target: path, dist: path]: nothing -> path {
    let name = $'fffinder-(version)-windows-($nu.os-info.arch)'
    let out = $dist | path join $name
    rm --recursive --force $out
    mkdir $out
    cp ($target | path join fffinder.exe) ($root | path join nu fff.nu) $out
    cp ($root | path join README.md) $out
    let zip = $dist | path join $'($name).zip'
    rm --force $zip
    cd $dist
    ^tar -a -c -f $zip $name
    $zip
}
