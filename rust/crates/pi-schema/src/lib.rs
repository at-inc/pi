//! Port of `packages/schema/src/index.ts`.
//! PORT-STATUS: ported
//!
//! JSON Schema validation (drafts 2020-12 and 7) without eval: compile() builds a schema into plain closures once.
//! The builders return plain JSON Schema objects.
//!
//! Rust port: the type-only exports `Annotations`, `LengthBounds`, `NumberBounds`, `Static` and `StaticObject` have no
//! runtime effect and are not exported (see `builders.rs`). `oneOf` is `one_of`.

// <generated-modules>
pub mod annotations;
pub mod builders;
pub mod checks;
pub mod coerce;
pub mod compiler;
pub mod explain;
pub mod formats;
pub mod resolver;
pub mod shared;
pub mod typebox_static;
pub mod types;
pub mod validator;
// </generated-modules>

pub use builders::{
	Optional, Properties, Property, Schema, arr, bool, bytes, int, lit, map, nul, nullable, num, obj, one_of, opt, str,
	tuple, union, unknown,
};
pub use types::{Check, CompileOptions, Draft, FormatCheck, JsonSchema, ParseResult, UnresolvedRefs, ValidationError};
pub use validator::{CoercingValidator, Validator, compile, compile_coercing};
