use std::collections::BTreeSet;

use console::style;
use gio::glib;
use gio::prelude::*;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};

use crate::format;
use crate::plan::{Plan, Reason};
use crate::tree::Tree;

/// One library folder to mirror onto the Kindle.
pub struct Job {
    pub label: &'static str,
    pub target_name: &'static str,
    pub source: gio::File,
    pub target: gio::File,
    pub state_key: String,
    pub source_tree: Tree,
    pub plan: Plan,
}

impl Job {
    fn display(&self, rel: &str) -> String {
        format!("{}/{rel}", self.target_name)
    }
}

#[derive(Default)]
pub struct Report {
    pub uploaded: usize,
    pub uploaded_bytes: u64,
    pub deleted: usize,
    pub errors: Vec<String>,
    /// Per job, the uploads that did not make it to the Kindle.
    pub failed_uploads: Vec<BTreeSet<String>>,
}

/// Deletes first to free up space, then uploads.
pub fn run(jobs: &[Job]) -> Report {
    let mut report = Report {
        failed_uploads: vec![BTreeSet::new(); jobs.len()],
        ..Report::default()
    };
    delete_phase(jobs, &mut report);
    upload_phase(jobs, &mut report);
    report
}

fn delete_phase(jobs: &[Job], report: &mut Report) {
    let total: usize = jobs
        .iter()
        .map(|job| {
            let plan = &job.plan;
            plan.delete_dirs.len() + plan.delete_sidecars.len() + plan.delete_files.len()
        })
        .sum();
    if total == 0 {
        return;
    }

    let bar = ProgressBar::new(total as u64).with_style(
        ProgressStyle::with_template("{prefix:.bold} [{bar:40.red/white}] {pos}/{len}  {wide_msg}")
            .expect("valid template")
            .progress_chars("=> "),
    );
    bar.set_prefix("Törlés");

    for job in jobs {
        let plan = &job.plan;
        let dirs = plan.delete_dirs.iter().chain(&plan.delete_sidecars);
        let targets = dirs
            .map(|rel| (rel, "/"))
            .chain(plan.delete_files.iter().map(|(rel, _)| (rel, "")));
        for (rel, suffix) in targets {
            let shown = format!("{}{suffix}", job.display(rel));
            bar.set_message(shown.clone());
            match delete_recursive(&job.target.resolve_relative_path(rel)) {
                Ok(()) => {
                    report.deleted += 1;
                    bar.suspend(|| println!("  {} {shown}", style("−").red()));
                }
                Err(err) => fail(&bar, report, &shown, &err.to_string()),
            }
            bar.inc(1);
        }
    }
    bar.finish_and_clear();
}

fn upload_phase(jobs: &[Job], report: &mut Report) {
    for job in jobs {
        if job.plan.uploads.is_empty() && job.plan.create_dirs.is_empty() {
            continue;
        }
        for dir in std::iter::once(job.target.clone()).chain(
            job.plan
                .create_dirs
                .iter()
                .map(|rel| job.target.resolve_relative_path(rel)),
        ) {
            if let Err(err) = make_dir(&dir) {
                report.errors.push(format!("{}: {err}", dir.uri()));
            }
        }
    }

    let count: usize = jobs.iter().map(|job| job.plan.uploads.len()).sum();
    if count == 0 {
        return;
    }
    let total: u64 = jobs.iter().map(|job| job.plan.upload_bytes()).sum();
    let bar = ProgressBar::new(total).with_style(upload_style());
    bar.set_prefix("Feltöltés");

    let mut index = 0;
    for (job_index, job) in jobs.iter().enumerate() {
        for upload in &job.plan.uploads {
            index += 1;
            let shown = job.display(&upload.rel);
            bar.set_message(format!("{index}/{count}  {shown}"));

            let base = bar.position();
            let source = job.source.resolve_relative_path(&upload.rel);
            let target = job.target.resolve_relative_path(&upload.rel);
            let result = source
                .copy(
                    &target,
                    gio::FileCopyFlags::OVERWRITE,
                    gio::Cancellable::NONE,
                    Some(&mut |done, _| bar.set_position(base + done.max(0) as u64)),
                )
                .map_err(|err| err.to_string())
                .and_then(|()| verify_size(&target, upload.size));
            bar.set_position(base + upload.size);

            match result {
                Ok(()) => {
                    report.uploaded += 1;
                    report.uploaded_bytes += upload.size;
                    let line = match upload.reason {
                        Reason::New => format!("  {} {shown}", style("+").green()),
                        _ => format!(
                            "  {} {shown} {}",
                            style("~").yellow(),
                            style("(frissítve)").dim()
                        ),
                    };
                    bar.suspend(|| println!("{line}"));
                }
                Err(err) => {
                    report.failed_uploads[job_index].insert(upload.rel.clone());
                    fail(&bar, report, &shown, &err);
                }
            }
        }
    }
    bar.finish_and_clear();
}

fn upload_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{msg}\n{prefix:.bold} [{bar:40.cyan/blue}] {done}/{total}  {rate}  hátra: {eta_precise}",
    )
    .expect("valid template")
    .progress_chars("=> ")
    .with_key(
        "done",
        |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            let _ = w.write_str(&format::size(state.pos()));
        },
    )
    .with_key(
        "total",
        |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            let _ = w.write_str(&format::size(state.len().unwrap_or(0)));
        },
    )
    .with_key(
        "rate",
        |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            let _ = write!(w, "{}/s", format::size(state.per_sec() as u64));
        },
    )
}

fn fail(bar: &ProgressBar, report: &mut Report, shown: &str, err: &str) {
    bar.suspend(|| println!("  {} {shown}: {err}", style("✗").red().bold()));
    report.errors.push(format!("{shown}: {err}"));
}

fn verify_size(target: &gio::File, expected: u64) -> Result<(), String> {
    let info = target
        .query_info(
            "standard::size",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .map_err(|err| err.to_string())?;
    let actual = info.size().max(0) as u64;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "a feltöltött fájl mérete {} a várt {} helyett",
            format::size(actual),
            format::size(expected)
        ))
    }
}

fn make_dir(dir: &gio::File) -> Result<(), glib::Error> {
    match dir.make_directory_with_parents(gio::Cancellable::NONE) {
        Err(err) if err.matches(gio::IOErrorEnum::Exists) => Ok(()),
        other => other,
    }
}

/// MTP refuses to delete non-empty directories, so empty them first.
fn delete_recursive(file: &gio::File) -> Result<(), glib::Error> {
    let kind = file.query_file_type(
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
    );
    if kind == gio::FileType::Directory {
        let children: Vec<gio::FileInfo> = file
            .enumerate_children(
                "standard::name",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            )?
            .collect::<Result<_, _>>()?;
        for child in children {
            delete_recursive(&file.child(child.name()))?;
        }
    }
    file.delete(gio::Cancellable::NONE)
}
