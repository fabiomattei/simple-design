//! The CLI side of the socket: connect to a running instance (if any) and
//! send one request per connection.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use super::protocol::{Request, Response};
use super::registry;

/// Connects to the running GUI instance for `doc_path`, if one is listening.
pub fn connect(doc_path: &Path) -> Option<UnixStream> {
    registry::connect_live(doc_path)
}

pub fn send(stream: &UnixStream, op: &str, args: serde_json::Value) -> anyhow::Result<Response> {
    let request = Request { id: 1, op: op.to_string(), args };
    let mut writer = stream.try_clone()?;
    writeln!(writer, "{}", serde_json::to_string(&request)?)?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}
