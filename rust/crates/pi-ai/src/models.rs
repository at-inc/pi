//! Port of `packages/ai/src/models.ts`.
//! PORT-STATUS: ported
//!
//! `Provider` is an object-literal interface that apps implement, so it is a struct of `Rc` callbacks; optional
//! methods are `Option` fields. `Models` and `MutableModels` are traits, implemented by `createModels()`'s
//! collection and by `withClientIdentity()`'s wrapper (ported here from `utils/identity.ts`, next to the trait it
//! wraps). Providers are held as `Rc<Provider>` so `getProvider()` returns the object that was set.
//!
//! Differences from the TS:
//! - `getModels()` / `getAllModels()` cannot throw in Rust, so the "a throwing provider yields no models" guard
//!   has no counterpart.
//! - `getAuth(providerOrModel)`'s overload is the `GetAuthTarget` enum.
//! - `refresh()`'s `errors` are `PiError`s; every Rust error is an `Error`, so the `ModelsError(\"model_source\")`
//!   wrapper for non-`Error` throws never applies.
//! - `complete()` resolves with the stream's `result()`, which is `None` only for a stream that ended without a
//!   final message.
//! - What TS runs before an operation's first `await` (abort checks, provider lookups, the start of auth resolution,
//!   the provider's login call, the credential delete) runs at call time; the rest is spawned on the host executor at
//!   call time, so it runs even when the returned future is dropped, as an unawaited TS promise does.
//! - The option types carry only their declared fields: TS spreads any extra keys of an untyped caller's
//!   `ModelsImagesOptions`, `ModelsSimpleStreamOptions`, `ModelsDeferred*Options` or `ModelsClassifierOptions` into
//!   the provider request, and reads `sessionId` from any of them; Rust reads `session_id` from the stream options,
//!   which declare it, and `ModelsApiStreamOptions` keeps API-specific keys in `extra`.
//! - `withClientIdentity()` checks for `MutableModels` at runtime; Rust has `with_client_identity` for `Models` and
//!   `with_client_identity_mutable` for `MutableModels`, so the caller picks.
//! - Header maps hold strings, so a non-string header value on a model of an unknown type is dropped.
//! - A `model.api` such as `"toString"` finds no implementation instead of an `Object.prototype` member.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::{Shared, join_all, try_join_all};
use indexmap::IndexMap;
use pi_host::{AbortController, AbortSignal, LocalBoxFuture, PiError, PiResult};
use serde_json::Value;

use crate::api::lazy::lazy_stream;
use crate::auth::context::default_provider_auth_context;
use crate::auth::credential_store::InMemoryCredentialStore;
use crate::auth::resolve::{AuthResolutionOverrides, refresh_stored_oauth_credential, resolve_provider_auth};
use crate::auth::types::{
	ApiKeyAuthInput, ApiKeyCredential, AuthCheck, AuthContext, AuthInteraction, AuthOperationOptions, AuthResult,
	AuthType, Credential, CredentialStore, LoginOptions, ProviderAuth, ProviderAuthInteraction,
};
use crate::models_store::{InMemoryModelsStore, ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions};
use crate::telemetry::observe_model_request;
use crate::types::{
	AnyModel, AssistantImages, AssistantMessage, BaseModel, CancelDeferredFunction, ClassifierContext,
	ClassifierFunction, ClassifierModel, ClassifierOptions, ClassifierResult, Context, DeferredCancelOptions,
	DeferredFetchOptions, DeferredHandle, FetchDeferredFunction, ImageModel, ImagesContext, ImagesFunction,
	ImagesOptions, Model, ModelCostRates, ModelThinkingLevel, ModelType, ProviderClassifier, ProviderHeaders,
	ProviderImages, ProviderImagesOptions, ProviderRequestOptions, ProviderStreamOptions, ProviderStreams,
	SimpleStreamOptions, StreamFunction, Usage, UsageCost,
};
use crate::utils::abort::{operation_signal, race_with_abort_signal};
use crate::utils::event_stream::AssistantMessageEventStream;
use crate::utils::identity::{
	ClientIdentity, ClientIdentityTarget, get_open_code_session_headers, merge_client_identity,
};
use crate::utils::model_operations::{
	ModelRef, assert_chat_model, assert_classifier_input_supported, assert_classifier_model, assert_image_model,
	classifier_error_result, image_error_result,
};
use crate::utils::transcript::normalize_context;

pub use crate::auth::resolve::{ModelsError, ModelsErrorCode};
pub use crate::telemetry::AI_TELEMETRY_SCHEMA;
pub use crate::utils::model_operations::{get_model_type, is_model_type};

/// `ModelsPublication`.
#[derive(Clone, Default)]
pub struct ModelsPublication {
	/// Provider-selected persisted catalog. Omit to leave storage unchanged; null deletes it.
	pub persist: Option<Option<ModelsStoreEntry>>,
	/// Optional synchronous update of provider-private in-memory catalog state.
	pub update: Option<Rc<dyn Fn()>>,
}

/// `RefreshModelsContext.publish`.
pub type PublishModelsFunction = Rc<dyn Fn(ModelsPublication) -> LocalBoxFuture<'static, PiResult<bool>>>;

#[derive(Clone)]
pub struct RefreshModelsContext {
	/// Effective configured credential. OAuth credentials are refreshed before network access.
	pub credential: Option<Credential>,
	/// Immutable provider-scoped catalog snapshot captured before this refresh phase.
	pub stored: Option<ModelsStoreEntry>,
	/// Generation-checked publication. Persistence policy remains provider-owned;
	/// the update runs synchronously only after the selected persistence mutation.
	pub publish: PublishModelsFunction,
	/// False during offline/cache-only initialization.
	pub allow_network: bool,
	/// Bypass provider freshness checks and fetch immediately when network access is allowed.
	pub force: Option<bool>,
	/// Always present, including when the public refresh caller omits its optional signal.
	pub signal: AbortSignal,
	/// Base URL of the registered provider, including configuration overrides that wrappers apply
	/// outside the provider receiving this context.
	pub base_url: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ModelsRefreshOptions {
	pub allow_network: Option<bool>,
	/// Restrict refresh to these provider IDs. Unknown and static providers are ignored.
	pub providers: Option<Vec<String>>,
	/// Bypass provider freshness checks and fetch immediately when network access is allowed.
	pub force: Option<bool>,
	pub signal: Option<AbortSignal>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelsRefreshResult {
	pub aborted: bool,
	pub errors: IndexMap<String, PiError>,
}

/// `ModelsRequestTransforms.transformHeaders`.
pub type TransformHeadersFunction = Rc<dyn Fn(ProviderHeaders) -> LocalBoxFuture<'static, PiResult<ProviderHeaders>>>;

#[derive(Clone, Default)]
pub struct ModelsRequestTransforms {
	/// Transform fully assembled model/auth/request headers before provider dispatch.
	pub transform_headers: Option<TransformHeadersFunction>,
}

macro_rules! models_options {
	($(#[$doc:meta])* $name:ident, $base:ty) => {
		$(#[$doc])*
		#[derive(Clone, Default)]
		pub struct $name {
			pub base: $base,
			/// Transform fully assembled model/auth/request headers before provider dispatch.
			pub transform_headers: Option<TransformHeadersFunction>,
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
				Self { base, transform_headers: None }
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
models_options!(
	/// `DeferredFetchOptions & ModelsRequestTransforms`.
	ModelsDeferredFetchOptions,
	DeferredFetchOptions
);
models_options!(
	/// `DeferredCancelOptions & ModelsRequestTransforms`.
	ModelsDeferredCancelOptions,
	DeferredCancelOptions
);
models_options!(
	/// `ImagesOptions & ModelsRequestTransforms`.
	ModelsImagesOptions,
	ImagesOptions
);
models_options!(
	/// `ClassifierOptions & ModelsRequestTransforms`.
	ModelsClassifierOptions,
	ClassifierOptions
);

/// Models from stores and remote sources may have types that only newer versions know.
fn has_known_model_type(model: &AnyModel) -> bool {
	matches!(get_model_type(model), "chat" | "image" | "classifier")
}

/// Drops stored models whose type this version does not know.
fn with_known_model_types(entry: ModelsStoreEntry) -> ModelsStoreEntry {
	ModelsStoreEntry {
		models: entry.models.into_iter().filter(has_known_model_type).collect(),
		..entry
	}
}

/// Any model a provider can list: `Model | ImageModel | ClassifierModel`.
pub type ProviderModel = AnyModel;

/// `Provider.getModels`.
pub type GetModelsFunction = Rc<dyn Fn() -> Vec<Model>>;
/// `Provider.getAllModels`.
pub type GetAllModelsFunction = Rc<dyn Fn() -> Vec<ProviderModel>>;
/// `Provider.refreshModels`.
pub type RefreshModelsFunction = Rc<dyn Fn(RefreshModelsContext) -> LocalBoxFuture<'static, PiResult<()>>>;
/// `Provider.filterModels`.
pub type FilterModelsFunction = Rc<dyn Fn(&[Model], Option<&Credential>) -> Vec<Model>>;
/// `Provider.filterAllModels`.
pub type FilterAllModelsFunction = Rc<dyn Fn(&[ProviderModel], Option<&Credential>) -> Vec<ProviderModel>>;

/// A provider is the concrete runtime unit. It owns id/name/base metadata,
/// auth methods, model listing, and the operations its models support
/// (streaming, image generation, classification).
#[derive(Clone)]
pub struct Provider {
	pub id: String,
	pub name: String,

	pub base_url: Option<String>,
	pub headers: Option<ProviderHeaders>,

	/// Required: at least one of `apiKey`/`oauth`. Every provider has auth
	/// semantics — even providers with only ambient credentials (env vars, AWS
	/// profiles, ADC files) and keyless local servers provide `apiKey` auth
	/// whose `resolve()` reports whether the provider is configured.
	/// `Models.getAuth()` returns undefined when the provider is unconfigured.
	pub auth: ProviderAuth,

	/// Current known chat models, sync. Static providers return their catalog;
	/// dynamic providers return the list as of the last `refreshModels()` (empty
	/// before the first).
	pub get_models: GetModelsFunction,

	/// Current known models of every type, sync, with the same contract as
	/// `getModels()`. Providers with only chat models may omit it; `Models` then
	/// uses `getModels()`. Model ids are unique within each type; one upstream
	/// model may have separate entries for different operations.
	pub get_all_models: Option<GetAllModelsFunction>,

	/// Dynamic providers only: restore `context.stored` and optionally fetch a newer list using
	/// the effective credential. Implementations retain their previous list on failure, publish
	/// persistence and synchronous state changes through `context.publish()`, and honor the
	/// shared abort signal for blocking work.
	pub refresh_models: Option<RefreshModelsFunction>,

	/// Optional provider policy for credential-specific model availability.
	/// `getModels()` remains the complete synchronous chat catalog; `Models.getAvailable()`
	/// applies this filter after confirming that provider auth is configured.
	pub filter_models: Option<FilterModelsFunction>,

	/// Optional credential-specific availability policy across every model type.
	/// Without it, `Models.getAllAvailable()` applies `filterModels` to chat models
	/// and keeps every other model.
	pub filter_all_models: Option<FilterAllModelsFunction>,

	/// Stream a normalized transcript. `Models` normalizes the caller's `Context` before dispatching here.
	pub stream: StreamFunction<ProviderStreamOptions>,

	pub stream_simple: StreamFunction<SimpleStreamOptions>,
	pub fetch_deferred: Option<FetchDeferredFunction>,
	pub cancel_deferred: Option<CancelDeferredFunction>,

	/// Present when the provider supports dedicated image models.
	pub generate_images: Option<ImagesFunction>,

	/// Present when the provider supports structured classifier models.
	pub classify: Option<ClassifierFunction>,
}

impl Provider {
	fn all_models(&self) -> Vec<ProviderModel> {
		match &self.get_all_models {
			Some(get_all_models) => get_all_models(),
			None => (self.get_models)().into_iter().map(AnyModel::Chat).collect(),
		}
	}
}

/// `getAuth(providerId)` or `getAuth(model)`.
#[derive(Clone, Copy)]
pub enum GetAuthTarget<'a> {
	Provider(&'a str),
	Model(&'a AnyModel),
}

/// Runtime collection of providers plus auth application and request
/// convenience. Providers own request behavior; `Models` resolves auth and
/// delegates each request to the provider that owns the model.
///
/// Read accessors come in three flavors: the unqualified ones (`getModels`,
/// `getModel`, `getAvailable`) return chat models, the `*OfType` accessors
/// return one model type, and `getAllModels`/`getAllAvailable` return every type.
pub trait Models {
	fn get_providers(&self) -> Vec<Rc<Provider>>;
	fn get_provider(&self, id: &str) -> Option<Rc<Provider>>;

	/// Sync read of last-known chat models from one provider or all providers.
	fn get_models(&self, provider: Option<&str>) -> Vec<Model>;

	/// Sync runtime chat model lookup against last-known lists.
	fn get_model(&self, provider: &str, id: &str) -> Option<Model>;

	/// Sync read of last-known models of one type from one provider or all providers.
	fn get_models_of_type(&self, model_type: ModelType, provider: Option<&str>) -> Vec<AnyModel>;

	/// Sync runtime lookup of a model of one type against last-known lists.
	fn get_model_of_type(&self, model_type: ModelType, provider: &str, id: &str) -> Option<AnyModel>;

	/// Sync read of last-known models of every type from one provider or all providers.
	fn get_all_models(&self, provider: Option<&str>) -> Vec<AnyModel>;

	/// Refresh selected configured dynamic providers concurrently (all when `providers` is omitted).
	/// Provider errors and cancellation are returned without rejecting; static, unknown, and
	/// unconfigured providers are skipped.
	fn refresh(&self, options: Option<ModelsRefreshOptions>) -> LocalBoxFuture<'static, PiResult<ModelsRefreshResult>>;

	/// Check whether a provider has complete auth configuration without refreshing OAuth.
	fn check_auth(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<AuthCheck>>>;

	/// Return chat models whose providers have complete auth configuration.
	fn get_available(
		&self,
		provider_id: Option<&str>,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Vec<Model>>>;

	/// Return models of one type whose providers have complete auth configuration.
	fn get_available_of_type(
		&self,
		model_type: ModelType,
		provider_id: Option<&str>,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Vec<AnyModel>>>;

	/// Return models of every type whose providers have complete auth configuration.
	fn get_all_available(
		&self,
		provider_id: Option<&str>,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Vec<AnyModel>>>;

	/// Resolve provider-scoped auth by provider id, or provider auth plus static
	/// model headers when passed a model. Includes a source label for status UI.
	/// Resolves `undefined` when the provider is unknown or unconfigured.
	/// Rejects with `ModelsError`: code "oauth" when a token refresh fails (the
	/// stored credential is preserved for retry; re-login fixes it), code "auth"
	/// when api-key resolution or the credential store fails. Request paths
	/// surface rejections as stream errors.
	fn get_auth(
		&self,
		target: GetAuthTarget<'_>,
		overrides: Option<AuthResolutionOverrides>,
	) -> LocalBoxFuture<'static, PiResult<Option<AuthResult>>>;

	/// Run a provider-owned login flow and persist its returned credential.
	fn login(
		&self,
		provider_id: &str,
		r#type: AuthType,
		interaction: AuthInteraction,
		options: Option<LoginOptions>,
	) -> LocalBoxFuture<'static, PiResult<Credential>>;

	/// Remove the stored credential for a provider.
	fn logout(&self, provider_id: &str, options: Option<AuthOperationOptions>)
	-> LocalBoxFuture<'static, PiResult<()>>;

	fn stream(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsApiStreamOptions>,
	) -> AssistantMessageEventStream;

	fn complete(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsApiStreamOptions>,
	) -> LocalBoxFuture<'static, Option<AssistantMessage>>;

	fn stream_simple(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsSimpleStreamOptions>,
	) -> AssistantMessageEventStream;
	fn complete_simple(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsSimpleStreamOptions>,
	) -> LocalBoxFuture<'static, Option<AssistantMessage>>;
	fn stream_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<ModelsDeferredFetchOptions>,
	) -> AssistantMessageEventStream;
	fn fetch_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<ModelsDeferredFetchOptions>,
	) -> LocalBoxFuture<'static, Option<AssistantMessage>>;
	fn cancel_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<ModelsDeferredCancelOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>>;

	/// Generate images through the owning provider with auth resolved like
	/// `stream()`. Never rejects: unknown providers, unconfigured auth, and
	/// providers without `generateImages` return an error `AssistantImages`.
	fn generate_images(
		&self,
		model: &ImageModel,
		context: &ImagesContext,
		options: Option<ModelsImagesOptions>,
	) -> LocalBoxFuture<'static, AssistantImages>;

	/// Classify structured state through the owning provider. Never rejects.
	fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<ModelsClassifierOptions>,
	) -> LocalBoxFuture<'static, ClassifierResult>;
}

pub trait MutableModels: Models {
	/// Upsert/replace by provider.id. Provider ids are unique.
	fn set_provider(&self, provider: Rc<Provider>);
	fn delete_provider(&self, id: &str);
	fn clear_providers(&self);
}

#[derive(Clone, Default)]
pub struct CreateModelsOptions {
	pub credentials: Option<Rc<dyn CredentialStore>>,
	pub models_store: Option<Rc<dyn ModelsStore>>,
	pub auth_context: Option<Rc<dyn AuthContext>>,
	/// Default identity of every request; a request's `identity` overrides it field by field.
	pub identity: Option<ClientIdentity>,
}

fn merge_headers(
	base: Option<&ProviderHeaders>,
	override_headers: Option<&ProviderHeaders>,
) -> Option<ProviderHeaders> {
	if base.is_none() && override_headers.is_none() {
		return None;
	}
	let mut merged = base.cloned().unwrap_or_default();
	for (name, value) in override_headers.into_iter().flatten() {
		let lower_name = name.to_lowercase();
		merged.retain(|existing_name, _| existing_name.to_lowercase() != lower_name);
		merged.insert(name.clone(), value.clone());
	}
	Some(merged)
}

fn to_provider_headers(headers: &IndexMap<String, String>) -> ProviderHeaders {
	headers
		.iter()
		.map(|(name, value)| (name.clone(), Some(value.clone())))
		.collect()
}

/// `model.headers` of any model, as provider headers.
fn any_model_headers(model: &AnyModel) -> Option<ProviderHeaders> {
	match model {
		AnyModel::Chat(model) => model.headers.as_ref().map(to_provider_headers),
		AnyModel::Image(model) => model.headers.as_ref().map(to_provider_headers),
		AnyModel::Classifier(model) => model.headers.as_ref().map(to_provider_headers),
		AnyModel::Other(value) => value.get("headers").and_then(Value::as_object).map(|headers| {
			headers
				.iter()
				.map(|(name, value)| (name.clone(), value.as_str().map(str::to_string)))
				.collect()
		}),
	}
}

/// The `Credential` of an auth error message: `new ModelsError(code, message, { cause: error })`.
fn models_error(code: ModelsErrorCode, message: &str, cause: &PiError) -> PiError {
	ModelsError::new(code, message, Some(cause.into()))
}

fn auth_type_str(r#type: AuthType) -> &'static str {
	match r#type {
		AuthType::ApiKey => "api_key",
		AuthType::OAuth => "oauth",
	}
}

/// One refresh in flight for a provider: its generation and the controller that supersedes it.
struct RefreshController {
	generation: u64,
	controller: AbortController,
}

type PublicationChain = Shared<LocalBoxFuture<'static, ()>>;

struct ModelsState {
	providers: RefCell<IndexMap<String, Rc<Provider>>>,
	credentials: Rc<dyn CredentialStore>,
	models_store: Rc<dyn ModelsStore>,
	auth_context: Rc<dyn AuthContext>,
	identity: Option<ClientIdentity>,
	refresh_generations: RefCell<HashMap<String, u64>>,
	refresh_controllers: RefCell<IndexMap<String, RefreshController>>,
	publication_chains: RefCell<HashMap<String, PublicationChain>>,
}

/// The collection `createModels()` returns.
struct ModelsImpl {
	state: Rc<ModelsState>,
}

/// An authenticated provider with the credential it was checked against.
struct AuthenticatedProvider {
	provider: Rc<Provider>,
	credential: Option<Credential>,
}

/// `applyAuth`'s result: the request base URL override and the request options with auth applied.
struct AppliedAuth<M> {
	base_url: Option<String>,
	options: ProviderRequestOptions<M>,
}

impl ModelsState {
	fn provider(&self, id: &str) -> Option<Rc<Provider>> {
		self.providers.borrow().get(id).cloned()
	}

	fn supersede_provider_refresh(&self, provider_id: &str) -> u64 {
		let generation = {
			let mut generations = self.refresh_generations.borrow_mut();
			let generation = generations.get(provider_id).copied().unwrap_or(0) + 1;
			generations.insert(provider_id.to_string(), generation);
			generation
		};
		let previous = self.refresh_controllers.borrow_mut().shift_remove(provider_id);
		if let Some(previous) = previous {
			previous.controller.abort();
		}
		generation
	}

	fn begin_provider_refresh(&self, provider_id: &str) -> (u64, AbortController) {
		let generation = self.supersede_provider_refresh(provider_id);
		let controller = AbortController::new();
		self.refresh_controllers.borrow_mut().insert(
			provider_id.to_string(),
			RefreshController {
				generation,
				controller: controller.clone(),
			},
		);
		(generation, controller)
	}

	fn is_current_generation(&self, provider_id: &str, generation: u64) -> bool {
		self.refresh_generations.borrow().get(provider_id) == Some(&generation)
	}

	fn publish_provider_models(
		self: &Rc<Self>,
		provider_id: &str,
		generation: u64,
		signal: &AbortSignal,
		publication: ModelsPublication,
	) -> LocalBoxFuture<'static, PiResult<bool>> {
		let previous = self.publication_chains.borrow().get(provider_id).cloned();
		let (settle, settled) = oneshot::channel::<PiResult<bool>>();
		let queued = {
			let (state, provider_id, signal) = (self.clone(), provider_id.to_string(), signal.clone());
			async move {
				if let Some(previous) = previous {
					previous.await;
				}
				let result = async {
					if signal.aborted() || !state.is_current_generation(&provider_id, generation) {
						return Ok(false);
					}
					let options = Some(ModelsStoreOperationOptions {
						signal: Some(signal.clone()),
					});
					match publication.persist {
						Some(None) => state.models_store.delete(&provider_id, options).await?,
						Some(Some(entry)) => state.models_store.write(&provider_id, entry, options).await?,
						None => {}
					}
					if signal.aborted() || !state.is_current_generation(&provider_id, generation) {
						return Ok(false);
					}
					if let Some(update) = &publication.update {
						update();
					}
					Ok(true)
				}
				.await;
				let _ = settle.send(result);
			}
		};
		let tail: PublicationChain = queued.boxed_local().shared();
		self.publication_chains
			.borrow_mut()
			.insert(provider_id.to_string(), tail.clone());
		{
			let (state, provider_id, tail) = (self.clone(), provider_id.to_string(), tail.clone());
			pi_host::spawn_local(async move {
				tail.clone().await;
				let mut chains = state.publication_chains.borrow_mut();
				if chains
					.get(&provider_id)
					.is_some_and(|current| Shared::ptr_eq(current, &tail))
				{
					chains.remove(&provider_id);
				}
			});
		}
		let queued = async move {
			match settled.await {
				Ok(result) => result,
				Err(_) => futures::future::pending().await,
			}
		};
		race_with_abort_signal(queued, signal).boxed_local()
	}

	async fn run_provider_refresh_phase(
		self: Rc<Self>,
		provider: Rc<Provider>,
		credential: Option<Credential>,
		allow_network: bool,
		force: Option<bool>,
		generation: u64,
		signal: AbortSignal,
	) -> PiResult<()> {
		let refresh_models = provider
			.refresh_models
			.clone()
			.expect("only providers with refreshModels are refreshed");
		let stored = self
			.models_store
			.read(
				&provider.id,
				Some(ModelsStoreOperationOptions {
					signal: Some(signal.clone()),
				}),
			)
			.await?;
		let publish: PublishModelsFunction = {
			let (state, provider_id, signal) = (self.clone(), provider.id.clone(), signal.clone());
			Rc::new(move |publication| state.publish_provider_models(&provider_id, generation, &signal, publication))
		};
		refresh_models(RefreshModelsContext {
			credential,
			stored: stored.map(with_known_model_types),
			publish,
			allow_network,
			force: if allow_network { force } else { None },
			signal,
			base_url: provider.base_url.clone(),
		})
		.await
	}

	async fn resolve_refresh_credential(
		self: Rc<Self>,
		provider: Rc<Provider>,
		stored: Option<Credential>,
		signal: AbortSignal,
	) -> PiResult<Option<Credential>> {
		if let Some(Credential::OAuth(stored)) = stored {
			let Some(oauth) = &provider.auth.oauth else {
				return Ok(None);
			};
			if pi_host::now_ms() < stored.expires {
				return Ok(Some(Credential::OAuth(stored)));
			}
			if signal.aborted() {
				return Ok(None);
			}
			// A refresh that has started survives cancellation or a superseding model refresh, so a
			// rotated refresh token is always persisted. A newer refresh then sees the fresh credential.
			let refreshed = refresh_stored_oauth_credential(
				self.credentials.clone(),
				&provider.id,
				oauth,
				Rc::new(|current| pi_host::now_ms() >= current.expires),
				&signal,
			)
			.await?;
			return Ok(refreshed.map(Credential::OAuth));
		}

		let Some(api_key) = &provider.auth.api_key else {
			return Ok(None);
		};
		let credential = match stored {
			Some(Credential::ApiKey(credential)) => Some(credential),
			_ => None,
		};
		let result = (api_key.resolve)(ApiKeyAuthInput {
			ctx: self.auth_context.clone(),
			credential,
			signal,
		})
		.await?;
		let Some(result) = result else {
			return Ok(None);
		};
		Ok(Some(Credential::ApiKey(ApiKeyCredential {
			key: result.auth.api_key,
			env: result.env,
		})))
	}

	async fn read_credential(self: Rc<Self>, provider_id: String, signal: AbortSignal) -> PiResult<Option<Credential>> {
		self.credentials
			.read(&provider_id, Some(AuthOperationOptions { signal: Some(signal) }))
			.await
			.map_err(|error| {
				models_error(
					ModelsErrorCode::Auth,
					&format!("Credential store read failed for {provider_id}"),
					&error,
				)
			})
	}

	async fn check_provider_auth(
		self: Rc<Self>,
		provider: Rc<Provider>,
		credential: Option<Credential>,
		signal: AbortSignal,
	) -> PiResult<Option<AuthCheck>> {
		if let Some(Credential::OAuth(_)) = credential {
			return Ok(provider.auth.oauth.as_ref().map(|_| AuthCheck {
				source: Some("OAuth".into()),
				r#type: AuthType::OAuth,
			}));
		}
		let Some(api_key) = &provider.auth.api_key else {
			return Ok(None);
		};
		if let Some(check) = &api_key.check {
			let credential = match credential {
				Some(Credential::ApiKey(credential)) => Some(credential),
				_ => None,
			};
			return check(ApiKeyAuthInput {
				ctx: self.auth_context.clone(),
				credential,
				signal,
			})
			.await
			.map_err(|error| {
				models_error(
					ModelsErrorCode::Auth,
					&format!("API key auth check failed for provider {}", provider.id),
					&error,
				)
			});
		}

		let resolution = resolve_provider_auth(
			&provider.id,
			&provider.auth,
			self.credentials.clone(),
			self.auth_context.clone(),
			Some(AuthResolutionOverrides {
				signal: Some(signal),
				..AuthResolutionOverrides::default()
			}),
		)
		.await?;
		Ok(resolution.map(|resolution| AuthCheck {
			source: resolution.source,
			r#type: AuthType::ApiKey,
		}))
	}

	fn get_authenticated_providers(
		self: &Rc<Self>,
		provider_id: Option<&str>,
		signal: &AbortSignal,
	) -> LocalBoxFuture<'static, PiResult<Vec<AuthenticatedProvider>>> {
		let aborted = signal.throw_if_aborted();
		// TS tests `providerId` for truthiness, so "" lists every provider.
		let providers: Vec<Rc<Provider>> = match provider_id.filter(|provider_id| !provider_id.is_empty()) {
			Some(provider_id) => self.provider(provider_id).into_iter().collect(),
			None => self.providers.borrow().values().cloned().collect(),
		};
		let (state, signal) = (self.clone(), signal.clone());
		Box::pin(async move {
			aborted?;
			// Each check runs to completion like a `Promise.all` member, even after another one fails.
			let checks = try_join_all(providers.into_iter().map(|provider| {
				let (state, signal) = (state.clone(), signal.clone());
				start_now(async move {
					let credential = state
						.clone()
						.read_credential(provider.id.clone(), signal.clone())
						.await?;
					let auth = state
						.check_provider_auth(provider.clone(), credential.clone(), signal)
						.await?;
					PiResult::Ok((AuthenticatedProvider { provider, credential }, auth))
				})
			}))
			.await?;
			Ok(checks
				.into_iter()
				.filter(|(_, auth)| auth.is_some())
				.map(|(entry, _)| entry)
				.collect())
		})
	}

	fn get_auth(
		self: &Rc<Self>,
		provider_id: &str,
		model_headers: Option<ProviderHeaders>,
		overrides: Option<AuthResolutionOverrides>,
	) -> LocalBoxFuture<'static, PiResult<Option<AuthResult>>> {
		let overrides = overrides.unwrap_or_default();
		let signal = operation_signal(overrides.signal.as_ref());
		let Some(provider) = self.provider(provider_id) else {
			return Box::pin(async { Ok(None) });
		};
		let state = self.clone();
		start_now(async move {
			let result = resolve_provider_auth(
				&provider.id,
				&provider.auth,
				state.credentials.clone(),
				state.auth_context.clone(),
				Some(AuthResolutionOverrides {
					signal: Some(signal),
					..overrides
				}),
			)
			.await?;
			let (Some(mut result), Some(model_headers)) = (result.clone(), model_headers) else {
				return Ok(result);
			};
			result.auth.headers = merge_headers(result.auth.headers.as_ref(), Some(&model_headers));
			Ok(Some(result))
		})
	}

	fn require_provider<M: ModelRef + ?Sized>(&self, model: &M) -> PiResult<Rc<Provider>> {
		self.provider(model.model_provider()).ok_or_else(|| {
			ModelsError::new(
				ModelsErrorCode::Provider,
				&format!("Unknown provider: {}", model.model_provider()),
				None,
			)
		})
	}

	fn require_chat_provider(&self, model: &Model) -> PiResult<Rc<Provider>> {
		assert_chat_model(model)?;
		self.require_provider(model)
	}

	/// `applyAuth(model, options)`. `model` is passed as the fields it reads; `session_id` is
	/// `options.sessionId` when the options have one. The provider lookup and the start of auth resolution run at
	/// call time, the part TS runs before its first `await`.
	fn apply_auth<M: 'static>(
		self: &Rc<Self>,
		model: AnyModel,
		model_base_url: String,
		options: ProviderRequestOptions<M>,
		session_id: Option<String>,
		transform_headers: Option<TransformHeadersFunction>,
	) -> LocalBoxFuture<'static, PiResult<AppliedAuth<M>>> {
		if let Err(error) = self.require_provider(&model) {
			return Box::pin(async move { Err(error) });
		}
		let resolution = self.get_auth(
			model.model_provider(),
			any_model_headers(&model),
			Some(AuthResolutionOverrides {
				api_key: options.api_key.clone(),
				env: options.env.clone(),
				signal: options.signal.clone(),
				..AuthResolutionOverrides::default()
			}),
		);
		let identity = self.identity.clone();
		Box::pin(async move {
			let resolution = resolution.await?;
			finish_apply_auth(
				model,
				model_base_url,
				options,
				session_id,
				transform_headers,
				resolution,
				identity,
			)
			.await
		})
	}
}

/// The rest of `applyAuth`, after auth resolution settles.
async fn finish_apply_auth<M: 'static>(
	model: AnyModel,
	model_base_url: String,
	options: ProviderRequestOptions<M>,
	session_id: Option<String>,
	transform_headers: Option<TransformHeadersFunction>,
	resolution: Option<AuthResult>,
	models_identity: Option<ClientIdentity>,
) -> PiResult<AppliedAuth<M>> {
	let Some(resolution) = resolution else {
		return Err(ModelsError::new(
			ModelsErrorCode::Auth,
			&format!("Provider is not configured: {}", model.model_provider()),
			None,
		));
	};
	let auth = resolution.auth;

	// Explicit request options win per-field; the Models-only transform runs last.
	let api_key = options.api_key.clone().or(auth.api_key);
	let base_url = auth.base_url.filter(|base_url| !base_url.is_empty());
	let request_base_url = base_url.clone().unwrap_or(model_base_url);
	let session_headers = get_open_code_session_headers(
		ClientIdentityTarget {
			provider: model.model_provider(),
			base_url: &request_base_url,
		},
		session_id.as_deref(),
	)
	.map(|headers| to_provider_headers(&headers));
	let mut headers = merge_headers(
		merge_headers(session_headers.as_ref(), auth.headers.as_ref()).as_ref(),
		options.headers.as_ref(),
	);
	if let Some(transform_headers) = transform_headers {
		headers = Some(transform_headers(headers.unwrap_or_default()).await?);
	}
	let env = if resolution.env.is_some() || options.env.is_some() {
		let mut env = resolution.env.unwrap_or_default();
		env.extend(options.env.clone().unwrap_or_default());
		Some(env)
	} else {
		None
	};
	let identity = merge_client_identity(&[models_identity.as_ref(), options.identity.as_ref()]);
	let mut options = options;
	options.api_key = api_key;
	options.headers = headers;
	options.env = env;
	if identity.is_some() {
		options.identity = identity;
	}

	Ok(AppliedAuth { base_url, options })
}

/// Runs `operation` on the host executor from now on, like the body of a TS async function, which runs whether or
/// not anyone awaits its promise. The returned future only waits for the outcome, so dropping it cancels nothing.
fn start_now<T: 'static>(operation: impl Future<Output = T> + 'static) -> LocalBoxFuture<'static, T> {
	let (sender, receiver) = oneshot::channel();
	pi_host::spawn_local(async move {
		let _ = sender.send(operation.await);
	});
	Box::pin(async move {
		match receiver.await {
			Ok(outcome) => outcome,
			Err(_) => futures::future::pending().await,
		}
	})
}

fn with_base_url<A>(model: &mut BaseModel<A>, base_url: Option<String>) {
	if let Some(base_url) = base_url {
		model.base_url = base_url;
	}
}

impl Models for ModelsImpl {
	fn get_providers(&self) -> Vec<Rc<Provider>> {
		self.state.providers.borrow().values().cloned().collect()
	}

	fn get_provider(&self, id: &str) -> Option<Rc<Provider>> {
		self.state.provider(id)
	}

	fn get_models(&self, provider: Option<&str>) -> Vec<Model> {
		if let Some(provider) = provider {
			return match self.state.provider(provider) {
				Some(entry) => (entry.get_models)(),
				None => Vec::new(),
			};
		}

		self.get_providers()
			.iter()
			.flat_map(|entry| (entry.get_models)())
			.collect()
	}

	fn get_model(&self, provider: &str, id: &str) -> Option<Model> {
		self.get_models(Some(provider)).into_iter().find(|model| model.id == id)
	}

	fn get_models_of_type(&self, model_type: ModelType, provider: Option<&str>) -> Vec<AnyModel> {
		self.get_all_models(provider)
			.into_iter()
			.filter(|model| is_model_type(model, model_type))
			.collect()
	}

	fn get_model_of_type(&self, model_type: ModelType, provider: &str, id: &str) -> Option<AnyModel> {
		self.get_models_of_type(model_type, Some(provider))
			.into_iter()
			.find(|model| model.model_id() == id)
	}

	fn get_all_models(&self, provider: Option<&str>) -> Vec<AnyModel> {
		if let Some(provider) = provider {
			return match self.state.provider(provider) {
				Some(entry) => entry.all_models(),
				None => Vec::new(),
			};
		}

		self.get_providers()
			.iter()
			.flat_map(|entry| entry.all_models())
			.collect()
	}

	fn refresh(&self, options: Option<ModelsRefreshOptions>) -> LocalBoxFuture<'static, PiResult<ModelsRefreshResult>> {
		let options = options.unwrap_or_default();
		let allow_network = options.allow_network.unwrap_or(true);
		let caller_signal = operation_signal(options.signal.as_ref());
		let errors: Rc<RefCell<IndexMap<String, PiError>>> = Rc::new(RefCell::new(IndexMap::new()));
		if caller_signal.aborted() {
			return Box::pin(async {
				Ok(ModelsRefreshResult {
					aborted: true,
					errors: IndexMap::new(),
				})
			});
		}
		let selected: Option<HashSet<String>> = options.providers.map(|providers| providers.into_iter().collect());
		let refreshable: Vec<Rc<Provider>> = self
			.get_providers()
			.into_iter()
			.filter(|provider| {
				provider.refresh_models.is_some()
					&& selected.as_ref().is_none_or(|selected| selected.contains(&provider.id))
			})
			.collect();

		let tasks: Vec<_> = refreshable
			.into_iter()
			.map(|provider| {
				let state = self.state.clone();
				let (generation, controller) = state.begin_provider_refresh(&provider.id);
				let signal = AbortSignal::any(&[caller_signal.clone(), controller.signal()]);
				let operation = {
					let (state, provider, signal) = (state.clone(), provider.clone(), signal.clone());
					let force = options.force;
					async move {
						let (stored_credential, credential_error) =
							match state.clone().read_credential(provider.id.clone(), signal.clone()).await {
								Ok(credential) => (credential, None),
								Err(error) => (None, Some(error)),
							};

						// Restore cached provider state before auth resolution or network access.
						state
							.clone()
							.run_provider_refresh_phase(
								provider.clone(),
								stored_credential.clone(),
								false,
								None,
								generation,
								signal.clone(),
							)
							.await?;
						if let Some(error) = credential_error {
							return Err(error);
						}
						if !allow_network || signal.aborted() {
							return Ok(());
						}

						let credential = state
							.clone()
							.resolve_refresh_credential(provider.clone(), stored_credential, signal.clone())
							.await?;
						let Some(credential) = credential else {
							return Ok(());
						};
						state
							.run_provider_refresh_phase(provider, Some(credential), true, force, generation, signal)
							.await
					}
				};
				let raced = race_with_abort_signal(operation, &signal).boxed_local();
				let errors = errors.clone();
				async move {
					if let Err(error) = raced.await
						&& !signal.aborted()
					{
						errors.borrow_mut().insert(provider.id.clone(), error);
					}
					let mut controllers = state.refresh_controllers.borrow_mut();
					if controllers
						.get(&provider.id)
						.is_some_and(|current| current.generation == generation)
					{
						controllers.shift_remove(&provider.id);
					}
				}
			})
			.collect();

		let refresh = race_with_abort_signal(
			async move {
				join_all(tasks).await;
				Ok(())
			},
			&caller_signal,
		);
		Box::pin(async move {
			if let Err(error) = refresh.await
				&& !caller_signal.aborted()
			{
				return Err(error);
			}
			Ok(ModelsRefreshResult {
				aborted: caller_signal.aborted(),
				errors: errors.borrow().clone(),
			})
		})
	}

	fn check_auth(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<AuthCheck>>> {
		let signal = operation_signal(options.and_then(|options| options.signal).as_ref());
		let aborted = signal.throw_if_aborted();
		let provider = self.state.provider(provider_id);
		let state = self.state.clone();
		let check = {
			let signal = signal.clone();
			async move {
				aborted?;
				let Some(provider) = provider else {
					return Ok(None);
				};
				let credential = state
					.clone()
					.read_credential(provider.id.clone(), signal.clone())
					.await?;
				state.check_provider_auth(provider, credential, signal).await
			}
		};
		race_with_abort_signal(check, &signal).boxed_local()
	}

	fn get_available(
		&self,
		provider_id: Option<&str>,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Vec<Model>>> {
		let signal = operation_signal(options.and_then(|options| options.signal).as_ref());
		let providers = self.state.get_authenticated_providers(provider_id, &signal);
		let available = async move {
			let providers = providers.await?;
			Ok(providers
				.into_iter()
				.flat_map(|AuthenticatedProvider { provider, credential }| {
					let models = (provider.get_models)();
					match &provider.filter_models {
						Some(filter_models) => filter_models(&models, credential.as_ref()),
						None => models,
					}
				})
				.collect())
		};
		race_with_abort_signal(available, &signal).boxed_local()
	}

	fn get_available_of_type(
		&self,
		model_type: ModelType,
		provider_id: Option<&str>,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Vec<AnyModel>>> {
		let available = self.get_all_available(provider_id, options);
		Box::pin(async move {
			Ok(available
				.await?
				.into_iter()
				.filter(|model| is_model_type(model, model_type))
				.collect())
		})
	}

	fn get_all_available(
		&self,
		provider_id: Option<&str>,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Vec<AnyModel>>> {
		let signal = operation_signal(options.and_then(|options| options.signal).as_ref());
		let providers = self.state.get_authenticated_providers(provider_id, &signal);
		let available = async move {
			let providers = providers.await?;
			Ok(providers
				.into_iter()
				.flat_map(|AuthenticatedProvider { provider, credential }| {
					let models = provider.all_models();
					if let Some(filter_all_models) = &provider.filter_all_models {
						return filter_all_models(&models, credential.as_ref());
					}
					let Some(filter_models) = &provider.filter_models else {
						return models;
					};
					let available_chat_ids: HashSet<String> =
						filter_models(&(provider.get_models)(), credential.as_ref())
							.into_iter()
							.map(|model| model.base.id)
							.collect();
					models
						.into_iter()
						.filter(|model| {
							!is_model_type(model, ModelType::Chat) || available_chat_ids.contains(model.model_id())
						})
						.collect()
				})
				.collect())
		};
		race_with_abort_signal(available, &signal).boxed_local()
	}

	fn get_auth(
		&self,
		target: GetAuthTarget<'_>,
		overrides: Option<AuthResolutionOverrides>,
	) -> LocalBoxFuture<'static, PiResult<Option<AuthResult>>> {
		match target {
			GetAuthTarget::Provider(provider_id) => self.state.get_auth(provider_id, None, overrides),
			GetAuthTarget::Model(model) => {
				self.state
					.get_auth(model.model_provider(), any_model_headers(model), overrides)
			}
		}
	}

	fn login(
		&self,
		provider_id: &str,
		r#type: AuthType,
		interaction: AuthInteraction,
		options: Option<LoginOptions>,
	) -> LocalBoxFuture<'static, PiResult<Credential>> {
		let signal = operation_signal(interaction.signal.as_ref());
		// Everything up to the provider's login call runs at call time, as in TS before the first `await`.
		let started = (|| -> PiResult<LocalBoxFuture<'static, PiResult<Credential>>> {
			signal.throw_if_aborted()?;
			let Some(provider) = self.state.provider(provider_id) else {
				return Err(ModelsError::new(
					ModelsErrorCode::Provider,
					&format!("Unknown provider: {provider_id}"),
					None,
				));
			};
			let interaction = ProviderAuthInteraction {
				signal: signal.clone(),
				prompt: interaction.prompt,
				notify: interaction.notify,
			};
			Ok(match r#type {
				AuthType::OAuth => match &provider.auth.oauth {
					Some(oauth) => {
						let login = (oauth.login)(interaction, options);
						Box::pin(async move { login.await.map(Credential::OAuth) })
					}
					None => return Err(unsupported_login(&provider.name, r#type)),
				},
				AuthType::ApiKey => match provider
					.auth
					.api_key
					.as_ref()
					.and_then(|api_key| api_key.login.as_ref())
				{
					Some(login) => {
						let login = login(interaction);
						Box::pin(async move { login.await.map(Credential::ApiKey) })
					}
					None => return Err(unsupported_login(&provider.name, r#type)),
				},
			})
		})();
		let (state, provider_id) = (self.state.clone(), provider_id.to_string());
		start_now(async move {
			let login_operation = started?;
			let credential = race_with_abort_signal(login_operation, &signal).await?;
			let mutation_started = Rc::new(Cell::new(false));
			let (mark_mutation_started, started) = oneshot::channel::<()>();
			let mutation = {
				let (mutation_started, credential) = (mutation_started.clone(), credential.clone());
				let modify = state.credentials.modify(
					&provider_id,
					Box::new(move |_| {
						mutation_started.set(true);
						let _ = mark_mutation_started.send(());
						Box::pin(async move { Ok(Some(credential)) })
					}),
					Some(AuthOperationOptions {
						signal: Some(signal.clone()),
					}),
				);
				let (settle, settled) = oneshot::channel();
				pi_host::spawn_local(async move {
					let _ = settle.send(modify.await);
				});
				settled
					.map(|result| result.unwrap_or_else(|_| Ok(None)))
					.boxed_local()
					.shared()
			};
			let outcome: PiResult<()> = async {
				// TS `onAbort` reads `mutationStarted` when the abort fires, so the listener decides then.
				let aborted_first: Rc<RefCell<Option<PiError>>> = Rc::default();
				let (abort_fired, abort) = oneshot::channel::<()>();
				let listener = {
					let (aborted_first, mutation_started) = (aborted_first.clone(), mutation_started.clone());
					signal.add_listener(move |reason| {
						if !mutation_started.get() {
							*aborted_first.borrow_mut() = Some(reason.clone());
							let _ = abort_fired.send(());
						}
					})
				};
				if !mutation_started.get()
					&& let Some(reason) = signal.reason()
				{
					*aborted_first.borrow_mut() = Some(reason);
				}
				let started = started.map(|_| Ok(()));
				let settled = mutation.clone().map(|result| result.map(|_| ()));
				let raced = futures::select_biased! {
					_ = abort.fuse() => Ok(()),
					result = started.fuse() => result,
					result = settled.fuse() => result,
				};
				signal.remove_listener(listener);
				if let Some(reason) = aborted_first.take() {
					return Err(reason);
				}
				raced?;
				mutation.await.map(|_| ())
			}
			.await;
			if let Err(error) = outcome {
				signal.throw_if_aborted()?;
				return Err(models_error(
					ModelsErrorCode::Auth,
					&format!("Credential store modify failed for {provider_id}"),
					&error,
				));
			}
			Ok(credential)
		})
	}

	fn logout(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let signal = operation_signal(options.and_then(|options| options.signal).as_ref());
		if let Err(error) = signal.throw_if_aborted() {
			return Box::pin(async move { Err(error) });
		}
		let delete = self.state.credentials.delete(
			provider_id,
			Some(AuthOperationOptions {
				signal: Some(signal.clone()),
			}),
		);
		let provider_id = provider_id.to_string();
		start_now(async move {
			if let Err(error) = delete.await {
				signal.throw_if_aborted()?;
				return Err(models_error(
					ModelsErrorCode::Auth,
					&format!("Credential store delete failed for {provider_id}"),
					&error,
				));
			}
			Ok(())
		})
	}

	fn stream(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsApiStreamOptions>,
	) -> AssistantMessageEventStream {
		let transcript = normalize_context(context);
		let options = options.unwrap_or_default();
		let observer = observe_model_request(
			model,
			options.telemetry.clone(),
			options.extra.get("reasoning").and_then(Value::as_str),
			false,
		);
		let state = self.state.clone();
		let request_model = model.clone();
		lazy_stream(
			model,
			move || {
				let provider = match state.require_chat_provider(&request_model) {
					Ok(provider) => provider,
					Err(error) => return Box::pin(async move { Err(error) }) as LocalBoxFuture<'static, _>,
				};
				let ModelsApiStreamOptions {
					base: mut stream_options,
					transform_headers,
				} = options;
				let session_id = stream_options.session_id.clone();
				let applied = state.apply_auth(
					AnyModel::Chat(request_model.clone()),
					request_model.base_url.clone(),
					std::mem::take(&mut stream_options.base.base),
					session_id,
					transform_headers,
				);
				Box::pin(async move {
					let applied = applied.await?;
					let mut request_model = request_model;
					with_base_url(&mut request_model.base, applied.base_url);
					stream_options.base.base = applied.options;
					(provider.stream)(&request_model, &transcript, Some(stream_options))
				})
			},
			Some(observer),
		)
	}

	fn complete(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsApiStreamOptions>,
	) -> LocalBoxFuture<'static, Option<AssistantMessage>> {
		let stream = self.stream(model, context, options);
		Box::pin(async move { stream.result().await })
	}

	fn stream_simple(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsSimpleStreamOptions>,
	) -> AssistantMessageEventStream {
		let transcript = normalize_context(context);
		let options = options.unwrap_or_default();
		let observer = observe_model_request(
			model,
			options.telemetry.clone(),
			options.reasoning.map(|reasoning| reasoning.as_str()),
			false,
		);
		let state = self.state.clone();
		let request_model = model.clone();
		lazy_stream(
			model,
			move || {
				let provider = match state.require_chat_provider(&request_model) {
					Ok(provider) => provider,
					Err(error) => return Box::pin(async move { Err(error) }) as LocalBoxFuture<'static, _>,
				};
				let ModelsSimpleStreamOptions {
					base: mut stream_options,
					transform_headers,
				} = options;
				let session_id = stream_options.session_id.clone();
				let applied = state.apply_auth(
					AnyModel::Chat(request_model.clone()),
					request_model.base_url.clone(),
					std::mem::take(&mut stream_options.base.base),
					session_id,
					transform_headers,
				);
				Box::pin(async move {
					let applied = applied.await?;
					let mut request_model = request_model;
					with_base_url(&mut request_model.base, applied.base_url);
					stream_options.base.base = applied.options;
					(provider.stream_simple)(&request_model, &transcript, Some(stream_options))
				})
			},
			Some(observer),
		)
	}

	fn complete_simple(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsSimpleStreamOptions>,
	) -> LocalBoxFuture<'static, Option<AssistantMessage>> {
		let stream = self.stream_simple(model, context, options);
		Box::pin(async move { stream.result().await })
	}

	fn stream_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<ModelsDeferredFetchOptions>,
	) -> AssistantMessageEventStream {
		let options = options.unwrap_or_default();
		let observer = observe_model_request(model, options.telemetry.clone(), None, true);
		let state = self.state.clone();
		let (request_model, handle) = (model.clone(), handle.clone());
		lazy_stream(
			model,
			move || {
				let fetch_deferred = state.require_chat_provider(&request_model).and_then(|provider| {
					provider.fetch_deferred.clone().ok_or_else(|| {
						ModelsError::new(
							ModelsErrorCode::Provider,
							&format!(
								"Provider {} does not support deferred responses",
								request_model.provider
							),
							None,
						)
					})
				});
				let fetch_deferred = match fetch_deferred {
					Ok(fetch_deferred) => fetch_deferred,
					Err(error) => return Box::pin(async move { Err(error) }) as LocalBoxFuture<'static, _>,
				};
				let ModelsDeferredFetchOptions {
					base: mut fetch_options,
					transform_headers,
				} = options;
				let applied = state.apply_auth(
					AnyModel::Chat(request_model.clone()),
					request_model.base_url.clone(),
					std::mem::take(&mut fetch_options.base),
					None,
					transform_headers,
				);
				Box::pin(async move {
					let applied = applied.await?;
					let mut request_model = request_model;
					with_base_url(&mut request_model.base, applied.base_url);
					fetch_options.base = applied.options;
					fetch_deferred(&request_model, &handle, Some(fetch_options))
				})
			},
			Some(observer),
		)
	}

	fn fetch_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<ModelsDeferredFetchOptions>,
	) -> LocalBoxFuture<'static, Option<AssistantMessage>> {
		let stream = self.stream_deferred(model, handle, options);
		Box::pin(async move { stream.result().await })
	}

	fn cancel_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<ModelsDeferredCancelOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let cancel_deferred = self.state.require_chat_provider(model).and_then(|provider| {
			provider.cancel_deferred.clone().ok_or_else(|| {
				ModelsError::new(
					ModelsErrorCode::Provider,
					&format!("Provider {} does not support deferred responses", model.provider),
					None,
				)
			})
		});
		let cancel_deferred = match cancel_deferred {
			Ok(cancel_deferred) => cancel_deferred,
			Err(error) => return Box::pin(async move { Err(error) }),
		};
		let ModelsDeferredCancelOptions {
			base: cancel_options,
			transform_headers,
		} = options.unwrap_or_default();
		let applied = self.state.apply_auth(
			AnyModel::Chat(model.clone()),
			model.base_url.clone(),
			cancel_options,
			None,
			transform_headers,
		);
		let (mut request_model, handle) = (model.clone(), handle.clone());
		start_now(async move {
			let applied = applied.await?;
			with_base_url(&mut request_model.base, applied.base_url);
			cancel_deferred(&request_model, &handle, Some(applied.options)).await
		})
	}

	fn generate_images(
		&self,
		model: &ImageModel,
		context: &ImagesContext,
		options: Option<ModelsImagesOptions>,
	) -> LocalBoxFuture<'static, AssistantImages> {
		let options = options.unwrap_or_default();
		let signal = options.signal.clone();
		let started = assert_image_model(model)
			.and_then(|()| self.state.require_provider(model))
			.and_then(|provider| {
				provider.generate_images.clone().ok_or_else(|| {
					ModelsError::new(
						ModelsErrorCode::Provider,
						&format!("Provider {} does not support image generation", model.provider),
						None,
					)
				})
			})
			.map(|generate_images| {
				let ModelsImagesOptions {
					base: mut images_options,
					transform_headers,
				} = options;
				let applied = self.state.apply_auth(
					AnyModel::Image(model.clone()),
					model.base_url.clone(),
					std::mem::take(&mut images_options.base),
					None,
					transform_headers,
				);
				(generate_images, images_options, applied)
			});
		let (request_model, context) = (model.clone(), context.clone());
		start_now(async move {
			let result = async {
				let (generate_images, mut images_options, applied) = started?;
				let applied = applied.await?;
				let mut request_model = request_model.clone();
				with_base_url(&mut request_model.base, applied.base_url);
				images_options.base = applied.options;
				generate_images(&request_model, &context, Some(images_options)).await
			}
			.await;
			match result {
				Ok(images) => images,
				Err(error) => image_error_result(
					&request_model,
					&error,
					signal.as_ref().is_some_and(AbortSignal::aborted),
				),
			}
		})
	}

	fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<ModelsClassifierOptions>,
	) -> LocalBoxFuture<'static, ClassifierResult> {
		let options = options.unwrap_or_default();
		let signal = options.signal.clone();
		let started = assert_classifier_model(model)
			.and_then(|()| assert_classifier_input_supported(model, context))
			.and_then(|()| self.state.require_provider(model))
			.and_then(|provider| {
				provider.classify.clone().ok_or_else(|| {
					ModelsError::new(
						ModelsErrorCode::Provider,
						&format!("Provider {} does not support classification", model.provider),
						None,
					)
				})
			})
			.map(|classify| {
				let ModelsClassifierOptions {
					base: mut classifier_options,
					transform_headers,
				} = options;
				let applied = self.state.apply_auth(
					AnyModel::Classifier(model.clone()),
					model.base_url.clone(),
					std::mem::take(&mut classifier_options.base),
					None,
					transform_headers,
				);
				(classify, classifier_options, applied)
			});
		let (request_model, context) = (model.clone(), context.clone());
		start_now(async move {
			let result = async {
				let (classify, mut classifier_options, applied) = started?;
				let applied = applied.await?;
				let mut request_model = request_model.clone();
				with_base_url(&mut request_model.base, applied.base_url);
				classifier_options.base = applied.options;
				classify(&request_model, &context, Some(classifier_options)).await
			}
			.await;
			match result {
				Ok(result) => result,
				Err(error) => classifier_error_result(
					&request_model,
					&error,
					signal.as_ref().is_some_and(AbortSignal::aborted),
				),
			}
		})
	}
}

fn unsupported_login(provider_name: &str, r#type: AuthType) -> PiError {
	ModelsError::new(
		ModelsErrorCode::Auth,
		&format!("{provider_name} does not support {} login", auth_type_str(r#type)),
		None,
	)
}

impl MutableModels for ModelsImpl {
	fn set_provider(&self, provider: Rc<Provider>) {
		self.state.supersede_provider_refresh(&provider.id);
		self.state.providers.borrow_mut().insert(provider.id.clone(), provider);
	}

	fn delete_provider(&self, id: &str) {
		self.state.supersede_provider_refresh(id);
		self.state.providers.borrow_mut().shift_remove(id);
	}

	fn clear_providers(&self) {
		let ids: indexmap::IndexSet<String> = self
			.state
			.providers
			.borrow()
			.keys()
			.chain(self.state.refresh_controllers.borrow().keys())
			.cloned()
			.collect();
		for id in ids {
			self.state.supersede_provider_refresh(&id);
		}
		self.state.providers.borrow_mut().clear();
	}
}

pub fn create_models(options: Option<CreateModelsOptions>) -> Rc<dyn MutableModels> {
	let options = options.unwrap_or_default();
	Rc::new(ModelsImpl {
		state: Rc::new(ModelsState {
			providers: RefCell::new(IndexMap::new()),
			credentials: options
				.credentials
				.unwrap_or_else(|| Rc::new(InMemoryCredentialStore::new())),
			models_store: options
				.models_store
				.unwrap_or_else(|| Rc::new(InMemoryModelsStore::new())),
			auth_context: options.auth_context.unwrap_or_else(default_provider_auth_context),
			identity: options.identity,
			refresh_generations: RefCell::new(HashMap::new()),
			refresh_controllers: RefCell::new(IndexMap::new()),
			publication_chains: RefCell::new(HashMap::new()),
		}),
	})
}

/// `CreateProviderOptions.api`: a single implementation for all chat models, or a map keyed by `model.api`.
#[derive(Clone)]
pub enum ProviderApi {
	Single(Rc<dyn ProviderStreams>),
	ByApi(IndexMap<String, Rc<dyn ProviderStreams>>),
}

/// `CreateProviderOptions.fetchModels`.
pub type FetchModelsFunction =
	Rc<dyn Fn(RefreshModelsContext) -> LocalBoxFuture<'static, PiResult<Vec<ProviderModel>>>>;

#[derive(Clone, Default)]
pub struct CreateProviderOptions {
	pub id: String,
	/// Display name. Default: `id`.
	pub name: Option<String>,
	pub base_url: Option<String>,
	pub headers: Option<ProviderHeaders>,
	/// Required — every provider has auth semantics, even ambient/keyless ones.
	pub auth: ProviderAuth,
	/// Static baseline models of every type (empty for purely dynamic providers).
	/// Models without `type` are chat models.
	pub models: Vec<ProviderModel>,
	/// Fetch a dynamic model overlay of every type. createProvider restores and
	/// publishes it transactionally and drops models of unknown types.
	pub fetch_models: Option<FetchModelsFunction>,
	/// Credential-specific chat model availability. See `Provider.filterModels`.
	pub filter_models: Option<FilterModelsFunction>,
	/// Credential-specific availability across every model type. See `Provider.filterAllModels`.
	pub filter_all_models: Option<FilterAllModelsFunction>,
	/// Chat implementation: a single one for all chat models, or a map keyed by
	/// `model.api` for mixed-API providers. Optional when `images` or
	/// `classifiers` is given.
	pub api: Option<ProviderApi>,
	/// Image-generation implementations keyed by `model.api`.
	pub images: Option<IndexMap<String, Rc<dyn ProviderImages>>>,
	/// Classifier implementations keyed by `model.api`.
	pub classifiers: Option<IndexMap<String, Rc<dyn ProviderClassifier>>>,
}

/// Builds a provider from parts. Built-in provider factories and models.json
/// custom providers both go through this. A single `api` streams all chat
/// models; an `api` map dispatches on `model.api`, and a model whose api has
/// no entry produces a stream error. One-shot operation maps dispatch on
/// `model.api` the same way. At least one concrete implementation across
/// `api`/`images`/`classifiers` is required; empty maps are rejected.
pub fn create_provider(input: CreateProviderOptions) -> PiResult<Provider> {
	let api = Rc::new(input.api);
	let streams: Vec<Rc<dyn ProviderStreams>> = match &*api {
		Some(ProviderApi::Single(single)) => vec![single.clone()],
		Some(ProviderApi::ByApi(by_api)) => by_api.values().cloned().collect(),
		None => Vec::new(),
	};
	let images = input.images.filter(|images| !images.is_empty());
	let classifiers = input.classifiers.filter(|classifiers| !classifiers.is_empty());
	if streams.is_empty() && images.is_none() && classifiers.is_none() {
		return Err(PiError::new(format!(
			"Provider {}: at least one of \"api\", \"images\", or \"classifiers\" is required.",
			input.id
		)));
	}

	let baseline_models = Rc::new(input.models);
	let dynamic_models: Rc<RefCell<Vec<ProviderModel>>> = Rc::new(RefCell::new(Vec::new()));
	let current_models = {
		let (baseline_models, dynamic_models) = (baseline_models.clone(), dynamic_models.clone());
		Rc::new(move || {
			let mut merged = (*baseline_models).clone();
			for model in dynamic_models.borrow().iter() {
				let index = merged.iter().position(|entry| {
					get_model_type(entry) == get_model_type(model) && entry.model_id() == model.model_id()
				});
				match index {
					Some(index) => merged[index] = model.clone(),
					None => merged.push(model.clone()),
				}
			}
			merged
		})
	};
	let api_for = {
		let api = api.clone();
		move |model: &Model| -> Option<Rc<dyn ProviderStreams>> {
			match &*api {
				Some(ProviderApi::Single(single)) => Some(single.clone()),
				Some(ProviderApi::ByApi(by_api)) => by_api.get(&*model.api).cloned(),
				None => None,
			}
		}
	};
	let id = input.id.clone();
	let no_api_error = move |model: &Model| -> AssistantMessageEventStream {
		let message = format!("Provider {} has no API implementation for \"{}\"", id, model.api);
		lazy_stream(
			model,
			move || async move { Err(ModelsError::new(ModelsErrorCode::Stream, &message, None)) },
			None,
		)
	};

	let refresh_models: Option<RefreshModelsFunction> = input.fetch_models.map(|fetch_models| {
		let (provider_id, dynamic_models) = (input.id.clone(), dynamic_models.clone());
		Rc::new(move |context: RefreshModelsContext| {
			let (provider_id, dynamic_models, fetch_models) =
				(provider_id.clone(), dynamic_models.clone(), fetch_models.clone());
			Box::pin(async move {
				if let Some(stored) = &context.stored {
					let restored: Vec<ProviderModel> = stored
						.models
						.iter()
						.filter(|model| model.model_provider() == provider_id)
						.cloned()
						.collect();
					let update_target = dynamic_models.clone();
					let published = (context.publish)(ModelsPublication {
						persist: None,
						update: Some(Rc::new(move || {
							*update_target.borrow_mut() = restored.clone();
						})),
					})
					.await?;
					if !published {
						return Ok(());
					}
				}
				if !context.allow_network || context.signal.aborted() {
					return Ok(());
				}
				let fetched = fetch_models(context.clone()).await?;
				if context.signal.aborted() {
					return Ok(());
				}
				let refreshed: Vec<ProviderModel> = fetched.into_iter().filter(has_known_model_type).collect();
				let update_target = dynamic_models.clone();
				let update_models = refreshed.clone();
				(context.publish)(ModelsPublication {
					persist: Some(Some(ModelsStoreEntry {
						models: refreshed,
						checked_at: Some(pi_host::now_ms()),
						..ModelsStoreEntry::default()
					})),
					update: Some(Rc::new(move || {
						*update_target.borrow_mut() = update_models.clone();
					})),
				})
				.await?;
				Ok(())
			}) as LocalBoxFuture<'static, PiResult<()>>
		}) as RefreshModelsFunction
	});

	let stream: StreamFunction<ProviderStreamOptions> = {
		let (api_for, no_api_error) = (api_for.clone(), no_api_error.clone());
		Rc::new(move |model, context, options| match api_for(model) {
			Some(streams) => streams.stream(model, context, options),
			None => Ok(no_api_error(model)),
		})
	};
	let stream_simple: StreamFunction<SimpleStreamOptions> = {
		let (api_for, no_api_error) = (api_for.clone(), no_api_error.clone());
		Rc::new(move |model, context, options| match api_for(model) {
			Some(streams) => streams.stream_simple(model, context, options),
			None => Ok(no_api_error(model)),
		})
	};

	let get_models: GetModelsFunction = {
		let current_models = current_models.clone();
		Rc::new(move || {
			current_models()
				.into_iter()
				.filter(|model| is_model_type(model, ModelType::Chat))
				.filter_map(|model| match model {
					AnyModel::Chat(model) => Some(model),
					_ => None,
				})
				.collect()
		})
	};

	let mut provider = Provider {
		id: input.id.clone(),
		name: input.name.unwrap_or_else(|| input.id.clone()),
		base_url: input.base_url,
		headers: input.headers,
		auth: input.auth,
		get_models,
		get_all_models: Some(Rc::new(move || current_models())),
		refresh_models,
		filter_models: input.filter_models,
		filter_all_models: input.filter_all_models,
		stream,
		stream_simple,
		fetch_deferred: None,
		cancel_deferred: None,
		generate_images: None,
		classify: None,
	};

	if streams.iter().any(|entry| entry.fetch_deferred().is_some()) {
		let (api_for, id) = (api_for.clone(), input.id.clone());
		provider.fetch_deferred = Some(Rc::new(move |model, handle, options| {
			let (implementation, id, request_model, handle) =
				(api_for(model), id.clone(), model.clone(), handle.clone());
			Ok(lazy_stream(
				model,
				move || async move {
					let Some(fetch_deferred) =
						implementation.and_then(|implementation| implementation.fetch_deferred())
					else {
						return Err(ModelsError::new(
							ModelsErrorCode::Provider,
							&format!(
								"Provider {id} does not support deferred responses for \"{}\"",
								request_model.api
							),
							None,
						));
					};
					fetch_deferred(&request_model, &handle, options)
				},
				None,
			))
		}));
	}
	if streams.iter().any(|entry| entry.cancel_deferred().is_some()) {
		let (api_for, id) = (api_for.clone(), input.id.clone());
		provider.cancel_deferred = Some(Rc::new(move |model, handle, options| {
			let implementation = api_for(model).and_then(|implementation| implementation.cancel_deferred());
			let Some(cancel_deferred) = implementation else {
				let error = ModelsError::new(
					ModelsErrorCode::Provider,
					&format!("Provider {id} cannot cancel deferred responses for \"{}\"", model.api),
					None,
				);
				return Box::pin(async move { Err(error) });
			};
			cancel_deferred(model, handle, options)
		}));
	}
	if let Some(images) = images {
		let id = input.id.clone();
		provider.generate_images = Some(Rc::new(move |model, context, options| {
			let Some(implementation) = images.get(&*model.api) else {
				let error = ModelsError::new(
					ModelsErrorCode::Provider,
					&format!(
						"Provider {id} has no image generation implementation for \"{}\"",
						model.api
					),
					None,
				);
				let result = image_error_result(model, &error, false);
				return Box::pin(async move { Ok(result) });
			};
			implementation.generate_images(
				model,
				context,
				options.map(|base| ProviderImagesOptions {
					base,
					extra: serde_json::Map::new(),
				}),
			)
		}));
	}
	if let Some(classifiers) = classifiers {
		let id = input.id.clone();
		provider.classify = Some(Rc::new(move |model, context, options| {
			let Some(implementation) = classifiers.get(&*model.api) else {
				let error = ModelsError::new(
					ModelsErrorCode::Provider,
					&format!("Provider {id} has no classifier implementation for \"{}\"", model.api),
					None,
				);
				let result = classifier_error_result(model, &error, false);
				return Box::pin(async move { Ok(result) });
			};
			implementation.classify(model, context, options)
		}));
	}

	Ok(provider)
}

/// Runtime-checked narrowing for dynamically looked-up models: the chat model when `model` is a chat model whose
/// api is `api`.
///
/// Non-chat models never match, even when their api id equals `api`.
pub fn has_api<'a>(model: &'a AnyModel, api: &str) -> Option<&'a Model> {
	match model {
		AnyModel::Chat(chat) if is_model_type(model, ModelType::Chat) && *chat.api == *api => Some(chat),
		_ => None,
	}
}

/// `calculateCost(model, usage)`: sets `usage.cost` from the model's catalog rates and returns it.
pub fn calculate_cost<A, M: Deref<Target = BaseModel<A>>>(model: &M, usage: &mut Usage) -> UsageCost {
	let input_tokens = usage.input + usage.cache_read + usage.cache_write;
	let mut rates = ModelCostRates {
		input: model.cost.input,
		output: model.cost.output,
		cache_read: model.cost.cache_read,
		cache_write: model.cost.cache_write,
	};
	let mut matched_threshold = -1.0;
	for tier in model.cost.tiers.iter().flatten() {
		if input_tokens > tier.input_tokens_above && tier.input_tokens_above > matched_threshold {
			rates = ModelCostRates {
				input: tier.input,
				output: tier.output,
				cache_read: tier.cache_read,
				cache_write: tier.cache_write,
			};
			matched_threshold = tier.input_tokens_above;
		}
	}

	// Anthropic charges 2x base input for 1h cache writes.
	let long_write = usage.cache_write_1h.unwrap_or(0.0);
	let short_write = usage.cache_write - long_write;
	usage.cost.input = (rates.input / 1000000.0) * usage.input;
	usage.cost.output = (rates.output / 1000000.0) * usage.output;
	usage.cost.cache_read = (rates.cache_read / 1000000.0) * usage.cache_read;
	usage.cost.cache_write = (rates.cache_write * short_write + rates.input * 2.0 * long_write) / 1000000.0;
	usage.cost.total = usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
	usage.cost.total_from_provider = Some(false);
	usage.cost.clone()
}

const EXTENDED_THINKING_LEVELS: [ModelThinkingLevel; 7] = [
	ModelThinkingLevel::Off,
	ModelThinkingLevel::Minimal,
	ModelThinkingLevel::Low,
	ModelThinkingLevel::Medium,
	ModelThinkingLevel::High,
	ModelThinkingLevel::Xhigh,
	ModelThinkingLevel::Max,
];

pub fn get_supported_thinking_levels(model: &Model) -> Vec<ModelThinkingLevel> {
	if !model.reasoning {
		return vec![ModelThinkingLevel::Off];
	}

	EXTENDED_THINKING_LEVELS
		.into_iter()
		.filter(|level| {
			let mapped = model.thinking_level_map.as_ref().and_then(|map| map.get(*level));
			if mapped == Some(None) {
				return false;
			}
			if matches!(level, ModelThinkingLevel::Xhigh | ModelThinkingLevel::Max) {
				return mapped.is_some();
			}
			true
		})
		.collect()
}

pub fn clamp_thinking_level(model: &Model, level: ModelThinkingLevel) -> ModelThinkingLevel {
	let available_levels = get_supported_thinking_levels(model);
	if available_levels.contains(&level) {
		return level;
	}

	let first = available_levels.first().copied().unwrap_or(ModelThinkingLevel::Off);
	let Some(requested_index) = EXTENDED_THINKING_LEVELS
		.iter()
		.position(|candidate| *candidate == level)
	else {
		return first;
	};

	for candidate in &EXTENDED_THINKING_LEVELS[requested_index..] {
		if available_levels.contains(candidate) {
			return *candidate;
		}
	}
	for candidate in EXTENDED_THINKING_LEVELS[..requested_index].iter().rev() {
		if available_levels.contains(candidate) {
			return *candidate;
		}
	}
	first
}

/// Check if two models are equal by comparing their type, id, and provider.
/// Returns false if either model is null or undefined.
pub fn models_are_equal<A: ModelRef + ?Sized, B: ModelRef + ?Sized>(a: Option<&A>, b: Option<&B>) -> bool {
	let (Some(a), Some(b)) = (a, b) else {
		return false;
	};
	get_model_type(a) == get_model_type(b) && a.model_id() == b.model_id() && a.model_provider() == b.model_provider()
}

/// `withClientIdentity(models, identity)` (from `utils/identity.ts`): `models` with `identity` as the default
/// identity of its requests.
struct WithClientIdentity<T: ?Sized> {
	models: Rc<T>,
	identity: ClientIdentity,
}

impl<T: ?Sized> WithClientIdentity<T> {
	fn with_identity<M>(&self, options: &mut ProviderRequestOptions<M>) {
		options.identity = merge_client_identity(&[Some(&self.identity), options.identity.as_ref()]);
	}
}

macro_rules! with_identity_options {
	($self:ident, $options:ident, $($field:tt)+) => {{
		let mut options = $options.unwrap_or_default();
		$self.with_identity(&mut options.$($field)+);
		Some(options)
	}};
}

impl<T: Models + ?Sized> Models for WithClientIdentity<T> {
	fn get_providers(&self) -> Vec<Rc<Provider>> {
		self.models.get_providers()
	}

	fn get_provider(&self, id: &str) -> Option<Rc<Provider>> {
		self.models.get_provider(id)
	}

	fn get_models(&self, provider: Option<&str>) -> Vec<Model> {
		self.models.get_models(provider)
	}

	fn get_model(&self, provider: &str, id: &str) -> Option<Model> {
		self.models.get_model(provider, id)
	}

	fn get_models_of_type(&self, model_type: ModelType, provider: Option<&str>) -> Vec<AnyModel> {
		self.models.get_models_of_type(model_type, provider)
	}

	fn get_model_of_type(&self, model_type: ModelType, provider: &str, id: &str) -> Option<AnyModel> {
		self.models.get_model_of_type(model_type, provider, id)
	}

	fn get_all_models(&self, provider: Option<&str>) -> Vec<AnyModel> {
		self.models.get_all_models(provider)
	}

	fn refresh(&self, options: Option<ModelsRefreshOptions>) -> LocalBoxFuture<'static, PiResult<ModelsRefreshResult>> {
		self.models.refresh(options)
	}

	fn check_auth(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<AuthCheck>>> {
		self.models.check_auth(provider_id, options)
	}

	fn get_available(
		&self,
		provider_id: Option<&str>,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Vec<Model>>> {
		self.models.get_available(provider_id, options)
	}

	fn get_available_of_type(
		&self,
		model_type: ModelType,
		provider_id: Option<&str>,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Vec<AnyModel>>> {
		self.models.get_available_of_type(model_type, provider_id, options)
	}

	fn get_all_available(
		&self,
		provider_id: Option<&str>,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Vec<AnyModel>>> {
		self.models.get_all_available(provider_id, options)
	}

	fn get_auth(
		&self,
		target: GetAuthTarget<'_>,
		overrides: Option<AuthResolutionOverrides>,
	) -> LocalBoxFuture<'static, PiResult<Option<AuthResult>>> {
		self.models.get_auth(target, overrides)
	}

	fn login(
		&self,
		provider_id: &str,
		r#type: AuthType,
		interaction: AuthInteraction,
		options: Option<LoginOptions>,
	) -> LocalBoxFuture<'static, PiResult<Credential>> {
		self.models.login(provider_id, r#type, interaction, options)
	}

	fn logout(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		self.models.logout(provider_id, options)
	}

	fn stream(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsApiStreamOptions>,
	) -> AssistantMessageEventStream {
		self.models
			.stream(model, context, with_identity_options!(self, options, base.base.base))
	}

	fn complete(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsApiStreamOptions>,
	) -> LocalBoxFuture<'static, Option<AssistantMessage>> {
		self.models
			.complete(model, context, with_identity_options!(self, options, base.base.base))
	}

	fn stream_simple(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsSimpleStreamOptions>,
	) -> AssistantMessageEventStream {
		self.models
			.stream_simple(model, context, with_identity_options!(self, options, base.base.base))
	}

	fn complete_simple(
		&self,
		model: &Model,
		context: &Context,
		options: Option<ModelsSimpleStreamOptions>,
	) -> LocalBoxFuture<'static, Option<AssistantMessage>> {
		self.models
			.complete_simple(model, context, with_identity_options!(self, options, base.base.base))
	}

	fn stream_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<ModelsDeferredFetchOptions>,
	) -> AssistantMessageEventStream {
		self.models
			.stream_deferred(model, handle, with_identity_options!(self, options, base.base))
	}

	fn fetch_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<ModelsDeferredFetchOptions>,
	) -> LocalBoxFuture<'static, Option<AssistantMessage>> {
		self.models
			.fetch_deferred(model, handle, with_identity_options!(self, options, base.base))
	}

	fn cancel_deferred(
		&self,
		model: &Model,
		handle: &DeferredHandle,
		options: Option<ModelsDeferredCancelOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		self.models
			.cancel_deferred(model, handle, with_identity_options!(self, options, base))
	}

	fn generate_images(
		&self,
		model: &ImageModel,
		context: &ImagesContext,
		options: Option<ModelsImagesOptions>,
	) -> LocalBoxFuture<'static, AssistantImages> {
		self.models
			.generate_images(model, context, with_identity_options!(self, options, base.base))
	}

	fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<ModelsClassifierOptions>,
	) -> LocalBoxFuture<'static, ClassifierResult> {
		self.models
			.classify(model, context, with_identity_options!(self, options, base.base))
	}
}

impl<T: MutableModels + ?Sized> MutableModels for WithClientIdentity<T> {
	fn set_provider(&self, provider: Rc<Provider>) {
		self.models.set_provider(provider);
	}

	fn delete_provider(&self, id: &str) {
		self.models.delete_provider(id);
	}

	fn clear_providers(&self) {
		self.models.clear_providers();
	}
}

/// `models` with `identity` as the default identity of its requests: a request's `identity` overrides it field by
/// field, and it overrides the identity `models` was created with. Everything else is delegated unchanged.
pub fn with_client_identity(models: Rc<dyn Models>, identity: ClientIdentity) -> Rc<dyn Models> {
	Rc::new(WithClientIdentity { models, identity })
}

/// `withClientIdentity(models: MutableModels, identity)`: a `MutableModels` stays mutable.
pub fn with_client_identity_mutable(models: Rc<dyn MutableModels>, identity: ClientIdentity) -> Rc<dyn MutableModels> {
	Rc::new(WithClientIdentity { models, identity })
}
