fn main() {
    // No Tauri codegen any more - just the Windows details the app needs.
    //
    // A build script runs on the *host*, so `#[cfg(windows)]` here would mean
    // "the machine doing the build is Windows", not "the binary being built is
    // for Windows". Cross-compiling from a Mac to Windows would then silently
    // drop every Windows-specific setting below. The target is asked for
    // explicitly instead.

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "windows" {
        // 1. Media Foundation + DXGI are delay-loaded on purpose: ClipFlow must
        //    still launch on Windows N/KN editions that ship without the media
        //    pack, so we can surface a friendly error instead of a hard loader
        //    failure.
        println!("cargo:rustc-link-arg=/DELAYLOAD:mf.dll");
        println!("cargo:rustc-link-arg=/DELAYLOAD:mfplat.dll");
        println!("cargo:rustc-link-arg=/DELAYLOAD:mfreadwrite.dll");
        println!("cargo:rustc-link-arg=/DELAYLOAD:mfuuid.dll");
        println!("cargo:rustc-link-lib=dylib=delayimp");

        // 2. The app icon, compiled into the executable.
        //
        //    Without it the .exe (and every shortcut the installer makes) shows
        //    the generic "unknown application" icon. The resource compiler is
        //    `rc.exe` from the Windows SDK on Windows; it is not needed on the
        //    other platforms, so this is a no-op there.
        //
        //    A missing resource compiler is not fatal - an icon is cosmetic -
        //    but the warning says so, because on the release runner it means
        //    the shipped binary would have no icon.
        //
        //    The DPI-awareness / UAC manifest is deliberately *not* emitted
        //    here. gpui-pre already embeds its own manifest (`1 RT_MANIFEST`)
        //    through its default `windows-manifest` feature, which covers DPI
        //    awareness, UAC `asInvoker`, SegmentHeap and the Common-Controls 6
        //    dependency. Adding a second manifest through `/MANIFESTINPUT` +
        //    `/MANIFEST:EMBED` puts two `type:MANIFEST, name:1` resources into
        //    the same binary and the linker dies with
        //      CVTRES : fatal error CVT1100: duplicate resource
        //    so ClipFlow relies on gpui's manifest alone.
        //
        //    The .rc lives next to the icon: see the comment in
        //    `icons/clipflow.rc` for why the relative path has to resolve from
        //    two different working directories.
        let icon_dir = std::path::Path::new("icons");
        let rc_file = icon_dir.join("clipflow.rc");
        println!("cargo:rerun-if-changed={}", rc_file.display());
        println!("cargo:rerun-if-changed=icons/icon.ico");

        let result = embed_resource::compile(
            rc_file,
            embed_resource::ParamsIncludeDirs([icon_dir]),
        );
        match result {
            embed_resource::CompilationResult::Ok => {
                println!("cargo:warning=app icon compiled into the executable")
            }
            other => {
                println!("cargo:warning=app icon not compiled into the exe: {other}");
            }
        }
    }
}
