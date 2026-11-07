//! wasm-bindgen bindings of the pi Rust core, loaded by the npm package `@at-inc/pi-core` and used by the facades
//! under `rust/npm` (see `rust/BINDINGS.md` and `rust/npm/README.md`).
//!
//! The boundary helpers (`error`, `json`, `abort`, `callback`, `fetch`, `events`, `options`) are what every export
//! uses; `host` is the JS host; `ai` holds the exports behind `@at-inc/pi-ai-rs`, one file per facade module.

mod abort;
mod ai;
mod callback;
mod error;
mod events;
mod fetch;
mod host;
mod json;
mod options;
mod websocket;

pub use host::init;
