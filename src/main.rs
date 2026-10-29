mod format;
mod kindle;
mod plan;
mod state;
mod sync;
mod tree;

use std::collections::BTreeSet;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{ArgAction, Parser};
use console::style;
use gio::prelude::*;
use indicatif::{ProgressBar, ProgressStyle};

use crate::kindle::Kindle;
use crate::plan::{Plan, Reason};
use crate::state::{Record, State};
use crate::sync::Job;

/// The computer is always the master copy; the Kindle folders are made identical to it.
struct Library {
    label: &'static str,
    /// Relative to the home directory.
    source: &'static str,
    /// Relative to the Kindle's `documents` directory.
    target: &'static str,
}

const LIBRARIES: [Library; 2] = [
    Library {
        label: "Magyar",
        source: "Könyvek/Könyvtár/Magyar",
        target: "hungarian",
    },
    Library {
        label: "Angol",
        source: "Könyvek/Könyvtár/Angol",
        target: "english",
    },
];

const LIST_LIMIT: usize = 20;

#[derive(Parser)]
#[command(
    name = "kindle-updater",
    version,
    about = "A gépen lévő könyvtárakat (Magyar, Angol) tükrözi a Kindle-re.\n\
             A gép a mester: ami ott nincs meg, az a Kindle-ről is törlődik. \
             A megmaradó könyvek olvasási adatai (.sdr) megmaradnak.",
    disable_help_flag = true,
    disable_version_flag = true,
    override_usage = "kindle-updater [KAPCSOLÓK]",
    help_template = "{about}\n\nHasználat: {usage}\n\nKapcsolók:\n{options}"
)]
struct Cli {
    /// Csak megmutatja, mi történne, de semmit nem módosít
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// Nem kér megerősítést a végrehajtás előtt
    #[arg(short, long)]
    yes: bool,

    /// Minden érintett fájlt kilistáz (alapból kategóriánként legfeljebb 20-at)
    #[arg(short, long)]
    verbose: bool,

    /// A Kindle gyökérkönyvtára vagy documents mappája (alapból automatikusan megkeresi)
    #[arg(long, value_name = "ÚTVONAL")]
    kindle: Option<PathBuf>,

    /// Súgó megjelenítése
    #[arg(short, long, action = ArgAction::Help)]
    help: Option<bool>,

    /// Verziószám megjelenítése
    #[arg(short = 'V', long, action = ArgAction::Version)]
    version: Option<bool>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{} {err:#}", style("Hiba:").red().bold());
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<ExitCode> {
    let home = dirs::home_dir().context("nem találom a saját könyvtáradat")?;
    let kindle = kindle::locate(cli.kindle.as_deref())?;
    println!("{} {}", style("Kindle:").bold(), kindle.name);

    let mut state = State::load().unwrap_or_else(|err| {
        warn(&format!(
            "az állapotfájl nem olvasható, csak méret alapján hasonlítok: {err:#}"
        ));
        State::default()
    });

    let jobs = LIBRARIES
        .iter()
        .map(|library| prepare(library, &home, &kindle, &state))
        .collect::<Result<Vec<_>>>()?;

    for job in &jobs {
        print_plan(job, &home, cli.verbose);
    }

    if jobs.iter().all(|job| job.plan.is_empty()) {
        println!("\n{}", style("Minden naprakész, nincs teendő.").green());
        if !cli.dry_run {
            for job in &jobs {
                record_state(&mut state, job, &BTreeSet::new());
            }
            save_state(&state);
        }
        return Ok(ExitCode::SUCCESS);
    }

    let upload_bytes: u64 = jobs.iter().map(|job| job.plan.upload_bytes()).sum();
    let freed_bytes: u64 = jobs.iter().map(|job| job.plan.freed_bytes()).sum();
    let free = free_space(&kindle);
    print_totals(&jobs, upload_bytes, free);
    if let Some(free) = free
        && upload_bytes > free + freed_bytes
    {
        bail!(
            "nincs elég hely a Kindle-ön: {} kellene, a törlések után is csak {} lesz szabad",
            format::size(upload_bytes),
            format::size(free + freed_bytes)
        );
    }

    if cli.dry_run {
        println!("{}", style("Próbafuttatás: nem módosítottam semmit.").dim());
        return Ok(ExitCode::SUCCESS);
    }
    if !cli.yes && !confirm()? {
        println!("Megszakítva, nem módosítottam semmit.");
        return Ok(ExitCode::SUCCESS);
    }

    println!();
    let started = Instant::now();
    let report = sync::run(&jobs);
    for (job, failed) in jobs.iter().zip(&report.failed_uploads) {
        record_state(&mut state, job, failed);
    }
    save_state(&state);

    println!(
        "\n{} {} feltöltve ({}), {} törölve, {} alatt.",
        style("Kész:").bold(),
        report.uploaded,
        format::size(report.uploaded_bytes),
        report.deleted,
        duration(started.elapsed())
    );
    if report.errors.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!(
        "{}",
        style(format!("{} hiba történt:", report.errors.len()))
            .red()
            .bold()
    );
    for err in &report.errors {
        eprintln!("  {err}");
    }
    eprintln!("Futtasd újra a programot, a hiányzó lépéseket akkor pótolja.");
    Ok(ExitCode::FAILURE)
}

fn prepare(library: &'static Library, home: &Path, kindle: &Kindle, state: &State) -> Result<Job> {
    let source_path = home.join(library.source);
    if !source_path.is_dir() {
        bail!("a forrásmappa nem létezik: {}", source_path.display());
    }
    let source = gio::File::for_path(&source_path);
    let target = kindle.documents.child(library.target);

    let spinner = ProgressBar::new_spinner()
        .with_style(ProgressStyle::with_template("{spinner} {msg}").expect("valid template"));
    spinner.enable_steady_tick(Duration::from_millis(100));
    spinner.set_message(format!("Beolvasás: {}", tilde(&source_path, home)));
    let source_tree = tree::scan(&source, false)?;
    spinner.set_message(format!("Beolvasás: Kindle documents/{}", library.target));
    let target_tree = tree::scan(&target, true)?;
    spinner.finish_and_clear();

    if source_tree.files.is_empty() && !target_tree.files.is_empty() {
        bail!(
            "a forrásmappa üres ({}), így a Kindle documents/{} mappájából mind a {} fájl \
             törlődne. A biztonság kedvéért leállok.",
            tilde(&source_path, home),
            library.target,
            target_tree.files.len()
        );
    }

    let state_key = target.uri().to_string();
    let plan = plan::build(&source_tree, &target_tree, &state.folder(&state_key));
    Ok(Job {
        label: library.label,
        target_name: library.target,
        source,
        target,
        state_key,
        source_tree,
        plan,
    })
}

fn print_plan(job: &Job, home: &Path, verbose: bool) {
    let plan: &Plan = &job.plan;
    let source = job
        .source
        .path()
        .map(|p| tilde(&p, home))
        .unwrap_or_default();
    println!(
        "\n{}  {} → Kindle documents/{}",
        style(job.label).bold().underlined(),
        source,
        job.target_name
    );
    if plan.is_empty() {
        println!(
            "  {} naprakész ({} fájl, {})",
            style("=").green(),
            plan.unchanged,
            format::size(job.source_tree.total_size())
        );
        return;
    }

    let new: Vec<_> = plan.new_uploads().collect();
    if !new.is_empty() {
        let bytes = new.iter().map(|u| u.size).sum();
        heading(
            style("+").green(),
            format!("{} új ({})", new.len(), format::size(bytes)),
        );
        list(
            new.iter()
                .map(|u| format!("{} ({})", u.rel, format::size(u.size))),
            verbose,
        );
    }
    let updates: Vec<_> = plan.updates().collect();
    if !updates.is_empty() {
        heading(
            style("~").yellow(),
            format!("{} frissítendő", updates.len()),
        );
        list(
            updates.iter().map(|u| match u.reason {
                Reason::Modified => format!("{} (módosult a gépen)", u.rel),
                _ => format!(
                    "{} ({} → {})",
                    u.rel,
                    format::size(u.old_size.unwrap_or(0)),
                    format::size(u.size)
                ),
            }),
            verbose,
        );
    }
    if !plan.delete_files.is_empty() {
        heading(
            style("−").red(),
            format!("{} törlendő", plan.delete_files.len()),
        );
        list(
            plan.delete_files.iter().map(|(rel, _)| rel.clone()),
            verbose,
        );
    }
    if !plan.delete_dirs.is_empty() {
        heading(
            style("−").red(),
            format!(
                "{} törlendő mappa, a tartalmával együtt",
                plan.delete_dirs.len()
            ),
        );
        list(
            plan.delete_dirs.iter().map(|rel| format!("{rel}/")),
            verbose,
        );
    }
    if !plan.delete_sidecars.is_empty() {
        heading(
            style("−").red(),
            format!(
                "{} törölt könyv olvasási adatai (.sdr)",
                plan.delete_sidecars.len()
            ),
        );
        list(plan.delete_sidecars.iter().cloned(), verbose);
    }
    if !plan.create_dirs.is_empty() {
        heading(
            style("+").green(),
            format!("{} új mappa", plan.create_dirs.len()),
        );
        list(
            plan.create_dirs.iter().map(|rel| format!("{rel}/")),
            verbose,
        );
    }
    if plan.unchanged > 0 {
        heading(style("=").dim(), format!("{} változatlan", plan.unchanged));
    }
}

fn heading(sign: console::StyledObject<&str>, text: String) {
    println!("  {sign} {text}");
}

fn list(items: impl Iterator<Item = String>, verbose: bool) {
    let items: Vec<String> = items.collect();
    let shown = if verbose {
        items.len()
    } else {
        items.len().min(LIST_LIMIT)
    };
    for item in &items[..shown] {
        println!("      {item}");
    }
    if shown < items.len() {
        println!(
            "      {}",
            style(format!(
                "… és még {} (mind: --verbose)",
                items.len() - shown
            ))
            .dim()
        );
    }
}

fn print_totals(jobs: &[Job], upload_bytes: u64, free: Option<u64>) {
    let uploads: usize = jobs.iter().map(|job| job.plan.uploads.len()).sum();
    let deletions: usize = jobs
        .iter()
        .map(|job| {
            let plan = &job.plan;
            plan.delete_files.len() + plan.delete_dirs.len() + plan.delete_sidecars.len()
        })
        .sum();
    let free = free
        .map(|bytes| format!(" Szabad hely a Kindle-ön: {}.", format::size(bytes)))
        .unwrap_or_default();
    println!(
        "\n{} {uploads} feltöltés ({}), {deletions} törlés.{free}",
        style("Összesen:").bold(),
        format::size(upload_bytes)
    );
}

fn free_space(kindle: &Kindle) -> Option<u64> {
    let info = kindle
        .documents
        .query_filesystem_info("filesystem::free", gio::Cancellable::NONE)
        .ok()?;
    info.has_attribute("filesystem::free")
        .then(|| info.attribute_uint64("filesystem::free"))
}

fn confirm() -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("nem interaktív futtatás: a --yes kapcsolóval hagyhatod jóvá a változtatásokat");
    }
    print!("Végrehajtsam? [i/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_lowercase().as_str(),
        "i" | "igen" | "y" | "yes"
    ))
}

/// Records the source version of every file now on the Kindle. Failed uploads keep
/// their previous record so that the next run retries them.
fn record_state(state: &mut State, job: &Job, failed: &BTreeSet<String>) {
    let previous = state.folder(&job.state_key);
    let folder = job
        .source_tree
        .files
        .iter()
        .filter_map(|(rel, entry)| {
            let record = if failed.contains(rel) {
                *previous.get(rel)?
            } else {
                Record::from(*entry)
            };
            Some((rel.clone(), record))
        })
        .collect();
    state.set_folder(&job.state_key, folder);
}

fn save_state(state: &State) {
    if let Err(err) = state.save() {
        warn(&format!("az állapotfájlt nem sikerült menteni: {err:#}"));
    }
}

fn warn(message: &str) {
    eprintln!("{} {message}", style("Figyelem:").yellow().bold());
}

fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn duration(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs} mp")
    } else {
        format!("{} perc {} mp", secs / 60, secs % 60)
    }
}
