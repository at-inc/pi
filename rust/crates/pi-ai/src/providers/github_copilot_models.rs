//! Port of `packages/ai/src/providers/github-copilot.models.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-core
// Signature copied from w2-ai-core's port on `capy/rust-port-w2-ai-core` (c990cc571), which replaces this stub.
use std::sync::LazyLock;

use crate::model_catalog::ChatModelCatalog;

pub static GITHUB_COPILOT_MODELS: LazyLock<ChatModelCatalog> = LazyLock::new(|| todo!("PORT-STUB"));
