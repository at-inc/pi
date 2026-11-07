//! Port of `packages/ai/src/models.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-core
// Only the items `compat/mod.rs` and `cli.rs` use, with the signatures of w2-ai-core's port on `capy/rust-port-w2-ai-core` (c990cc571), which replaces this stub.
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use crate::auth::types::ProviderAuth;
use crate::types::{Context, Model, ProviderStreamOptions, SimpleStreamOptions, StreamFunction};
use crate::utils::event_stream::AssistantMessageEventStream;

macro_rules! models_options {
	($(#[$doc:meta])* $name:ident, $base:ty) => {
		$(#[$doc])*
		#[derive(Clone, Default)]
		pub struct $name {
			pub base: $base,
		}

		impl Deref for $name {
			type Target = $base;

			fn deref(&self) -> &Self::Target {
				&self.base
			}
		}

		impl DerefMut for $name {
			fn deref_mut(&mut self) -> &mut Self::Target {
				&mut self.base
			}
		}

		impl From<$base> for $name {
			fn from(base: $base) -> Self {
				Self { base }
			}
		}
	};
}

models_options!(
	/// `ModelsApiStreamOptions<TApi> = ApiStreamOptions<TApi> & ModelsRequestTransforms`.
	ModelsApiStreamOptions,
	ProviderStreamOptions
);
models_options!(
	/// `SimpleStreamOptions & ModelsRequestTransforms`.
	ModelsSimpleStreamOptions,
	SimpleStreamOptions
);

/// `Provider.getModels`.
pub type GetModelsFunction = Rc<dyn Fn() -> Vec<Model>>;

#[derive(Clone)]
pub struct Provider {
	pub id: String,
	pub name: String,

	/// Required: at least one of `apiKey`/`oauth`. Every provider has auth
	/// semantics — even providers with only ambient credentials (env vars, AWS
	/// profiles, ADC files) and keyless local servers provide `apiKey` auth
	/// whose `resolve()` reports whether the provider is configured.
	/// `Models.getAuth()` returns undefined when the provider is unconfigured.
	pub auth: ProviderAuth,

	pub get_models: GetModelsFunction,
	pub stream: StreamFunction<ProviderStreamOptions>,
	pub stream_simple: StreamFunction<SimpleStreamOptions>,
}

pub trait Models {
	fn get_provider(&self, id: &str) -> Option<Rc<Provider>>;

	fn stream(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsApiStreamOptions>,
	) -> AssistantMessageEventStream;

	fn stream_simple(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsSimpleStreamOptions>,
	) -> AssistantMessageEventStream;
}

pub trait MutableModels: Models {}

#[derive(Clone, Default)]
pub struct CreateModelsOptions {}
