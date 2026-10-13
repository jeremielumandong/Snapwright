//! Text in captures: reads every capture's text into the library so search finds it, and
//! copies the text of a region straight from the overlay (`T`).
//!
//! One worker thread owns the OCR engine and sleeps on a channel, so it costs nothing while
//! idle. Library changes wake it ([`wake`]); once things are quiet for a second it reads,
//! newest first, every capture whose shown image has not been read yet (new captures, edits,
//! and on first run the whole library), one at a time. A text grab from the overlay jumps
//! that queue.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use futures::StreamExt as _;
use futures::channel::{mpsc as async_mpsc, oneshot};
use gpui_kit::{App, ClipboardItem, Global};
use snapwright_capture::Frame;
use snapwright_library::Library;
use snapwright_ocr::{Config, Image, Order, Recognizer};

use crate::output;
use crate::session::LibraryService;

/// Quiet time after the last library change before reading starts, so recognition (which
/// uses every core for a moment on Linux) stays out of the way of a burst of captures.
const SETTLE: Duration = Duration::from_millis(1000);
/// How often an open library window reloads while a backlog is being read.
const RELOAD_EVERY: Duration = Duration::from_secs(2);
/// How long to wait before trying again to set up an engine that failed (say, offline while
/// fetching the Linux models).
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

enum Job {
    /// Read whatever the library still has unread.
    Index,
    /// Read one frame now and hand the text back.
    Read {
        frame: Frame,
        done: oneshot::Sender<anyhow::Result<String>>,
    },
}

struct OcrService(Sender<Job>);
impl Global for OcrService {}

/// Starts the worker and reads anything the library has not read yet.
pub fn init(cx: &mut App) {
    let library = cx.global::<LibraryService>().0.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let (changed_tx, mut changed) = async_mpsc::unbounded();
    let spawned = std::thread::Builder::new()
        .name("ocr".into())
        .spawn(move || Worker::new(library, changed_tx).run(rx));
    if let Err(e) = spawned {
        tracing::error!(error = %e, "text recognition unavailable: no worker thread");
        return;
    }
    cx.set_global(OcrService(tx));
    cx.spawn(async move |cx| {
        while changed.next().await.is_some() {
            cx.update(crate::library_view::reload_open);
        }
    })
    .detach();
    wake(cx);
}

/// Tells the worker the library changed, so it reads anything new.
pub fn wake(cx: &App) {
    if let Some(service) = cx.try_global::<OcrService>() {
        let _ = service.0.send(Job::Index);
    }
}

/// Reads the text in `frame` and puts it on the clipboard.
pub fn copy_text(frame: Frame, cx: &mut App) {
    let Some(service) = cx.try_global::<OcrService>() else {
        tracing::warn!("text recognition unavailable");
        return;
    };
    let (done, text) = oneshot::channel();
    if service.0.send(Job::Read { frame, done }).is_err() {
        tracing::warn!("text recognition stopped");
        return;
    }
    let started = Instant::now();
    cx.spawn(async move |cx| match text.await {
        Ok(Ok(text)) if text.is_empty() => tracing::info!("no text found in the selection"),
        Ok(Ok(text)) => {
            tracing::info!(
                chars = text.chars().count(),
                ms = started.elapsed().as_secs_f64() * 1000.0,
                "text copied"
            );
            cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text)));
        }
        Ok(Err(e)) => tracing::error!(error = %e, "reading the selection's text failed"),
        Err(_) => tracing::warn!("text recognition stopped"),
    })
    .detach();
}

/// Where the Linux engine keeps its models: beside the library, in the data folder.
fn models_dir() -> anyhow::Result<PathBuf> {
    let library = output::library_dir()?;
    let data = library.parent().context("library folder has no parent")?;
    Ok(data.join("ocr-models"))
}

struct Worker {
    library: Option<Arc<Mutex<Library>>>,
    changed: async_mpsc::UnboundedSender<()>,
    engine: Option<Recognizer>,
    failed_at: Option<Instant>,
}

impl Worker {
    fn new(library: Option<Arc<Mutex<Library>>>, changed: async_mpsc::UnboundedSender<()>) -> Self {
        Self {
            library,
            changed,
            engine: None,
            failed_at: None,
        }
    }

    fn run(mut self, jobs: Receiver<Job>) {
        while let Ok(job) = jobs.recv() {
            match job {
                Job::Read { frame, done } => self.read_now(frame, done),
                Job::Index => self.index(&jobs),
            }
        }
    }

    /// The engine, set up on first use. `None` while it cannot be set up.
    fn engine(&mut self) -> Option<&mut Recognizer> {
        if self.engine.is_none() {
            if self.failed_at.is_some_and(|t| t.elapsed() < RETRY_AFTER) {
                return None;
            }
            let started = Instant::now();
            let config = Config {
                models_dir: models_dir().unwrap_or_else(|_| PathBuf::from("ocr-models")),
            };
            match Recognizer::new(&config) {
                Ok(engine) => {
                    tracing::info!(
                        engine = snapwright_ocr::ENGINE,
                        ms = started.elapsed().as_secs_f64() * 1000.0,
                        "text recognition ready"
                    );
                    self.engine = Some(engine);
                    self.failed_at = None;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "text recognition unavailable");
                    self.failed_at = Some(Instant::now());
                }
            }
        }
        self.engine.as_mut()
    }

    fn read_now(&mut self, frame: Frame, done: oneshot::Sender<anyhow::Result<String>>) {
        let text = match self.engine() {
            Some(engine) => engine
                .recognize(&Image {
                    pixels: &frame.data,
                    width: frame.width,
                    height: frame.height,
                    order: Order::Bgra,
                })
                .map(|lines| snapwright_ocr::text_of(&lines))
                .map_err(anyhow::Error::from),
            None => Err(anyhow::anyhow!("text recognition unavailable")),
        };
        let _ = done.send(text);
    }

    /// Reads unread captures until none are left, answering text grabs in between.
    fn index(&mut self, jobs: &Receiver<Job>) {
        let Some(library) = self.library.clone() else {
            return;
        };
        loop {
            match jobs.recv_timeout(SETTLE) {
                Ok(Job::Read { frame, done }) => self.read_now(frame, done),
                Ok(Job::Index) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let mut read = 0usize;
        let mut reloaded = Instant::now();
        loop {
            // Grabs from the overlay go first; queued wake-ups are this same loop.
            loop {
                match jobs.try_recv() {
                    Ok(Job::Read { frame, done }) => self.read_now(frame, done),
                    Ok(Job::Index) => {}
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            let next = match library.lock() {
                Ok(lib) => lib.pending_text(1),
                Err(_) => return,
            };
            let job = match next {
                Ok(mut jobs) if !jobs.is_empty() => jobs.remove(0),
                Ok(_) => break,
                Err(e) => {
                    tracing::error!(error = %e, "listing captures to read failed");
                    break;
                }
            };
            let Some(engine) = self.engine() else {
                break;
            };
            let started = Instant::now();
            let text = std::fs::read(&job.image)
                .map_err(anyhow::Error::from)
                .and_then(|png| output::decode_png(&png))
                .and_then(|(width, height, rgba)| {
                    Ok(engine.recognize(&Image {
                        pixels: &rgba,
                        width,
                        height,
                        order: Order::Rgba,
                    })?)
                })
                .map(|lines| snapwright_ocr::text_of(&lines));
            // A capture that cannot be read is stored as read with no text, so it does not
            // hold up the queue; a new edit puts it back.
            let text = text.unwrap_or_else(|e| {
                tracing::warn!(id = job.id, error = %e, "reading a capture's text failed");
                String::new()
            });
            tracing::debug!(
                id = job.id,
                chars = text.len(),
                ms = started.elapsed().as_secs_f64() * 1000.0,
                "capture text read"
            );
            let stored = match library.lock() {
                Ok(mut lib) => lib.set_recognized_text(job.id, &job.hash, &text),
                Err(_) => return,
            };
            if let Err(e) = stored {
                tracing::error!(id = job.id, error = %e, "storing a capture's text failed");
                break;
            }
            read += 1;
            if reloaded.elapsed() >= RELOAD_EVERY {
                let _ = self.changed.unbounded_send(());
                reloaded = Instant::now();
            }
        }
        if read > 0 {
            tracing::info!(captures = read, "capture text indexed");
            let _ = self.changed.unbounded_send(());
        }
    }
}
