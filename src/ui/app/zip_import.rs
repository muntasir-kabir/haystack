use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{atomic::Ordering, Arc};

use crossbeam_channel::{Receiver, TryRecvError};
use eframe::egui;

use crate::ui::icons::{self, Icon};

use super::archive::{extract_zip, ExtractionProgress};
use super::model::HaystackApp;

struct ExtractionJob {
    path: PathBuf,
    progress: Arc<ExtractionProgress>,
    result: Receiver<Result<Option<PathBuf>, String>>,
}

#[derive(Default)]
pub(super) struct ZipImports {
    pending: VecDeque<PathBuf>,
    active: Option<ExtractionJob>,
    pub(super) error: Option<String>,
}

impl Drop for ZipImports {
    fn drop(&mut self) {
        if let Some(job) = &self.active {
            job.progress.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl ZipImports {
    pub(super) fn has_interactive_surface(&self) -> bool {
        self.active.is_some() || self.error.is_some()
    }

    pub fn enqueue(&mut self, path: PathBuf) {
        if !self.pending.contains(&path)
            && !self.active.as_ref().is_some_and(|job| job.path == path)
        {
            self.pending.push_back(path);
        }
    }
}

impl HaystackApp {
    pub(super) fn zip_import_ui(&mut self, ctx: &egui::Context) {
        // Serialize archives and dialogs, even when several ZIPs are dropped together.
        if self.zip_imports.active.is_none() && self.zip_imports.error.is_none() {
            if let Some(path) = self.zip_imports.pending.pop_front() {
                let progress = Arc::new(ExtractionProgress::default());
                let (tx, result) = crossbeam_channel::bounded(1);
                let worker_path = path.clone();
                let worker_progress = progress.clone();
                crate::ui::worker_pool::spawn(move || {
                    let _ = tx.send(extract_zip(&worker_path, &worker_progress));
                });
                self.zip_imports.active = Some(ExtractionJob {
                    path,
                    progress,
                    result,
                });
            }
        }
        let result = self
            .zip_imports
            .active
            .as_ref()
            .and_then(|job| match job.result.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(
                    "ZIP extraction stopped unexpectedly. Please try again.".into(),
                )),
            });
        if let Some(result) = result {
            let job = self.zip_imports.active.take().unwrap();
            match result {
                Ok(Some(folder)) => {
                    self.status = format!("Extracted to {}", folder.display());
                    if let Some(paths) = rfd::FileDialog::new()
                        .set_title("Choose log files from the extracted ZIP")
                        .set_directory(&folder)
                        .pick_files()
                    {
                        for path in paths {
                            self.open_file(path);
                        }
                    }
                }
                Ok(None) => self.status = "ZIP extraction canceled.".into(),
                Err(error) => {
                    self.zip_imports.error = Some(format!("{}\n\n{error}", job.path.display()))
                }
            }
            ctx.request_repaint();
        }
        if let Some(job) = &self.zip_imports.active {
            super::overlay::modal(
                ctx,
                "zip_progress_modal",
                "Extracting ZIP",
                egui::vec2(480.0, 200.0),
                |ui| {
                    ui.label(job.path.file_name().unwrap_or_default().to_string_lossy());
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!(
                            "{} / {} entries extracted",
                            job.progress.completed.load(Ordering::Relaxed),
                            job.progress.total.load(Ordering::Relaxed)
                        ));
                    });
                    ui.label("When ready, choose which log files to open.");
                    if !self.zip_imports.pending.is_empty() {
                        ui.label(format!(
                            "{} more ZIPs queued",
                            self.zip_imports.pending.len()
                        ));
                    }
                    let canceling = job.progress.cancel.load(Ordering::Relaxed);
                    if icons::action_button_enabled(
                        ui,
                        !canceling,
                        Icon::Close,
                        if canceling { "Canceling…" } else { "Cancel" },
                        self.theme.text,
                        if canceling {
                            "Waiting for ZIP extraction to stop"
                        } else {
                            "Stop extracting this ZIP"
                        },
                    )
                    .clicked()
                    {
                        job.progress.cancel.store(true, Ordering::Relaxed);
                    }
                },
            );
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        }
        if let Some(error) = self.zip_imports.error.clone() {
            super::overlay::modal(
                ctx,
                "zip_error_modal",
                "Could not extract ZIP",
                egui::vec2(520.0, 220.0),
                |ui| {
                    ui.label(error);
                    ui.label(
                        "The archive may be damaged or use an unsupported compression method.",
                    );
                    ui.separator();
                    if icons::primary_action_button(
                        ui,
                        Icon::Close,
                        "Close",
                        self.theme.accent,
                        "Dismiss this error",
                    )
                    .clicked()
                    {
                        self.zip_imports.error = None;
                    }
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queues_archives_in_order_and_ignores_duplicates_in_flight() {
        let mut imports = ZipImports::default();
        imports.enqueue("first.zip".into());
        imports.enqueue("second.zip".into());
        imports.enqueue("first.zip".into());
        let (_tx, result) = crossbeam_channel::bounded(1);
        imports.active = Some(ExtractionJob {
            path: imports.pending.pop_front().unwrap(),
            progress: Arc::new(ExtractionProgress::default()),
            result,
        });
        imports.enqueue("first.zip".into());
        assert_eq!(
            imports.pending,
            VecDeque::from([PathBuf::from("second.zip")])
        );
        let progress = imports.active.as_ref().unwrap().progress.clone();
        drop(imports);
        assert!(progress.cancel.load(Ordering::Relaxed));
    }
}
