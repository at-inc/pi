//! Port of `packages/ai/src/bedrock-provider.ts`.
//! PORT-STATUS: reviewed

use std::rc::Rc;

use crate::api::bedrock_converse_stream::BedrockConverseStreamStreams;
use crate::types::ProviderStreams;

/// `bedrockProviderModule`: `stream` and `streamSimple` of `api::bedrock_converse_stream`, for
/// `set_bedrock_provider_module`.
pub fn bedrock_provider_module() -> Rc<dyn ProviderStreams> {
	Rc::new(BedrockConverseStreamStreams)
}
