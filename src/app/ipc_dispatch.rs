//! Command dispatch for the IPC socket (see `crate::ipc`). Most ops are
//! plain `Document` mutations delegated to `crate::ipc::ops`, wrapped here
//! with a `History::snapshot()` so each one lands as a single undo step;
//! `undo`/`redo`/`save` are handled directly since they need `History`/a
//! file path rather than just a `Document`.

use serde_json::json;

use crate::ipc::protocol::SaveArgs;
use crate::ipc::{self, ops, PendingCommand, Response};
use crate::io;

use super::App;

impl App {
    /// Keeps the IPC socket bound to `current_path`, rebinding on change
    /// (Open / Save As) and dropping it while the document is untitled
    /// (`current_path` is `None`) — there's nothing to key a socket on yet.
    pub(super) fn sync_ipc(&mut self, ctx: &egui::Context) {
        let target = self.current_path.as_deref().and_then(|path| path.canonicalize().ok());
        let bound = self.ipc.as_ref().map(|handle| handle.doc_path().to_path_buf());
        if bound == target {
            return;
        }
        self.ipc = None; // drops the old handle first, freeing its socket file
        if let Some(path) = target {
            let ctx = ctx.clone();
            self.ipc = ipc::server::start(&path, self.ipc_tx.clone(), move || ctx.request_repaint()).ok();
        }
    }

    /// Drains and applies every command the socket thread queued since the
    /// last frame. While a socket is bound, also asks for another frame
    /// within 100ms regardless: the socket thread's own `ctx.request_repaint()`
    /// is the fast path, but isn't reliable enough on its own to wake every
    /// windowing backend's idle event loop from a background thread, so this
    /// is the guaranteed fallback that bounds a command's worst-case latency.
    pub(super) fn drain_ipc(&mut self, ctx: &egui::Context) {
        if self.ipc.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        while let Ok(PendingCommand { request, respond }) = self.ipc_rx.try_recv() {
            let id = request.id;
            let response = match self.dispatch_op(&request.op, request.args) {
                Ok(value) => Response::ok(id, value),
                Err(err) => Response::err(id, err),
            };
            let _ = respond.send(response);
        }
    }

    fn dispatch_op(&mut self, op: &str, args: serde_json::Value) -> Result<serde_json::Value, String> {
        match op {
            "undo" => {
                self.history.undo();
                Ok(json!({}))
            }
            "redo" => {
                self.history.redo();
                Ok(json!({}))
            }
            "save" => {
                let args: SaveArgs = serde_json::from_value(args).map_err(|err| format!("invalid args: {err}"))?;
                let path = match args.path {
                    Some(path) => std::path::PathBuf::from(path),
                    None => self.current_path.clone().ok_or("no path given and the document has never been saved")?,
                };
                io::save_to(&path, self.history.get()).map_err(|err| err.to_string())?;
                self.current_path = Some(path.clone());
                Ok(json!({ "path": path }))
            }
            op => {
                if ops::mutates(op) {
                    self.history.snapshot();
                }
                ops::apply(self.history.mutate(), op, args)
            }
        }
    }
}
