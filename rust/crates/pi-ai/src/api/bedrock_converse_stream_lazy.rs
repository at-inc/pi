//! Port of `packages/ai/src/api/bedrock-converse-stream.lazy.ts`.
//! PORT-STATUS: ported
//!
//! The TS loads the module through a variable import specifier so bundlers cannot follow it into the Node-only AWS SDK.
//! Rust links `api::bedrock_converse_stream` statically, so the loader resolves at once; the override stays for callers
//! that register their own module.

use std::cell::RefCell;
use std::rc::Rc;

use crate::api::bedrock_converse_stream::BedrockConverseStreamStreams;
use crate::api::lazy::lazy_api;
use crate::types::ProviderStreams;

thread_local! {
	static BEDROCK_MODULE_OVERRIDE: RefCell<Option<Rc<dyn ProviderStreams>>> = const { RefCell::new(None) };
}

/// `setBedrockProviderModule(module)`: overrides the loaded bedrock implementation.
pub fn set_bedrock_provider_module(module: Rc<dyn ProviderStreams>) {
	BEDROCK_MODULE_OVERRIDE.with_borrow_mut(|slot| *slot = Some(module));
}

/// `bedrockConverseStreamApi()`: the bedrock-converse-stream API, loaded on first use.
pub fn bedrock_converse_stream_api() -> Rc<dyn ProviderStreams> {
	lazy_api(
		Rc::new(|| {
			Box::pin(async {
				Ok(BEDROCK_MODULE_OVERRIDE
					.with_borrow(Clone::clone)
					.unwrap_or_else(|| Rc::new(BedrockConverseStreamStreams) as Rc<dyn ProviderStreams>))
			})
		}),
		None,
	)
}
