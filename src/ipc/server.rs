//! The GUI side of the socket: listens for connections keyed to one open
//! document, forwards each parsed `Request` (plus a reply channel) to the
//! main thread, and writes back whatever `Response` it gets handed.
//!
//! All document mutation still happens on the egui UI thread — this just
//! hands requests across via a channel and blocks the connection's own
//! thread on the reply, so `Document` never needs to be `Send`/`Sync`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;

use super::protocol::{Request, Response};
use super::registry;

pub struct PendingCommand {
    pub request: Request,
    pub respond: Sender<Response>,
}

/// Owns the listening socket and registry entry for one open document;
/// dropping it (e.g. on "Save As" to a different file, or app exit) tears
/// both down.
pub struct IpcHandle {
    doc_path: PathBuf,
    socket_path: PathBuf,
}

impl IpcHandle {
    pub fn doc_path(&self) -> &Path {
        &self.doc_path
    }
}

impl Drop for IpcHandle {
    fn drop(&mut self) {
        registry::unregister(&self.doc_path);
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Starts listening for CLI connections against `doc_path`. Each accepted
/// request is sent down `tx`; `repaint` is called right after, so the GUI
/// wakes up and drains it on the next frame even if the window is otherwise
/// idle (egui only repaints on demand).
pub fn start(
    doc_path: &Path,
    tx: Sender<PendingCommand>,
    repaint: impl Fn() + Send + Sync + 'static,
) -> std::io::Result<IpcHandle> {
    let socket_path = registry::socket_path_for(doc_path);
    // A stale socket file from a previous instance that didn't exit
    // cleanly (e.g. killed) would otherwise make `bind` fail with "address
    // in use" even though nothing is listening on it anymore.
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    registry::register(doc_path, &socket_path);

    let repaint = Arc::new(repaint);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let tx = tx.clone();
            let repaint = Arc::clone(&repaint);
            thread::spawn(move || handle_connection(stream, tx, repaint));
        }
    });

    Ok(IpcHandle { doc_path: doc_path.to_path_buf(), socket_path })
}

fn handle_connection(stream: UnixStream, tx: Sender<PendingCommand>, repaint: Arc<dyn Fn() + Send + Sync>) {
    let Ok(reader_stream) = stream.try_clone() else { return };
    let mut writer = stream;
    let reader = BufReader::new(reader_stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(err) => {
                let _ = writeln!(writer, "{}", serde_json::to_string(&Response::err(0, format!("bad request: {err}"))).unwrap());
                continue;
            }
        };
        let id = request.id;
        let (resp_tx, resp_rx) = mpsc::channel();
        if tx.send(PendingCommand { request, respond: resp_tx }).is_err() {
            break; // the app is shutting down
        }
        repaint();
        let response = resp_rx.recv().unwrap_or_else(|_| Response::err(id, "app closed before responding"));
        if writeln!(writer, "{}", serde_json::to_string(&response).unwrap()).is_err() {
            break;
        }
    }
}
