# Building & Packaging Releases

logotomy ships **native OS installers** (not raw binaries) via
[cargo-packager](https://github.com/crabnebula-dev/cargo-packager). Installers
must be built **on their own host OS** (installer wrappers — NSIS, dpkg,
hdiutil — don't cross-compile reliably).

| Platform | Formats | Output (default `target/<triple>/release/`) |
|---|---|---|
| Windows x86_64 | `nsis` | `logotomy-<version>-setup.exe` |
| Ubuntu x86_64 | `deb,appimage` | `logotomy_<version>_amd64.deb`, `logotomy-<version>-x86_64.AppImage` |
| macOS Apple Silicon | `app,dmg` | `logotomy_<version>_aarch64.dmg` (+ `logotomy.app`) |
| macOS Intel | `app,dmg` | `logotomy_<version>_x64.dmg` (+ `logotomy.app`) |

## 1. Install the tool

```bash
cargo install cargo-packager --locked
```

The packaging config lives in `Cargo.toml` under `[package.metadata.packager]`
(identifier, product name/version, app icons, file associations, and
NSIS/macOS/Linux options). The native installers register `.log`, `.txt`,
`.out`, `.err`, `.evt`, `.evtx`, `.sys`, `.csv`, `.json`, `.xml`, and `.md` with
logotomy. On Linux, registration follows the MIME types advertised by the
desktop entry; Windows and macOS register the extensions directly.

### Platform prerequisites

- **Windows**: install [NSIS](https://nsis.sourceforge.io/) (e.g. `choco install nsis -y`).
- **macOS**: nothing extra — `hdiutil` ships with the OS.
- **Ubuntu/Linux**: `dpkg` tooling (present by default on Ubuntu); AppImage
  downloads `appimagetool` on first use.

## 2. Build the individual binary, then make the installer

Exactly two commands per platform — the packager wraps the binary that
`cargo build` already produced. (`cargo-packager` does **not** build for you;
it packages whatever release binary exists for the `--target` you pass.)

> The raw per-platform binary is what you build first:
> `target/<triple>/release/logotomy` (Windows uses `logotomy.exe`).

### Windows (generates `logotomy-<version>-setup.exe`)

```bash
rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
cargo packager --release --formats nsis --target x86_64-pc-windows-msvc
```

The runnable binary is `target/x86_64-pc-windows-msvc/release/logotomy.exe`
(correct `.exe` extension; icon/version info embedded at build time by
`build.rs` → `winres`).

### Ubuntu / Linux x86_64 (`.deb` + `.AppImage`)

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --release --target x86_64-unknown-linux-gnu
cargo packager --release --formats deb,appimage --target x86_64-unknown-linux-gnu
```

### macOS Apple Silicon (`dmg`)

```bash
rustup target add aarch64-apple-darwin
cargo build --release --target aarch64-apple-darwin
cargo packager --release --formats dmg --target aarch64-apple-darwin
```

### macOS Intel (`dmg`)

```bash
rustup target add x86_64-apple-darwin
cargo build --release --target x86_64-apple-darwin
cargo packager --release --formats dmg --target x86_64-apple-darwin
```

## 3. Where the installers land

By default cargo-packager writes next to the built binary (add `--out-dir dist`
to put them in a clean `dist/` folder instead):

```
target/<triple>/release/
   logotomy-0.1.0-setup.exe            # Windows
   logotomy_0.1.0_amd64.deb            # Ubuntu
   logotomy-0.1.0-x86_64.AppImage      # Linux (arch label may vary)
   logotomy.app/                       # macOS app bundle
   logotomy_0.1.0_aarch64.dmg          # macOS Apple Silicon
   logotomy_0.1.0_x64.dmg              # macOS Intel
```

> The installer's internal version comes from `Cargo.toml` (`[package].version`);
> keep it in sync with the release tag (`v<version>`).

## 4. CI/CD

`.github/workflows/release.yml` does all four automatically when a matching
`v*` tag is pushed: version validation, tests + bench, then packaging
(`--out-dir dist`), then staging renamed
installers (`logotomy-<tag>-<target>.<ext>`) and uploading them plus
the combined `benchmark-results.txt` and `checksums.sha256` to the GitHub
Release. Raw tarball/zip archives are **not** published anymore.

The packager is pinned to version `0.11.8` for reproducible release builds.
The Intel macOS target runs on a native `macos-15-intel` runner; the Apple
Silicon target runs on `macos-latest`.

Each platform job benchmarks the release build against the generated
`iOS-100K.log`. The release job combines the per-target output into
`benchmark-results.txt`, so the GitHub Release contains one file with the
Windows, Ubuntu/Linux, macOS Apple Silicon, and macOS Intel results.

## 5. Manual smoke test

After building an installer you can verify it right away. The GitHub Release
also includes these first-run instructions:
- **Windows**: double-click `logotomy-<version>-setup.exe` → install → launch.
- **Ubuntu**: `sudo apt install ./logotomy_0.1.0_amd64.deb` → run `logotomy`.
- **macOS**: open the DMG → drag `logotomy.app` to Applications → right-click
  the app and choose **Open** on first launch. If it remains blocked, remove
  quarantine only after verifying the download: `xattr -d
  com.apple.quarantine /Applications/logotomy.app`.
- **Windows**: if SmartScreen shows “Windows protected your PC”, verify the
  download, click **More info**, then **Run anyway**.
- **Ubuntu/Linux AppImage**: run `chmod +x logotomy-*.AppImage` before launching.

## Future work
- Code signing: Windows Authenticode (SmartScreen), macOS Developer-ID +
  notarization (Gatekeeper), Linux per-repo signatures.
