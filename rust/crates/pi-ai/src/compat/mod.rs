//! Port of `packages/ai/src/compat.ts`.
//! PORT-STATUS: reviewed
//!
//! Temporary compatibility entrypoint preserving the old global pi-ai API
//! surface: api-dispatch `stream()`/`complete()` with env API key injection,
//! the api-registry, generated catalog reads (`getModel`/`getModels`/
//! `getProviders`), per-API lazy stream wrappers, and image generation.
//!
//! Existing apps switch imports from "@at-inc/pi-ai" to
//! "@at-inc/pi-ai/compat" unchanged; new code uses `createModels()`
//! and the provider factories. This module is deleted with the coding-agent
//! ModelManager migration.
//!
//! Port notes:
//! - `export * from "./index.ts"` is `pub use crate::*`; the other `export *` lines are glob re-exports of their
//!   modules. `BuiltinProvider` only narrows a string type, so it has no counterpart (providers are `&str`).
//! - The api-registry, the builtin instances and `compatModels` are module state, so they are per thread, like the
//!   per-isolate module state in TS. The registration TS runs at module load (`registerBuiltInApiProviders()` and
//!   `builtinModels()`) runs on the first call into this module.
//! - `ApiProvider` and `ApiProviderInternal` take `ProviderStreamOptions` for `stream` (TS: `StreamOptions`): TS
//!   hands the caller's options object through unchanged, API-specific keys included, which `ProviderStreamOptions`
//!   carries in `extra`. A registered provider is held as `Rc<ApiProviderInternal>`, so `getApiProvider()` identity
//!   comparisons are `Rc::ptr_eq`.
//! - `stream()` and `streamSimple()` return `PiResult` for their synchronous throws; `complete()` and
//!   `completeSimple()` call `stream()` at call time, like the TS async functions before their first `await`, and
//!   resolve with the stream's `result()`, which is `None` only for a stream that ended without a final message.
//! - `compatModels.stream()` takes a `Context`; the normalized transcript is passed as one with only `messages`,
//!   which `normalizeContext()` leaves unchanged.

pub use crate::api::anthropic_messages_lazy::*;
pub use crate::api::azure_openai_responses_lazy::*;
pub use crate::api::bedrock_converse_stream_lazy::*;
pub use crate::api::google_generative_ai_lazy::*;
pub use crate::api::google_vertex_lazy::*;
pub use crate::api::mistral_conversations_lazy::*;
pub use crate::api::openai_codex_responses_lazy::*;
pub use crate::api::openai_completions_lazy::*;
pub use crate::api::openai_responses_lazy::*;
pub use crate::api::pi_messages_lazy::*;
pub use crate::env_api_keys::*;
pub use crate::image_models::*;
pub use crate::images::*;
pub use crate::images_api_registry::*;
pub use crate::legacy_api_aliases::*;
pub use crate::providers::images::register_builtins::*;
pub use crate::*;

use std::cell::{Cell, RefCell};
use std::ops::DerefMut;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_host::{LocalBoxFuture, PiError, PiResult};

use crate::providers::all::builtin_models;
use crate::utils::text::js_trim;

// <generated-modules>
pub mod extension_oauth_types;
// </generated-modules>

/// @deprecated Static catalog read. Use `getBuiltinModel` from "@at-inc/pi-ai/providers/all" or `Models.getModel()`.
pub use crate::providers::all::get_builtin_model as get_model;
/// @deprecated Static catalog read. Use `getBuiltinModels` from "@at-inc/pi-ai/providers/all" or `Models.getModels()`.
pub use crate::providers::all::get_builtin_models as get_models;
/// @deprecated Static catalog read. Use `getBuiltinProviders` from "@at-inc/pi-ai/providers/all" or `Models.getProviders()`.
pub use crate::providers::all::get_builtin_providers as get_providers;

/// `ApiStreamFunction`.
pub type ApiStreamFunction = StreamFunction<ProviderStreamOptions>;

/// `ApiStreamSimpleFunction`.
pub type ApiStreamSimpleFunction = StreamFunction<SimpleStreamOptions>;

/// `ApiProvider`.
#[derive(Clone)]
pub struct ApiProvider {
	pub api: String,
	pub stream: StreamFunction<ProviderStreamOptions>,
	pub stream_simple: StreamFunction<SimpleStreamOptions>,
}

/// The registered form of an `ApiProvider`: its streams check the model's API first.
pub struct ApiProviderInternal {
	pub api: String,
	pub stream: ApiStreamFunction,
	pub stream_simple: ApiStreamSimpleFunction,
}

struct RegisteredApiProvider {
	provider: Rc<ApiProviderInternal>,
	source_id: Option<String>,
}

thread_local! {
	static MODULE_LOADED: Cell<bool> = const { Cell::new(false) };
	static API_PROVIDER_REGISTRY: RefCell<IndexMap<String, RegisteredApiProvider>> = RefCell::new(IndexMap::new());
	static BUILTIN_API_PROVIDER_INSTANCES: RefCell<IndexMap<String, Option<Rc<ApiProviderInternal>>>> =
		RefCell::new(IndexMap::new());
	static COMPAT_MODELS: Rc<dyn MutableModels> = builtin_models(None);
}

/// Runs what TS runs when this module loads: `registerBuiltInApiProviders()`.
fn load_module() {
	if MODULE_LOADED.with(|loaded| loaded.replace(true)) {
		return;
	}
	register_built_in_api_providers_now();
}

fn mismatched_api(model: &Model, api: &str) -> PiError {
	PiError::new(format!("Mismatched api: {} expected {api}", &*model.api))
}

fn wrap_stream(api: String, stream: StreamFunction<ProviderStreamOptions>) -> ApiStreamFunction {
	Rc::new(move |model, context, options| {
		if *model.api != *api {
			return Err(mismatched_api(model, &api));
		}
		stream(model, context, options)
	})
}

fn wrap_stream_simple(api: String, stream_simple: StreamFunction<SimpleStreamOptions>) -> ApiStreamSimpleFunction {
	Rc::new(move |model, context, options| {
		if *model.api != *api {
			return Err(mismatched_api(model, &api));
		}
		stream_simple(model, context, options)
	})
}

/// `registerApiProvider(provider, sourceId)`.
pub fn register_api_provider(provider: ApiProvider, source_id: Option<String>) {
	load_module();
	register_api_provider_now(provider, source_id);
}

fn register_api_provider_now(provider: ApiProvider, source_id: Option<String>) {
	let registered = RegisteredApiProvider {
		provider: Rc::new(ApiProviderInternal {
			api: provider.api.clone(),
			stream: wrap_stream(provider.api.clone(), provider.stream),
			stream_simple: wrap_stream_simple(provider.api.clone(), provider.stream_simple),
		}),
		source_id,
	};
	API_PROVIDER_REGISTRY.with(|registry| registry.borrow_mut().insert(provider.api, registered));
}

/// `getApiProvider(api)`.
pub fn get_api_provider(api: &str) -> Option<Rc<ApiProviderInternal>> {
	load_module();
	get_api_provider_now(api)
}

fn get_api_provider_now(api: &str) -> Option<Rc<ApiProviderInternal>> {
	API_PROVIDER_REGISTRY.with(|registry| registry.borrow().get(api).map(|entry| entry.provider.clone()))
}

/// `getApiProviders()`.
pub fn get_api_providers() -> Vec<Rc<ApiProviderInternal>> {
	load_module();
	API_PROVIDER_REGISTRY.with(|registry| registry.borrow().values().map(|entry| entry.provider.clone()).collect())
}

/// `unregisterApiProviders(sourceId)`.
pub fn unregister_api_providers(source_id: &str) {
	load_module();
	API_PROVIDER_REGISTRY.with(|registry| {
		registry
			.borrow_mut()
			.retain(|_, entry| entry.source_id.as_deref() != Some(source_id));
	});
}

fn clear_api_providers() {
	API_PROVIDER_REGISTRY.with(|registry| registry.borrow_mut().clear());
}

/// `Math.random().toString(36).slice(2, 10)`: up to eight random base-36 digits.
fn random_base36_id() -> String {
	const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
	(0..8)
		.map(|_| char::from(DIGITS[rand::random_range(0..DIGITS.len())]))
		.collect()
}

/// `registerFauxProvider(options = {})`.
pub fn register_faux_provider(options: Option<RegisterFauxProviderOptions>) -> FauxProviderRegistration {
	load_module();
	let core = create_faux_core(options.unwrap_or_default());
	let source_id = format!("faux-provider-{}", random_base36_id());
	let (stream_core, stream_simple_core) = (core.clone(), core.clone());
	register_api_provider_now(
		ApiProvider {
			api: core.api.clone(),
			stream: Rc::new(move |model, context, options: Option<ProviderStreamOptions>| {
				Ok(stream_core.stream(
					model,
					context,
					options.map(|options| SimpleStreamOptions {
						base: options.base,
						..SimpleStreamOptions::default()
					}),
				))
			}),
			stream_simple: Rc::new(move |model, context, options| {
				Ok(stream_simple_core.stream_simple(model, context, options))
			}),
		},
		Some(source_id.clone()),
	);
	FauxProviderRegistration {
		core,
		unregister: Rc::new(move || unregister_api_providers(&source_id)),
	}
}

thread_local! {
	static BUILTIN_APIS: Vec<(&'static str, Rc<dyn ProviderStreams>)> = vec![
		("anthropic-messages", anthropic_messages_api()),
		("openai-completions", openai_completions_api()),
		("openai-responses", openai_responses_api()),
		("openai-codex-responses", openai_codex_responses_api()),
		("azure-openai-responses", azure_openai_responses_api()),
		("google-generative-ai", google_generative_ai_api()),
		("google-vertex", google_vertex_api()),
		("mistral-conversations", mistral_conversations_api()),
		("bedrock-converse-stream", bedrock_converse_stream_api()),
		("pi-messages", pi_messages_api()),
	];
}

/// Registers the builtin API implementations into the api-registry without
/// clobbering existing entries: compat may load after a test or extension has
/// already registered an override for a builtin api id.
pub fn register_built_in_api_providers() {
	load_module();
	register_built_in_api_providers_now();
}

fn register_built_in_api_providers_now() {
	let builtin_apis = BUILTIN_APIS.with(Clone::clone);
	for (api, streams) in builtin_apis {
		if get_api_provider_now(api).is_none() {
			let (stream, stream_simple) = (streams.clone(), streams);
			register_api_provider_now(
				ApiProvider {
					api: api.to_string(),
					stream: Rc::new(move |model, context, options| stream.stream(model, context, options)),
					stream_simple: Rc::new(move |model, context, options| {
						stream_simple.stream_simple(model, context, options)
					}),
				},
				None,
			);
		}
		let instance = get_api_provider_now(api);
		BUILTIN_API_PROVIDER_INSTANCES.with(|instances| instances.borrow_mut().insert(api.to_string(), instance));
	}
}

/// `resetApiProviders()`.
pub fn reset_api_providers() {
	load_module();
	clear_api_providers();
	BUILTIN_API_PROVIDER_INSTANCES.with(|instances| instances.borrow_mut().clear());
	register_built_in_api_providers_now();
}

const AMBIENT_AUTH_MARKER: &str = "<authenticated>";

fn has_explicit_api_key(api_key: Option<&str>) -> bool {
	api_key.is_some_and(|api_key| !js_trim(api_key).is_empty())
}

fn with_env_api_key<O: DerefMut<Target = StreamOptions> + Default>(model: &Model, options: Option<O>) -> Option<O> {
	if has_explicit_api_key(options.as_ref().and_then(|options| options.api_key.as_deref())) {
		return options;
	}
	let api_key = get_env_api_key(
		&model.provider,
		options.as_ref().and_then(|options| options.env.as_ref()),
	);
	let Some(api_key) = api_key.filter(|api_key| !api_key.is_empty() && api_key != AMBIENT_AUTH_MARKER) else {
		return options;
	};
	let mut options = options.unwrap_or_default();
	options.api_key = Some(api_key);
	Some(options)
}

fn has_resolved_cloudflare_auth(options: Option<&StreamOptions>) -> bool {
	has_explicit_api_key(options.and_then(|options| options.api_key.as_deref()))
		|| options
			.and_then(|options| options.headers.as_ref())
			.and_then(|headers| headers.get("cf-aig-authorization"))
			.is_some_and(Option::is_some)
}

fn compat_models() -> Rc<dyn MutableModels> {
	COMPAT_MODELS.with(Clone::clone)
}

fn get_builtin_provider_for_model(model: &Model) -> Option<Rc<Provider>> {
	let registered = get_api_provider_now(&model.api);
	let builtin =
		BUILTIN_API_PROVIDER_INSTANCES.with(|instances| instances.borrow().get(&*model.api).cloned().flatten());
	let same = match (&registered, &builtin) {
		(Some(registered), Some(builtin)) => Rc::ptr_eq(registered, builtin),
		(None, None) => true,
		_ => false,
	};
	if !same {
		return None;
	}
	let provider = compat_models().get_provider(&model.provider)?;
	(provider.get_models)()
		.iter()
		.any(|candidate| candidate.api == model.api)
		.then_some(provider)
}

fn resolve_api_provider(api: &str) -> PiResult<Rc<ApiProviderInternal>> {
	get_api_provider_now(api).ok_or_else(|| PiError::new(format!("No API provider registered for api: {api}")))
}

/// The normalized transcript as the `Context` `compatModels` takes.
fn transcript_context(transcript: &TranscriptContext) -> Context {
	Context {
		messages: transcript.messages.clone(),
		..Context::default()
	}
}

/// `stream(model, context, options)`.
pub fn stream(
	model: &Model,
	context: &Context,
	options: Option<ProviderStreamOptions>,
) -> PiResult<AssistantMessageEventStream> {
	load_module();
	let transcript = normalize_context(context);
	if let Some(builtin_provider) = get_builtin_provider_for_model(model) {
		if model.provider.starts_with("cloudflare-") && !has_resolved_cloudflare_auth(options.as_deref()) {
			return Ok(compat_models().stream(
				model,
				&transcript_context(&transcript),
				options.map(ModelsApiStreamOptions::from),
			));
		}
		return (builtin_provider.stream)(model, &transcript, with_env_api_key(model, options));
	}
	let provider = resolve_api_provider(&model.api)?;
	(provider.stream)(model, &transcript, with_env_api_key(model, options))
}

/// `complete(model, context, options)`.
pub fn complete(
	model: &Model,
	context: &Context,
	options: Option<ProviderStreamOptions>,
) -> LocalBoxFuture<'static, PiResult<Option<AssistantMessage>>> {
	let stream = stream(model, context, options);
	Box::pin(async move { Ok(stream?.result().await) })
}

/// `streamSimple(model, context, options)`.
pub fn stream_simple(
	model: &Model,
	context: &Context,
	options: Option<SimpleStreamOptions>,
) -> PiResult<AssistantMessageEventStream> {
	load_module();
	let transcript = normalize_context(context);
	if let Some(builtin_provider) = get_builtin_provider_for_model(model) {
		if model.provider.starts_with("cloudflare-") && !has_resolved_cloudflare_auth(options.as_deref()) {
			return Ok(compat_models().stream_simple(
				model,
				&transcript_context(&transcript),
				options.map(ModelsSimpleStreamOptions::from),
			));
		}
		return (builtin_provider.stream_simple)(model, &transcript, with_env_api_key(model, options));
	}
	let provider = resolve_api_provider(&model.api)?;
	(provider.stream_simple)(model, &transcript, with_env_api_key(model, options))
}

/// `completeSimple(model, context, options)`.
pub fn complete_simple(
	model: &Model,
	context: &Context,
	options: Option<SimpleStreamOptions>,
) -> LocalBoxFuture<'static, PiResult<Option<AssistantMessage>>> {
	let stream = stream_simple(model, context, options);
	Box::pin(async move { Ok(stream?.result().await) })
}
