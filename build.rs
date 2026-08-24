//! Embeds the app icon + version metadata into the Windows `.exe` itself
//! (Explorer icon, installed-app taskbar icon, file properties). No-op on
//! other platforms — the macOS `.icns`/`.app` and Linux desktop icons are
//! produced by cargo-packager at packaging time.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // Only embed resources on real Windows targets. Because build scripts run
    // on the host, `winres` (a cfg(windows) build-dependency) is only linked
    // when building on a Windows host, which is exactly the native case this
    // repo needs (windows-latest runner → x86_64-pc-windows-msvc).
    if target_os != "windows" {
        return;
    }

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/logotomy.ico"
        ));
        res.set(
            "FileDescription",
            "logotomy — high-performance log analyzer",
        );
        res.set("ProductName", "logotomy");
        res.set("ProductVersion", "0.1.0");
        res.set("FileVersion", "0.1.0");
        res.set("LegalCopyright", "Copyright (c) 2026 MK");
        res.set("OriginalFilename", "logotomy.exe");
        if let Err(e) = res.compile() {
            // Non-fatal: the NSIS installer icon and eframe runtime window icon
            // still apply even if the static .exe icon fails to embed.
            println!("cargo:warning=winres: failed to embed .exe icon/version: {e}");
        }
    }
}
