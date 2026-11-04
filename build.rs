fn main() {
    // No Tauri codegen any more - just the two Windows details the app needs.

    // 1. Media Foundation + DXGI are delay-loaded on purpose: ClipFlow must
    //    still launch on Windows N/KN editions that ship without the media
    //    pack, so we can surface a friendly error instead of a hard loader
    //    failure.
    #[cfg(windows)]
    {
        println!("cargo:rustc-link-arg=/DELAYLOAD:mf.dll");
        println!("cargo:rustc-link-arg=/DELAYLOAD:mfplat.dll");
        println!("cargo:rustc-link-arg=/DELAYLOAD:mfreadwrite.dll");
        println!("cargo:rustc-link-arg=/DELAYLOAD:mfuuid.dll");
        println!("cargo:rustc-link-lib=dylib=delayimp");
    }

    // 2. The DPI-awareness manifest Tauri used to emit. Without it the deck
    //    renders blurry on a scaled display and mouse coordinates drift from
    //    the painted pixels.
    #[cfg(all(windows, target_env = "msvc"))]
    {
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
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest_path.display());
    }
}
