//! Port of `packages/ai/src/index.ts`.
//! PORT-STATUS: reviewed
//!
//! The crate root re-exports the core surface like the TS index: no generated catalogs, no provider factories, no
//! api-registry, no OAuth implementations, no compat. Those stay reachable through their modules
//! (`pi_ai::providers::*`, `pi_ai::api::*`, `pi_ai::compat`). The TS type-only re-exports of the API option and
//! event types (`AnthropicOptions`, `BedrockOptions`, `PiMessagesEvent`, ...) and of the extension OAuth types
//! come from modules other shards port; they are reachable at their module paths once ported and are not re-exported
//! here yet.

// <generated-modules>
pub mod api;
pub mod auth;
pub mod bedrock_provider;
pub mod bun_oauth;
pub mod cli;
pub mod compat;
pub mod env_api_keys;
pub mod image_models;
pub mod images;
pub mod images_api_registry;
pub mod legacy_api_aliases;
pub mod model_catalog;
pub mod models;
pub mod models_generated;
pub mod models_store;
pub mod oauth;
pub mod providers;
pub mod session_resources;
pub mod telemetry;
pub mod types;
pub mod utils;
// </generated-modules>

pub use api::lazy::*;
pub use auth::context::*;
pub use auth::credential_store::*;
pub use auth::helpers::*;
pub use auth::types::*;
pub use models::*;
pub use models_store::*;
pub use providers::faux::*;
pub use session_resources::*;
pub use types::*;
pub use utils::diagnostics::*;
pub use utils::event_stream::*;
pub use utils::identity::*;
pub use utils::json_parse::*;
pub use utils::overflow::*;
pub use utils::retry::*;
pub use utils::streaming_json::*;
pub use utils::string_enum::*;
pub use utils::text::{content_text, get_system_message_text, render_system_message_update};
pub use utils::transcript::*;
pub use utils::uuid::uuidv7;
pub use utils::validation::*;
pub use utils::version::PI_AI_VERSION;
