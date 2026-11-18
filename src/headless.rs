use crate::download::{Queue, Shared};
use std::{
    collections::HashMap,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

pub const HELP: &str = "Usage: relay --headless <command>

  list                           Show saved direct downloads and IDs
  add URL [DIRECTORY]            Add and download a file
  resume ID [URL]                Resume a transfer, optionally with a fresh link
  run                            Resume all paused transfers and exit when idle
  torrents [SOURCE]              Run qBittorrent-nox with Relay's torrent profile

Ctrl+C saves any resumable progress before exiting.";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop(_: std::ffi::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

extern "C" {
    fn signal(number: std::ffi::c_int, handler: extern "C" fn(std::ffi::c_int)) -> usize;
}

pub fn torrents(state: &Path, source: Option<&str>) -> std::io::Error {
    let mut command = Command::new("qbittorrent-nox");
    command.arg(format!(
        "--profile={}",
        state.join("torrent-profile").display()
    ));
    if let Some(source) = source {
        if source.starts_with('-') {
            return std::io::Error::other("Enter a magnet link, torrent URL, or .torrent path.");
        }
        command.arg(source);
    }
    let error = command.exec();
    if error.kind() == std::io::ErrorKind::NotFound {
        std::io::Error::other("Install qbittorrent-nox to use headless torrents.")
    } else {
        error
    }
}

pub fn run(
    queue: &Shared,
    args: &[String],
    default_folder: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let tracked = {
        let mut queue = queue.lock().unwrap();
        match args.as_slice() {
            ["list"] => {
                for job in queue.jobs.iter().rev() {
                    println!(
                        "{}\t{}\t{} bytes\t{}",
                        job.id,
                        job.status,
                        job.received,
                        job.destination.display()
                    );
                    if !job.error.is_empty() {
                        println!("  {}", job.error);
                    }
                }
                return Ok(());
            }
            ["add", url] | ["add", url, _] => {
                let folder = args.get(2).map_or(default_folder.as_path(), Path::new);
                queue.add(url, folder)?;
                vec![queue.jobs.last().unwrap().id.clone()]
            }
            ["resume", id] | ["resume", id, _] => {
                let url = args.get(2).copied().unwrap_or_default();
                let action = if url.is_empty() {
                    "Resume"
                } else {
                    "Update link"
                };
                queue.action(id, action, url)?;
                vec![id.to_string()]
            }
            ["run"] => {
                let paused: Vec<_> = queue
                    .jobs
                    .iter()
                    .filter(|job| job.status == "Paused")
                    .map(|job| job.id.clone())
                    .collect();
                for id in &paused {
                    queue.action(id, "Resume", "")?;
                }
                paused
            }
            _ => return Err(HELP.into()),
        }
    };
    if tracked.is_empty() {
        println!("No paused transfers.");
        return Ok(());
    }
    unsafe {
        signal(2, stop);
        signal(15, stop);
    }
    let mut statuses = HashMap::new();
    loop {
        let stopping = STOP.load(Ordering::Relaxed);
        if stopping {
            queue.lock().unwrap().close();
        } else {
            Queue::dispatch(queue);
        }
        let queue = queue.lock().unwrap();
        let jobs: Vec<_> = queue
            .jobs
            .iter()
            .filter(|job| tracked.contains(&job.id))
            .collect();
        for job in &jobs {
            if statuses.get(&job.id) != Some(&job.status) {
                println!(
                    "{}  {}  {} bytes  {}",
                    job.id,
                    job.status,
                    job.received,
                    job.destination.display()
                );
                if !job.error.is_empty() {
                    eprintln!("{}", job.error);
                }
                statuses.insert(job.id.clone(), job.status.clone());
            }
        }
        if jobs
            .iter()
            .all(|job| !matches!(job.status.as_str(), "Queued" | "Downloading" | "Pausing"))
        {
            if stopping {
                return Err("Interrupted. Saved progress can be resumed.".into());
            }
            if jobs.iter().any(|job| job.status == "Failed") {
                return Err("One or more transfers failed.".into());
            }
            return Ok(());
        }
        drop(queue);
        thread::sleep(Duration::from_millis(150));
    }
}
