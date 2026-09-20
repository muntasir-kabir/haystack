//! Versioned reusable record-profile presets, separate from per-file sidecars.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{CompiledProfile, RecordProfile};

pub const PRESET_SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct PresetFile {
    schema_version: u32,
    profiles: Vec<RecordProfile>,
}

pub fn load_presets(path: &Path) -> Result<Vec<RecordProfile>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    let stored: PresetFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    if stored.schema_version != PRESET_SCHEMA_VERSION {
        return Err(format!(
            "unsupported record preset schema {} in {}",
            stored.schema_version,
            path.display()
        ));
    }
    let mut seen_ids = HashSet::new();
    let mut valid = Vec::with_capacity(stored.profiles.len());
    let mut discarded = 0usize;
    for profile in stored.profiles {
        let valid_profile = seen_ids.insert(profile.id.clone())
            && CompiledProfile::compile(profile.clone()).is_ok();
        if valid_profile {
            valid.push(profile);
        } else {
            discarded += 1;
        }
    }
    if discarded > 0 {
        log::warn!(
            "discarding {discarded} invalid saved log format(s) from {}",
            path.display()
        );
        if let Err(error) = save_presets(path, &valid) {
            log::warn!(
                "could not repair saved log formats at {}: {error}",
                path.display()
            );
        }
    }
    Ok(valid)
}

pub fn save_presets(path: &Path, profiles: &[RecordProfile]) -> Result<(), String> {
    validate(profiles)?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid preset path {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(&PresetFile {
        schema_version: PRESET_SCHEMA_VERSION,
        profiles: profiles.to_vec(),
    })
    .map_err(|error| format!("failed to encode presets: {error}"))?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("failed to write {}: {error}", temporary.display()))?;
    if let Err(error) = fs::rename(&temporary, path) {
        #[cfg(windows)]
        if path.exists() {
            // Windows rename does not replace an existing file. Move the old
            // preset list to an exact backup and restore it if installation
            // fails; never delete the user's only copy on a failed save.
            let backup = path.with_extension(format!("json.bak-{}", std::process::id()));
            if backup.exists() {
                let _ = fs::remove_file(&temporary);
                return Err(format!(
                    "preset backup already exists: {}",
                    backup.display()
                ));
            }
            fs::rename(path, &backup)
                .map_err(|failure| format!("failed to back up {}: {failure}", path.display()))?;
            if let Err(failure) = fs::rename(&temporary, path) {
                let _ = fs::rename(&backup, path);
                let _ = fs::remove_file(&temporary);
                return Err(format!("failed to replace {}: {failure}", path.display()));
            }
            let _ = fs::remove_file(&backup);
            return Ok(());
        }
        let _ = fs::remove_file(&temporary);
        return Err(format!("failed to replace {}: {error}", path.display()));
    }
    Ok(())
}

fn validate(profiles: &[RecordProfile]) -> Result<(), String> {
    let mut ids = HashSet::new();
    for profile in profiles {
        if !ids.insert(&profile.id) {
            return Err(format!("duplicate record profile ID {:?}", profile.id));
        }
        CompiledProfile::compile(profile.clone())
            .map_err(|error| format!("invalid record profile {:?}: {error}", profile.name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_round_trip_and_reject_bad_edits_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        assert!(load_presets(&path).unwrap().is_empty());
        let good = RecordProfile::text("test:one", "One", "{time} {log}");
        save_presets(&path, &[good.clone()]).unwrap();
        assert_eq!(load_presets(&path).unwrap(), vec![good.clone()]);

        let bad = RecordProfile::text("test:one", "Broken", "{unknown} {log}");
        assert!(save_presets(&path, &[bad]).is_err());
        assert_eq!(load_presets(&path).unwrap(), vec![good]);
    }

    #[test]
    fn loading_discards_and_repairs_invalid_unshipped_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        let mut invalid = RecordProfile::text("old", "Old", "{time} {log}");
        invalid.schema_version = 1;
        let valid = RecordProfile::text("current", "Current", "{time} {log}");
        fs::write(
            &path,
            serde_json::to_vec(&PresetFile {
                schema_version: PRESET_SCHEMA_VERSION,
                profiles: vec![invalid, valid.clone()],
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(load_presets(&path).unwrap(), vec![valid]);
        assert_eq!(load_presets(&path).unwrap().len(), 1);
    }
}
