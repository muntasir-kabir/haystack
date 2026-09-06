//! Streaming ZIP extraction into a fresh sibling folder; never overwrite user files.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
pub(super) struct ExtractionProgress {
    pub cancel: AtomicBool,
    pub completed: AtomicUsize,
    pub total: AtomicUsize,
}

pub(super) fn is_zip(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

fn create_destination(archive: &Path) -> std::io::Result<PathBuf> {
    let parent = archive.parent().unwrap_or_else(|| Path::new("."));
    let stem = archive.file_stem().unwrap_or_else(|| "archive".as_ref());
    for suffix in 1u64.. {
        let mut name = stem.to_os_string();
        if suffix > 1 {
            name.push(format!(" ({suffix})"));
        }
        let destination = parent.join(name);
        #[allow(unused_mut)] // Configured with a private mode on Unix.
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&destination) {
            Ok(()) => return Ok(destination),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

pub(super) fn extract_zip(
    path: &Path,
    progress: &ExtractionProgress,
) -> Result<Option<PathBuf>, String> {
    let file = File::open(path).map_err(|error| format!("Cannot read ZIP: {error}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| format!("Cannot read ZIP archive: {error}"))?;
    let destination = create_destination(path)
        .map_err(|error| format!("Cannot create an extraction folder beside the ZIP: {error}"))?;
    let result = (|| -> Result<Option<PathBuf>, String> {
        progress.total.store(archive.len(), Ordering::Relaxed);
        let mut files = 0;
        let mut buffer = [0u8; 64 * 1024];
        for index in 0..archive.len() {
            if progress.cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let mut entry = archive.by_index(index).map_err(|error| {
                format!(
                    "Cannot extract ZIP entry: {error}. Password-protected ZIPs are not supported."
                )
            })?;
            let relative = entry
                .enclosed_name()
                .ok_or_else(|| format!("The ZIP contains an unsafe path: {}", entry.name()))?;
            // Reject platform-specific path syntax on every host, as well as
            // links, so extraction cannot escape the newly created folder.
            if entry.name().starts_with('/')
                || entry.name().contains(['\\', ':'])
                || relative
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
                || entry.is_symlink()
            {
                return Err(format!(
                    "The ZIP contains an unsafe path or symbolic link: {}",
                    entry.name()
                ));
            }
            let relative: PathBuf = relative
                .components()
                .filter(|part| !matches!(part, Component::CurDir))
                .collect();
            if relative.starts_with("__MACOSX")
                || relative.file_name().is_some_and(|name| name == ".DS_Store")
            {
                progress.completed.store(index + 1, Ordering::Relaxed);
                continue;
            }
            let output = destination.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(&output).map_err(|error| error.to_string())?;
            } else {
                fs::create_dir_all(output.parent().unwrap()).map_err(|error| error.to_string())?;
                let mut target = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output)
                    .map_err(|error| format!("Cannot extract {}: {error}", entry.name()))?;
                loop {
                    if progress.cancel.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    let count = entry
                        .read(&mut buffer)
                        .map_err(|error| format!("ZIP data is damaged or unsupported: {error}"))?;
                    if count == 0 {
                        break;
                    }
                    target
                        .write_all(&buffer[..count])
                        .map_err(|error| format!("Cannot write extracted file: {error}"))?;
                }
                files += 1;
            }
            progress.completed.store(index + 1, Ordering::Relaxed);
        }
        if files == 0 {
            return Err("This ZIP contains no files to open.".into());
        }
        Ok(Some(destination.clone()))
    })();
    if !matches!(result, Ok(Some(_))) {
        if let Err(error) = fs::remove_dir_all(&destination) {
            return Err(format!(
                "{} Could not remove the incomplete folder {}: {error}",
                result
                    .err()
                    .unwrap_or_else(|| "Extraction canceled.".into()),
                destination.display()
            ));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn make_zip(path: &Path, entries: &[(&str, &str)]) {
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, contents) in entries {
            writer
                .start_file(
                    *name,
                    SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(contents.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn recognizes_zip_extensions_without_changing_text_file_routing() {
        for name in ["logs.zip", "logs.ZIP", "logs.Zip"] {
            assert!(is_zip(Path::new(name)));
        }
        for name in ["logs.log", "logs.txt", "logs.zip.log", "zip"] {
            assert!(!is_zip(Path::new(name)));
        }
    }

    #[test]
    fn extracts_nested_files_beside_archive_and_skips_macos_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("support logs.ZIP");
        make_zip(
            &path,
            &[
                ("nested/app.log", "INFO ready\n"),
                ("server.txt", "ERROR failed\n"),
                ("__MACOSX/._app.log", "metadata"),
                ("nested/.DS_Store", "metadata"),
            ],
        );
        let progress = ExtractionProgress::default();
        let folder = extract_zip(&path, &progress).unwrap().unwrap();
        assert_eq!(folder, temp.path().join("support logs"));
        assert_eq!(
            fs::read_to_string(folder.join("nested/app.log")).unwrap(),
            "INFO ready\n"
        );
        assert_eq!(
            fs::read_to_string(folder.join("server.txt")).unwrap(),
            "ERROR failed\n"
        );
        assert!(!folder.join("__MACOSX").exists());
        assert!(!folder.join("nested/.DS_Store").exists());
        assert_eq!(progress.completed.load(Ordering::Relaxed), 4);
        assert!(path.exists());
    }

    #[test]
    fn accepts_current_directory_prefixes_and_explicit_directories() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("logs.zip");
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        writer
            .add_directory("./", SimpleFileOptions::default())
            .unwrap();
        writer
            .add_directory("./nested/", SimpleFileOptions::default())
            .unwrap();
        writer
            .start_file("./nested/app.log", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"ready").unwrap();
        writer.finish().unwrap();
        let folder = extract_zip(&path, &ExtractionProgress::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            fs::read_to_string(folder.join("nested/app.log")).unwrap(),
            "ready"
        );
    }

    #[test]
    fn preserves_existing_folders_and_files_when_choosing_destination() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("logs.zip");
        make_zip(&path, &[("app.log", "new")]);
        fs::create_dir(temp.path().join("logs")).unwrap();
        fs::write(temp.path().join("logs/app.log"), "original").unwrap();
        fs::write(temp.path().join("logs (2)"), "existing file").unwrap();
        assert_eq!(
            extract_zip(&path, &ExtractionProgress::default())
                .unwrap()
                .unwrap(),
            temp.path().join("logs (3)")
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("logs/app.log")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("logs (2)")).unwrap(),
            "existing file"
        );
    }

    #[test]
    fn rejects_unsafe_paths_and_removes_already_extracted_files() {
        for name in [
            "../escape.log",
            "/absolute.log",
            "C:/escape.log",
            "..\\escape.log",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("logs.zip");
            make_zip(&path, &[("safe.log", "safe"), (name, "unsafe")]);
            assert!(
                extract_zip(&path, &ExtractionProgress::default()).is_err(),
                "{name}"
            );
            assert!(!temp.path().join("logs").exists());
            assert!(!temp.path().join("escape.log").exists());
            assert!(path.exists());
        }
    }

    #[test]
    fn rejects_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("logs.zip");
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        writer
            .add_symlink("link", "..", SimpleFileOptions::default())
            .unwrap();
        writer.finish().unwrap();
        assert!(extract_zip(&path, &ExtractionProgress::default())
            .unwrap_err()
            .contains("symbolic link"));
        assert!(!temp.path().join("logs").exists());
    }

    #[test]
    fn canceled_empty_and_invalid_archives_leave_no_extraction_folder() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("logs.zip");
        make_zip(&path, &[("app.log", "contents")]);
        let progress = ExtractionProgress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        assert_eq!(extract_zip(&path, &progress).unwrap(), None);
        assert!(!temp.path().join("logs").exists());
        make_zip(&path, &[]);
        assert!(extract_zip(&path, &ExtractionProgress::default())
            .unwrap_err()
            .contains("no files"));
        assert!(!temp.path().join("logs").exists());
        fs::write(&path, "not a ZIP").unwrap();
        assert!(extract_zip(&path, &ExtractionProgress::default()).is_err());
        assert!(!temp.path().join("logs").exists());
    }

    #[test]
    fn corrupt_file_data_removes_partial_extraction() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("logs.zip");
        make_zip(&path, &[("app.log", "contents contents contents")]);
        let mut bytes = fs::read(&path).unwrap();
        let offset = {
            let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
            let entry = archive.by_index(0).unwrap();
            entry.data_start().unwrap() as usize
        };
        bytes[offset] ^= 0xff;
        fs::write(&path, bytes).unwrap();
        assert!(extract_zip(&path, &ExtractionProgress::default()).is_err());
        assert!(!temp.path().join("logs").exists());
    }
}
