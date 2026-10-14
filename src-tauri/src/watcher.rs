//! Directory watching — keeps the filmstrip honest when the folder changes
//! behind the app's back.
//!
//! Scope is deliberately narrow: watch **one** directory, non-recursively, and
//! tell the frontend *that* something changed. It never reports what changed —
//! the frontend re-lists the directory, which is both simpler and impossible to
//! desynchronise (a missed or mis-ordered event cannot leave a stale entry on
//! screen, because nothing is ever applied incrementally).
//!
//! Events are coalesced: exporting a batch of photos is dozens of writes, and
//! the UI only wants one refresh at the end of it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter};

use crate::imaging;

/// Event the frontend subscribes to. Payload: `{ dir: String }`.
pub const EVENT: &str = "directory-changed";

/// Quiet period a burst of changes must observe before it is reported.
const DEBOUNCE: Duration = Duration::from_millis(280);

/// How often a parked watch re-checks whether it has been replaced.
const STOP_POLL: Duration = Duration::from_millis(50);

/// Owns the live watch. Dropping the `RecommendedWatcher` stops the OS watch,
/// so replacing the state replaces the watch.
#[derive(Default)]
pub struct WatcherState(Mutex<Option<Active>>);

struct Active {
    /// The directory exactly as the frontend named it, so the emitted payload
    /// compares equal to `DirectoryListing.dir` without any path round-trip.
    dir: String,
    /// Held to keep the OS watch alive; dropped with the state.
    _watcher: RecommendedWatcher,
    /// Signals the debounce thread to exit.
    stop: Sender<()>,
}

impl WatcherState {
    /// Start watching `dir`, replacing whatever was watched before.
    ///
    /// Re-watching the directory already being watched is a no-op, so the
    /// frontend may call this on every open without churning OS handles.
    pub fn watch(&self, app: &AppHandle, dir: &str) -> Result<(), String> {
        let mut slot = self.0.lock().map_err(|_| "watcher state poisoned".to_string())?;
        if slot.as_ref().is_some_and(|active| active.dir == dir) {
            return Ok(());
        }
        shutdown(slot.take());

        let (raw_tx, raw_rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher = notify::recommended_watcher(move |res| {
            // A closed receiver only means this watch was replaced; ignore it.
            let _ = raw_tx.send(res);
        })
        .map_err(|e| e.to_string())?;
        watcher
            .watch(Path::new(dir), RecursiveMode::NonRecursive)
            .map_err(|e| e.to_string())?;

        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let app = app.clone();
        let watched = dir.to_string();
        thread::spawn(move || {
            debounce_loop(watched, raw_rx, stop_rx, move |dir| {
                app.emit(EVENT, Payload { dir: dir.to_string() }).is_ok()
            })
        });

        *slot = Some(Active {
            dir: dir.to_string(),
            _watcher: watcher,
            stop: stop_tx,
        });
        Ok(())
    }

    /// Stop watching. Safe to call when nothing is watched.
    pub fn unwatch(&self) -> Result<(), String> {
        let mut slot = self.0.lock().map_err(|_| "watcher state poisoned".to_string())?;
        shutdown(slot.take());
        Ok(())
    }
}

/// Tell the debounce thread to stop and drop the OS watch.
fn shutdown(active: Option<Active>) {
    if let Some(active) = active {
        let _ = active.stop.send(());
    }
}

/// Collapse a burst of filesystem events into a single notification.
///
/// The thread parks on the channel: any *relevant* event pushes the deadline
/// out by [`DEBOUNCE`], and only once the channel has been quiet that long does
/// it report. That is what turns "this folder just gained 200 files" into one
/// refresh. Irrelevant traffic does not extend the deadline, so a folder with a
/// constantly-appended log file still reports its image changes.
///
/// `report` is handed the watched directory and returns whether the watch
/// should continue — it is a closure rather than an `AppHandle` so the
/// coalescing rules can be tested without a running Tauri app.
fn debounce_loop(
    dir: String,
    rx: mpsc::Receiver<notify::Result<Event>>,
    stop: mpsc::Receiver<()>,
    report: impl Fn(&str) -> bool,
) {
    let watched = PathBuf::from(&dir);
    loop {
        if !wait_for_change(&rx, &stop, &watched) {
            return;
        }

        let mut deadline = Instant::now() + DEBOUNCE;
        loop {
            if stopped(&stop) {
                return;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match rx.recv_timeout(deadline - now) {
                Ok(Ok(event)) => {
                    if is_relevant(&event, &watched) {
                        deadline = Instant::now() + DEBOUNCE;
                    }
                }
                Ok(Err(_)) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }

        if !report(&dir) {
            return;
        }
    }
}

/// Block until a relevant event arrives. Returns `false` once the watch is done.
///
/// The stop channel is polled alongside the events so a watch on a quiet
/// directory still shuts down promptly when it is replaced.
fn wait_for_change(
    rx: &mpsc::Receiver<notify::Result<Event>>,
    stop: &mpsc::Receiver<()>,
    watched: &Path,
) -> bool {
    loop {
        if stopped(stop) {
            return false;
        }
        match rx.recv_timeout(STOP_POLL) {
            Ok(Ok(event)) if is_relevant(&event, watched) => return true,
            Ok(_) => continue,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

/// True once the watch has been replaced or dropped.
fn stopped(stop: &mpsc::Receiver<()>) -> bool {
    !matches!(stop.try_recv(), Err(TryRecvError::Empty))
}

/// Only images matter, and only when something was created, removed or written
/// — `Access` events fire on every read and would keep the thread spinning.
///
/// The watched directory itself also counts: the frontend needs to hear about
/// the folder being renamed or deleted out from under it.
fn is_relevant(event: &Event, watched: &Path) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    // A rename reports both endpoints, so `a.jpg` becoming `a.txt` still counts.
    event
        .paths
        .iter()
        .any(|p| p == watched || imaging::is_supported(p))
}

/// Serialised payload of [`EVENT`].
#[derive(Clone, serde::Serialize)]
struct Payload {
    dir: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, ModifyKind, RemoveKind, RenameMode};

    fn event(kind: EventKind, paths: &[&str]) -> Event {
        Event {
            kind,
            paths: paths.iter().map(PathBuf::from).collect(),
            attrs: Default::default(),
        }
    }

    fn watched() -> PathBuf {
        PathBuf::from("/photos")
    }

    #[test]
    fn image_changes_are_relevant() {
        let w = watched();
        assert!(is_relevant(
            &event(EventKind::Create(CreateKind::File), &["/photos/a.JPG"]),
            &w
        ));
        assert!(is_relevant(
            &event(EventKind::Remove(RemoveKind::File), &["/photos/a.png"]),
            &w
        ));
    }

    #[test]
    fn a_rename_counts_when_either_end_is_an_image() {
        let w = watched();
        // `.jpg` -> `.txt` is a removal as far as the filmstrip is concerned.
        assert!(is_relevant(
            &event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &["/photos/a.jpg", "/photos/a.txt"]
            ),
            &w
        ));
    }

    #[test]
    fn other_files_and_reads_are_ignored() {
        let w = watched();
        assert!(!is_relevant(
            &event(EventKind::Create(CreateKind::File), &["/photos/notes.txt"]),
            &w
        ));
        // `Access` fires on every read; honouring it would spin the thread.
        assert!(!is_relevant(
            &event(EventKind::Access(AccessKind::Read), &["/photos/a.png"]),
            &w
        ));
    }

    #[test]
    fn the_watched_folder_itself_is_relevant() {
        let w = watched();
        assert!(is_relevant(
            &event(EventKind::Remove(RemoveKind::Folder), &["/photos"]),
            &w
        ));
    }

    /// Spin up the real debounce loop and feed it a burst of events.
    fn spawn_loop() -> (
        mpsc::Sender<notify::Result<Event>>,
        mpsc::Receiver<String>,
        mpsc::Sender<()>,
        thread::JoinHandle<()>,
    ) {
        let (raw_tx, raw_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            debounce_loop("/photos".to_string(), raw_rx, stop_rx, move |dir| {
                report_tx.send(dir.to_string()).is_ok()
            });
        });
        (raw_tx, report_rx, stop_tx, handle)
    }

    #[test]
    fn a_burst_collapses_into_one_report() {
        let (raw_tx, report_rx, stop_tx, handle) = spawn_loop();

        // Three writes, each well inside the previous one's quiet window.
        for name in ["a.jpg", "b.jpg", "c.jpg"] {
            let path = format!("/photos/{name}");
            raw_tx
                .send(Ok(event(EventKind::Create(CreateKind::File), &[&path])))
                .unwrap();
            thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(
            report_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "/photos"
        );
        // ...and nothing more: the burst produced exactly one refresh.
        assert!(report_rx.recv_timeout(DEBOUNCE * 2).is_err());

        drop(stop_tx);
        handle.join().unwrap();
    }

    #[test]
    fn irrelevant_traffic_does_not_report() {
        let (raw_tx, report_rx, stop_tx, handle) = spawn_loop();

        for name in ["notes.txt", ".DS_Store"] {
            let path = format!("/photos/{name}");
            raw_tx
                .send(Ok(event(EventKind::Create(CreateKind::File), &[&path])))
                .unwrap();
        }
        assert!(report_rx.recv_timeout(DEBOUNCE * 2).is_err());

        drop(stop_tx);
        handle.join().unwrap();
    }

    /// End-to-end on a real directory: the OS backend must actually deliver the
    /// event and the loop must report it. This is the part that cannot be
    /// covered by feeding synthetic events into the channel.
    #[test]
    fn a_real_filesystem_change_is_reported() {
        let dir = std::env::temp_dir().join(format!("lumina-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir_str = dir.to_string_lossy().into_owned();

        let (raw_tx, raw_rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = raw_tx.send(res);
        })
        .unwrap();
        watcher.watch(&dir, RecursiveMode::NonRecursive).unwrap();

        let (stop_tx, stop_rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        let watched = dir_str.clone();
        let handle = thread::spawn(move || {
            debounce_loop(watched, raw_rx, stop_rx, move |d| {
                report_tx.send(d.to_string()).is_ok()
            });
        });

        std::fs::write(dir.join("photo.png"), b"content is irrelevant here").unwrap();

        let reported = report_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("no change reported for a real filesystem write");
        assert_eq!(reported, dir_str);

        drop(stop_tx);
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
