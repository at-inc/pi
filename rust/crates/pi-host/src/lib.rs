//! Host platform layer shared by every crate of the pi Rust port.
//!
//! The TypeScript packages reach the platform through globals: `globalThis.fetch`, `process.env`,
//! `Date.now()`, `performance.now()`, `setTimeout`, `AbortSignal`, and fire-and-forget promises. This crate
//! gives each of those one Rust counterpart so ported code reads like the TypeScript it came from, while the
//! actual I/O is supplied by whoever embeds the crates: the napi-rs binding (Node), the wasm binding (Workers,
//! browsers), the native tokio host (`native` feature), or the scripted host in tests (`testing` feature).
//!
//! Everything here is single-threaded, mirroring the JavaScript event loop: handles are `Rc`, futures are
//! `!Send`, and shared state lives in `RefCell`. See `rust/PORTING.md`.

pub mod abort;
pub mod error;
pub mod host;
pub mod http;
pub mod js;
pub mod server;
pub mod sse;
pub mod websocket;

#[cfg(feature = "native")]
pub mod native;
#[cfg(feature = "testing")]
pub mod testing;

pub use abort::{AbortController, AbortSignal};
pub use error::{PiError, PiResult};
pub use host::{
	Host, current, env, file_exists, file_not_found, home_dir, monotonic_ms, now_ms, read_file, set_host, sleep,
	spawn_local,
};
pub use http::{Body, FetchFunction, HttpRequest, HttpResponse, RedirectMode, fetch, global_fetch, is_global_fetch};

/// `futures::future::LocalBoxFuture` re-exported so ported code needs one import for async callbacks.
pub type LocalBoxFuture<'a, T> = futures::future::LocalBoxFuture<'a, T>;
/// `futures::stream::LocalBoxStream` re-exported for byte and event streams.
pub type LocalBoxStream<'a, T> = futures::stream::LocalBoxStream<'a, T>;
