fn main() {
    // No Tauri codegen any more - just the Windows details the app needs.
    //
    // A build script runs on the *host*, so `#[cfg(windows)]` here would mean
    // "the machine doing the build is Windows", not "the binary being built is
    // for Windows". Cross-compiling from a Mac to Windows would then silently
    // drop every Windows-specific setting below. The target is asked for
    // explicitly instead.

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

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

        // 2. The DPI-awareness manifest Tauri used to emit. Without it the deck
        //    renders blurry on a scaled display and mouse coordinates drift
        //    from the painted pixels.
        if target_env == "msvc" {
            let manifest = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#;
            let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
            let manifest_path = std::path::Path::new(&out_dir).join("clipflow.manifest");
            std::fs::write(&manifest_path, manifest).expect("write application manifest");
            // `/MANIFESTINPUT` is merged into the default manifest rather than
            // replacing it, so the linker's UAC settings survive.
            //
            // `/MANIFEST:EMBED` is not optional here: the MSVC linker refuses
            // to take an input manifest unless it is also told to embed the
            // result, and fails the link with
            //   LNK1220: 'MANIFESTINPUT' requires '/MANIFEST:EMBED' specification
            // rustc passes no `/MANIFEST*` flag of its own on the MSVC target,
            // so nothing else supplies it. The flag was lost when the Tauri
            // build script was replaced by this one - tauri's build.rs emitted
            // both.
            println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
            println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest_path.display());
        }

        // 3. The app icon, compiled into the executable.
        //
        //    Without it the .exe (and every shortcut the installer makes) shows
        //    the generic "unknown application" icon. The resource compiler is
        //    `rc.exe` from the Windows SDK on Windows; it is not needed on the
        //    other platforms, so this is a no-op there.
        //
        //    A missing resource compiler is not fatal - an icon is cosmetic -
        //    but the warning says so, because on the release runner it means
        //    the shipped binary would have no icon.
        // The .rc lives next to the icon: see the comment in `icons/clipflow.rc`
        // for why the relative path has to resolve from two different working
        // directories.
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
