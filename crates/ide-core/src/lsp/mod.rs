//! Language servers (the Language Server Protocol): a program a module names - `clangd` for C and C++ - that knows a
//! language as well as its compiler and answers what the editor asks: where a name is defined, where it is used, how
//! to rename it, what may follow the caret, what is wrong in the text.
//!
//! Only the part of the protocol the IDE uses, with nothing but `serde_json`: [`rpc`] is the wire format, [`client`]
//! the process and its threads, [`session`] the conversation (start, open documents, requests, the end), [`proto`]
//! reads the answers and [`pos`] / [`uri`] turn the server's positions and file names into the editor's.

pub mod client;
pub mod install;
pub mod pos;
pub mod proto;
pub mod rpc;
pub mod session;
pub mod uri;

pub use client::{Client, Event, ResponseError, Wake};
pub use pos::{Encoding, LineMap, Position, Range};
pub use session::{Capabilities, Output, Session, State};
