//! Port of `packages/ai/src/model-catalog.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-core
// Signature copied from w2-ai-core's port on `capy/rust-port-w2-ai-core` (c990cc571), which replaces this stub.
use indexmap::IndexMap;

use crate::types::Model;

/// Chat models of one provider keyed by model id.
pub type ChatModelCatalog = IndexMap<String, Model>;
