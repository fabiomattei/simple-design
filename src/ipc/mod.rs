//! CLI ↔ GUI interop: a Unix-socket protocol keyed by the open document's
//! file path, so `simple-design-cli` can drive a running GUI instance live
//! (edits land on the same in-memory `Document`, same undo history) or fall
//! back to editing the `.sdesign` file directly when no instance is open.
//!
//! - `protocol`: the wire types (`Request`/`Response`) and each op's args.
//! - `registry`: maps a document path to a running instance's socket.
//! - `server`: the GUI side — accepts connections, forwards to the app.
//! - `client`: the CLI side — connects and sends one request.
//! - `ops`: the actual command implementations, shared by both the live
//!   server (via `app`'s `History`) and the CLI's headless fallback.

pub mod client;
pub mod ops;
pub mod protocol;
pub mod registry;
pub mod server;

pub use protocol::{Request, Response};
pub use server::{IpcHandle, PendingCommand};
