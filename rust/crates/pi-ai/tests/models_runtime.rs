//! Port of `packages/ai/test/models-runtime.test.ts`.
//!
//! The credential store, the default auth context, `resolve_provider_auth` and `refresh_stored_oauth_credential`
//! are `PORT-STUB: w2-ai-auth` on this branch. Tests whose TS only needs some store and context (refresh, login,
//! logout) get in-file doubles through `test_models()`: `TestCredentialStore`, a map with `InMemoryCredentialStore`'s
//! read/list/modify/delete results but no per-provider queue, and `TestAuthContext`, which has no env vars and no
//! files. Tests that exercise the real store or auth resolution keep the TS setup (`create_models(None)`,
//! `InMemoryCredentialStore::new()`) and are `#[ignore = "needs w2-ai-auth: ..."]`; they pass against the auth branch.
//!
//! Differences from the TS:
//! - "swallows provider source failures for both all-provider and single-provider listing" is not ported:
//!   `Provider.get_models` returns a `Vec` and cannot throw, so `Models` has no guard to test (see `models.rs`).
//! - "keeps chat reads independent from the all-model catalog" replaces the throwing `getAllModels` with one that
//!   lists nothing, for the same reason; a chat read that used it would still find no `model-a`.
//! - `toBeInstanceOf(AbortSignal)` holds by type, and so does `not.toHaveProperty("transformHeaders")`: the options a
//!   provider receives have no `transform_headers` field.
//! - Signal identity (`===`) is `AbortSignal::ptr_eq`; `vi.waitFor` is `wait_for`, which polls every 50 ms of host
//!   time for up to 1 s like vitest's defaults; `vi.fn` call counts are `Cell` counters; a promise created before it
//!   is awaited is `spawned`, so its work starts at once like the JS promise's.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::Shared;
use indexmap::IndexMap;
use pi_ai::auth::credential_store::InMemoryCredentialStore;
use pi_ai::auth::resolve::AuthResolutionOverrides;
use pi_ai::auth::types::{
	ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthCheck, AuthContext, AuthInteraction, AuthOperationOptions,
	AuthResult, AuthType, Credential, CredentialInfo, CredentialModifier, CredentialStore, ModelAuth, OAuthAuth,
	OAuthCredential, OAuthRefreshFunction, ProviderAuth,
};
use pi_ai::models::{
	CreateModelsOptions, CreateProviderOptions, GetAuthTarget, GetModelsFunction, ModelsError, ModelsErrorCode,
	ModelsPublication, ModelsRefreshOptions, ModelsSimpleStreamOptions, MutableModels, Provider, ProviderApi,
	RefreshModelsContext, RefreshModelsFunction, calculate_cost, create_models, create_provider, has_api,
};
use pi_ai::models_store::{InMemoryModelsStore, ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions};
use pi_ai::types::{
	AnyModel, AssistantContent, AssistantMessage, AssistantMessageEvent, BaseModel, Context, Message, Model, ModelCost,
	ModelCostTier, ModelInputModality, ProviderHeaders, ProviderStreamOptions, ProviderStreams, SimpleStreamOptions,
	StopReason, StreamOptions, TextContent, TranscriptContext, Usage, UsageCost, UserMessage, UserMessageContent,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::model_operations::ModelRef;
use pi_host::{AbortController, AbortSignal, LocalBoxFuture, PiError, PiResult};
use serde_json::Map;

fn test_model(provider: &str, id: &str) -> Model {
	Model {
		base: BaseModel {
			id: id.into(),
			name: id.into(),
			api: "test-api".into(),
			provider: provider.into(),
			base_url: "https://example.test/v1".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		reasoning: false,
		context_window: 10000.0,
		max_tokens: 1000.0,
		..Model::default()
	}
}

fn done_message(model: &Model, text: &str) -> AssistantMessage {
	AssistantMessage {
		content: vec![AssistantContent::Text(TextContent {
			text: text.into(),
			..TextContent::default()
		})],
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		usage: Usage::default(),
		stop_reason: StopReason::Stop,
		timestamp: pi_host::now_ms(),
		..AssistantMessage::default()
	}
}

struct ProviderCall {
	model: Model,
	options: Option<StreamOptions>,
}

type Calls = Rc<RefCell<Vec<ProviderCall>>>;

/// Ambient auth for keyless test providers; reports "configured" with no auth values.
fn ambient_auth() -> ApiKeyAuth {
	ApiKeyAuth {
		name: "Ambient".into(),
		login: None,
		check: None,
		resolve: Rc::new(|_| {
			Box::pin(async {
				Ok(Some(AuthResult {
					auth: ModelAuth::default(),
					..AuthResult::default()
				}))
			})
		}),
	}
}

#[derive(Default)]
struct TestProviderInput {
	id: &'static str,
	models: Option<Vec<Model>>,
	auth: Option<ProviderAuth>,
	get_models: Option<GetModelsFunction>,
	refresh_models: Option<RefreshModelsFunction>,
	calls: Option<Calls>,
}

fn test_provider(input: TestProviderInput) -> Provider {
	let models = input.models.unwrap_or_else(|| vec![test_model(input.id, "model-a")]);
	let calls = input.calls;
	let respond = Rc::new(move |model: &Model, options: Option<StreamOptions>| {
		if let Some(calls) = &calls {
			calls.borrow_mut().push(ProviderCall {
				model: model.clone(),
				options,
			});
		}
		let stream = AssistantMessageEventStream::new();
		let message = done_message(model, "ok");
		stream.push(AssistantMessageEvent::Start {
			partial: Rc::new(RefCell::new(message.clone())),
		});
		stream.push(AssistantMessageEvent::Done {
			reason: StopReason::Stop,
			message: Rc::new(RefCell::new(message.clone())),
		});
		stream.end(Some(message));
		stream
	});
	let simple_respond = respond.clone();
	Provider {
		id: input.id.into(),
		name: input.id.into(),
		base_url: None,
		headers: None,
		auth: input.auth.unwrap_or_else(|| ProviderAuth {
			api_key: Some(ambient_auth()),
			oauth: None,
		}),
		get_models: input.get_models.unwrap_or_else(|| Rc::new(move || models.clone())),
		get_all_models: None,
		refresh_models: input.refresh_models,
		filter_models: None,
		filter_all_models: None,
		stream: Rc::new(move |model, _context, options: Option<ProviderStreamOptions>| {
			Ok(respond(model, options.map(|options| options.base)))
		}),
		stream_simple: Rc::new(move |model, _context, options: Option<SimpleStreamOptions>| {
			Ok(simple_respond(model, options.map(|options| options.base)))
		}),
		fetch_deferred: None,
		cancel_deferred: None,
		generate_images: None,
		classify: None,
	}
}

fn context() -> Context {
	Context {
		messages: vec![Message::User(UserMessage {
			content: UserMessageContent::Text("hi".into()),
			timestamp: pi_host::now_ms(),
			..UserMessage::default()
		})],
		..Context::default()
	}
}

fn env_key_auth(key: Option<&str>) -> ApiKeyAuth {
	let key = key.map(str::to_string);
	ApiKeyAuth {
		name: "Test API key".into(),
		login: None,
		check: None,
		resolve: Rc::new(move |input| {
			let key = key.clone();
			Box::pin(async move {
				let stored = input.credential.is_some();
				let Some(resolved) = input.credential.and_then(|credential| credential.key).or(key) else {
					return Ok(None);
				};
				Ok(Some(AuthResult {
					auth: ModelAuth {
						api_key: Some(resolved),
						..ModelAuth::default()
					},
					env: None,
					source: Some(if stored { "stored" } else { "env" }.into()),
				}))
			})
		}),
	}
}

fn test_oauth(refresh: Option<OAuthRefreshFunction>) -> OAuthAuth {
	OAuthAuth {
		name: "Test OAuth".into(),
		is_subscription: None,
		login_label: None,
		login: Rc::new(|_, _| Box::pin(async { Err(PiError::new("not used")) })),
		refresh: refresh.unwrap_or_else(|| Rc::new(|credential, _| Box::pin(async move { Ok(credential) }))),
		to_auth: Rc::new(|credential| {
			Box::pin(async move {
				Ok(ModelAuth {
					api_key: Some(credential.access),
					..ModelAuth::default()
				})
			})
		}),
	}
}

fn refresh_models<F, Fut>(refresh: F) -> Option<RefreshModelsFunction>
where
	F: Fn(RefreshModelsContext) -> Fut + 'static,
	Fut: Future<Output = PiResult<()>> + 'static,
{
	Some(Rc::new(move |context| Box::pin(refresh(context))))
}

fn oauth_refresh<F, Fut>(refresh: F) -> Option<OAuthRefreshFunction>
where
	F: Fn(OAuthCredential) -> Fut + 'static,
	Fut: Future<Output = PiResult<OAuthCredential>> + 'static,
{
	Some(Rc::new(move |credential, _signal| Box::pin(refresh(credential))))
}

fn oauth_credential(access: &str, refresh: &str, expires: f64) -> OAuthCredential {
	OAuthCredential {
		access: access.into(),
		refresh: refresh.into(),
		expires,
		extra: Map::new(),
	}
}

fn api_key_credential(key: &str) -> Credential {
	Credential::ApiKey(ApiKeyCredential {
		key: Some(key.into()),
		env: None,
		extra: Default::default(),
	})
}

fn resolved_key(api_key: &str) -> Option<AuthResult> {
	Some(AuthResult {
		auth: ModelAuth {
			api_key: Some(api_key.into()),
			..ModelAuth::default()
		},
		..AuthResult::default()
	})
}

/// `credentials.modify(providerId, async () => credential)`.
async fn store_credential(credentials: &dyn CredentialStore, provider_id: &str, credential: Credential) {
	credentials
		.modify(
			provider_id,
			Box::new(move |_| Box::pin(async move { Ok(Some(credential)) })),
			None,
		)
		.await
		.unwrap();
}

/// The `access` and `refresh` of a stored OAuth credential.
fn stored_tokens(credential: Option<Credential>) -> (String, String) {
	match credential {
		Some(Credential::OAuth(credential)) => (credential.access, credential.refresh),
		other => panic!("expected a stored OAuth credential, got {other:?}"),
	}
}

fn chat(model: &Model) -> AnyModel {
	AnyModel::Chat(model.clone())
}

fn headers(entries: &[(&str, &str)]) -> ProviderHeaders {
	entries
		.iter()
		.map(|(name, value)| (name.to_string(), Some(value.to_string())))
		.collect()
}

fn with_signal(signal: AbortSignal) -> Option<AuthOperationOptions> {
	Some(AuthOperationOptions { signal: Some(signal) })
}

fn overrides_with_signal(signal: AbortSignal) -> Option<AuthResolutionOverrides> {
	Some(AuthResolutionOverrides {
		signal: Some(signal),
		..AuthResolutionOverrides::default()
	})
}

fn interaction(signal: Option<AbortSignal>) -> AuthInteraction {
	AuthInteraction {
		signal,
		prompt: Rc::new(|_| Box::pin(async { Ok("unused".to_string()) })),
		notify: Rc::new(|_| {}),
	}
}

fn simple_options(configure: impl FnOnce(&mut ModelsSimpleStreamOptions)) -> Option<ModelsSimpleStreamOptions> {
	let mut options = ModelsSimpleStreamOptions::default();
	configure(&mut options);
	Some(options)
}

/// `setTimeout(resolve, 0)`.
async fn next_timer_tick() {
	pi_host::sleep(0.0, None).await.unwrap();
}

/// `vi.waitFor(check)` with vitest's defaults: retry every 50 ms for up to 1 s.
async fn wait_for(check: impl Fn() -> LocalBoxFuture<'static, bool>) {
	for _ in 0..20 {
		if check().await {
			return;
		}
		pi_host::sleep(50.0, None).await.unwrap();
	}
	assert!(check().await, "wait_for timed out");
}

/// Starts `future` now, like a JS promise that runs before it is awaited.
fn spawned<T: 'static>(future: LocalBoxFuture<'static, T>) -> impl Future<Output = T> + 'static {
	let (sender, receiver) = oneshot::channel();
	pi_host::spawn_local(async move {
		let _ = sender.send(future.await);
	});
	async move { receiver.await.expect("the spawned future ran to completion") }
}

/// A promise with its resolve function taken out: `wait()` settles once `open()` runs.
struct Gate {
	sender: RefCell<Option<oneshot::Sender<()>>>,
	receiver: Shared<oneshot::Receiver<()>>,
}

impl Gate {
	fn new() -> Rc<Self> {
		let (sender, receiver) = oneshot::channel();
		Rc::new(Self {
			sender: RefCell::new(Some(sender)),
			receiver: receiver.shared(),
		})
	}

	fn open(&self) {
		if let Some(sender) = self.sender.borrow_mut().take() {
			let _ = sender.send(());
		}
	}

	fn wait(&self) -> impl Future<Output = ()> + 'static {
		let receiver = self.receiver.clone();
		async move {
			let _ = receiver.await;
		}
	}
}

/// Stand-in for `InMemoryCredentialStore` while it is a stub: same results, no per-provider queue.
#[derive(Default)]
struct TestCredentialStore {
	credentials: Rc<RefCell<IndexMap<String, Credential>>>,
}

fn throw_if_aborted(options: &Option<AuthOperationOptions>) -> PiResult<()> {
	match options.as_ref().and_then(|options| options.signal.as_ref()) {
		Some(signal) => signal.throw_if_aborted(),
		None => Ok(()),
	}
}

impl CredentialStore for TestCredentialStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		let result = throw_if_aborted(&options).map(|()| self.credentials.borrow().get(provider_id).cloned());
		Box::pin(async move { result })
	}

	fn list(&self, options: Option<AuthOperationOptions>) -> LocalBoxFuture<'static, PiResult<Vec<CredentialInfo>>> {
		let result = throw_if_aborted(&options).map(|()| {
			self.credentials
				.borrow()
				.iter()
				.map(|(provider_id, credential)| CredentialInfo {
					provider_id: provider_id.clone(),
					r#type: credential.r#type(),
				})
				.collect()
		});
		Box::pin(async move { result })
	}

	fn modify(
		&self,
		provider_id: &str,
		f: CredentialModifier,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		let (credentials, provider_id) = (self.credentials.clone(), provider_id.to_string());
		Box::pin(async move {
			throw_if_aborted(&options)?;
			let current = credentials.borrow().get(&provider_id).cloned();
			let next = f(current.clone()).await?;
			throw_if_aborted(&options)?;
			if let Some(next) = &next {
				credentials.borrow_mut().insert(provider_id, next.clone());
			}
			Ok(next.or(current))
		})
	}

	fn delete(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let result = throw_if_aborted(&options).map(|()| {
			self.credentials.borrow_mut().shift_remove(provider_id);
		});
		Box::pin(async move { result })
	}
}

/// Stand-in for the default auth context while it is a stub: no env vars, no files.
struct TestAuthContext;

impl AuthContext for TestAuthContext {
	fn env(&self, _name: &str) -> LocalBoxFuture<'static, Option<String>> {
		Box::pin(async { None })
	}

	fn file_exists(&self, _path: &str) -> LocalBoxFuture<'static, bool> {
		Box::pin(async { false })
	}
}

/// `createModels(options)` with the stand-in store and context where the options leave them out.
fn test_models(options: Option<CreateModelsOptions>) -> Rc<dyn MutableModels> {
	let mut options = options.unwrap_or_default();
	options
		.credentials
		.get_or_insert_with(|| Rc::new(TestCredentialStore::default()));
	options.auth_context.get_or_insert_with(|| Rc::new(TestAuthContext));
	create_models(Some(options))
}

struct EmptyStreams;

impl ProviderStreams for EmptyStreams {
	fn stream(
		&self,
		_model: &Model,
		_context: &TranscriptContext,
		_options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(AssistantMessageEventStream::new())
	}

	fn stream_simple(
		&self,
		_model: &Model,
		_context: &TranscriptContext,
		_options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		Ok(AssistantMessageEventStream::new())
	}
}

/// A models store over one shared entry, as the TS object literal closes over `let entry`.
struct EntryStore {
	entry: Rc<RefCell<Option<ModelsStoreEntry>>>,
}

impl ModelsStore for EntryStore {
	fn read(
		&self,
		_provider_id: &str,
		_options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<ModelsStoreEntry>>> {
		let entry = self.entry.borrow().clone();
		Box::pin(async move { Ok(entry) })
	}

	fn write(
		&self,
		_provider_id: &str,
		entry: ModelsStoreEntry,
		_options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		*self.entry.borrow_mut() = Some(entry);
		Box::pin(async { Ok(()) })
	}

	fn delete(
		&self,
		_provider_id: &str,
		_options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		*self.entry.borrow_mut() = None;
		Box::pin(async { Ok(()) })
	}
}

/// A models store that records the signal of every call.
struct SignalStore {
	signals: Rc<RefCell<Vec<Option<AbortSignal>>>>,
}

impl SignalStore {
	fn record(&self, options: Option<ModelsStoreOperationOptions>) {
		self.signals
			.borrow_mut()
			.push(options.and_then(|options| options.signal));
	}
}

impl ModelsStore for SignalStore {
	fn read(
		&self,
		_provider_id: &str,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<ModelsStoreEntry>>> {
		self.record(options);
		Box::pin(async { Ok(None) })
	}

	fn write(
		&self,
		_provider_id: &str,
		_entry: ModelsStoreEntry,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		self.record(options);
		Box::pin(async { Ok(()) })
	}

	fn delete(
		&self,
		_provider_id: &str,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		self.record(options);
		Box::pin(async { Ok(()) })
	}
}

/// Counts `modify` calls on an `InMemoryCredentialStore`; like the TS wrapper, it drops the options.
struct CountingStore {
	base: InMemoryCredentialStore,
	modifies: Rc<Cell<u32>>,
}

impl CredentialStore for CountingStore {
	fn read(
		&self,
		provider_id: &str,
		_options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		self.base.read(provider_id, None)
	}

	fn list(&self, _options: Option<AuthOperationOptions>) -> LocalBoxFuture<'static, PiResult<Vec<CredentialInfo>>> {
		self.base.list(None)
	}

	fn modify(
		&self,
		provider_id: &str,
		f: CredentialModifier,
		_options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		self.modifies.set(self.modifies.get() + 1);
		self.base.modify(provider_id, f, None)
	}

	fn delete(
		&self,
		provider_id: &str,
		_options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		self.base.delete(provider_id, None)
	}
}

/// A credential store whose `read` fails.
struct ReadFailingStore;

impl CredentialStore for ReadFailingStore {
	fn read(
		&self,
		_provider_id: &str,
		_options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		Box::pin(async { Err(PiError::new("disk on fire")) })
	}

	fn list(&self, _options: Option<AuthOperationOptions>) -> LocalBoxFuture<'static, PiResult<Vec<CredentialInfo>>> {
		Box::pin(async { Ok(Vec::new()) })
	}

	fn modify(
		&self,
		_provider_id: &str,
		_f: CredentialModifier,
		_options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		Box::pin(async { Ok(None) })
	}

	fn delete(
		&self,
		_provider_id: &str,
		_options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		Box::pin(async { Ok(()) })
	}
}

/// A credential store holding an expired OAuth credential whose `modify` fails.
struct ModifyFailingStore;

impl CredentialStore for ModifyFailingStore {
	fn read(
		&self,
		_provider_id: &str,
		_options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		Box::pin(async { Ok(Some(Credential::OAuth(oauth_credential("old", "r", 0.0)))) })
	}

	fn list(&self, _options: Option<AuthOperationOptions>) -> LocalBoxFuture<'static, PiResult<Vec<CredentialInfo>>> {
		Box::pin(async {
			Ok(vec![CredentialInfo {
				provider_id: "p1".into(),
				r#type: AuthType::OAuth,
			}])
		})
	}

	fn modify(
		&self,
		_provider_id: &str,
		_f: CredentialModifier,
		_options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		Box::pin(async { Err(PiError::new("disk on fire")) })
	}

	fn delete(
		&self,
		_provider_id: &str,
		_options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		Box::pin(async { Ok(()) })
	}
}

mod models_runtime {
	use super::*;

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore"]
	fn enumerates_credential_metadata_without_exposing_secrets() {
		pi_host::testing::run(|_| async {
			let credentials = InMemoryCredentialStore::new();
			store_credential(&credentials, "api-provider", api_key_credential("secret")).await;
			store_credential(
				&credentials,
				"oauth-provider",
				Credential::OAuth(oauth_credential("access", "refresh", pi_host::now_ms() + 60_000.0)),
			)
			.await;

			assert_eq!(
				credentials.list(None).await.unwrap(),
				vec![
					CredentialInfo {
						provider_id: "api-provider".into(),
						r#type: AuthType::ApiKey,
					},
					CredentialInfo {
						provider_id: "oauth-provider".into(),
						r#type: AuthType::OAuth,
					},
				]
			);
		});
	}

	#[test]
	fn applies_request_wide_pricing_tiers_above_the_configured_input_threshold() {
		let mut model = test_model("openai", "gpt-5.6-sol");
		model.base.cost = ModelCost {
			input: 5.0,
			output: 30.0,
			cache_read: 0.5,
			cache_write: 6.25,
			tiers: Some(vec![ModelCostTier {
				input_tokens_above: 272000.0,
				input: 10.0,
				output: 45.0,
				cache_read: 1.0,
				cache_write: 12.5,
				..ModelCostTier::default()
			}]),
			..ModelCost::default()
		};
		let create_usage = |cache_write: f64| Usage {
			input: 200000.0,
			output: 100000.0,
			cache_read: 72000.0,
			cache_write,
			total_tokens: 372000.0 + cache_write,
			cost: UsageCost::default(),
			..Usage::default()
		};

		let short = calculate_cost(&model, &mut create_usage(0.0));
		assert_eq!(
			(short.input, short.output, short.cache_read, short.cache_write),
			(1.0, 3.0, 0.036, 0.0)
		);

		let long = calculate_cost(&model, &mut create_usage(1.0));
		assert_eq!(long.input, 2.0);
		assert_eq!(long.output, 4.5);
		assert_eq!(long.cache_read, 0.072);
		assert_eq!(long.cache_write, 0.0000125);
	}

	#[test]
	fn marks_catalog_derived_totals_as_not_provider_reported() {
		let mut usage = Usage {
			input: 1_000_000.0,
			output: 0.0,
			cache_read: 0.0,
			cache_write: 0.0,
			total_tokens: 1_000_000.0,
			cost: UsageCost {
				total: 9.0,
				total_from_provider: Some(true),
				..UsageCost::default()
			},
			..Usage::default()
		};
		calculate_cost(&test_model("openai", "gpt-5.6-sol"), &mut usage);
		assert_eq!(usage.cost.total_from_provider, Some(false));
	}

	#[test]
	fn registers_replaces_and_deletes_providers() {
		let models = create_models(None);
		models.set_provider(Rc::new(test_provider(TestProviderInput {
			id: "p1",
			..TestProviderInput::default()
		})));
		models.set_provider(Rc::new(test_provider(TestProviderInput {
			id: "p2",
			..TestProviderInput::default()
		})));
		assert_eq!(
			models
				.get_providers()
				.iter()
				.map(|provider| provider.id.clone())
				.collect::<Vec<_>>(),
			["p1", "p2"]
		);

		let replacement = Rc::new(test_provider(TestProviderInput {
			id: "p1",
			..TestProviderInput::default()
		}));
		models.set_provider(replacement.clone());
		assert!(Rc::ptr_eq(&models.get_provider("p1").unwrap(), &replacement));
		assert_eq!(models.get_providers().len(), 2);

		models.delete_provider("p1");
		assert!(models.get_provider("p1").is_none());

		models.clear_providers();
		assert_eq!(models.get_providers().len(), 0);
	}

	#[test]
	fn lists_and_finds_models_per_provider() {
		let models = create_models(None);
		models.set_provider(Rc::new(test_provider(TestProviderInput {
			id: "p1",
			models: Some(vec![test_model("p1", "m1"), test_model("p1", "m2")]),
			..TestProviderInput::default()
		})));
		models.set_provider(Rc::new(test_provider(TestProviderInput {
			id: "p2",
			models: Some(vec![test_model("p2", "m3")]),
			..TestProviderInput::default()
		})));

		let ids = |list: Vec<Model>| list.into_iter().map(|model| model.base.id).collect::<Vec<_>>();
		assert_eq!(ids(models.get_models(None)), ["m1", "m2", "m3"]);
		assert_eq!(ids(models.get_models(Some("p1"))), ["m1", "m2"]);
		assert_eq!(models.get_models(Some("nope")).len(), 0);
		assert_eq!(
			models.get_model("p2", "m3").map(|model| model.base.id).as_deref(),
			Some("m3")
		);
		assert!(models.get_model("p2", "missing").is_none());

		// hasApi() narrows dynamically looked-up models with a runtime check
		let found = models.get_model("p2", "m3").map(|model| chat(&model));
		assert!(
			found
				.as_ref()
				.is_some_and(|found| has_api(found, "openai-completions").is_none())
		);
		assert!(found.as_ref().is_some_and(|found| has_api(found, "test-api").is_some()));
		if let Some(typed) = found.as_ref().and_then(|found| has_api(found, "test-api")) {
			assert_eq!(typed.id, "m3");
		}
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn keeps_chat_reads_independent_from_the_all_model_catalog() {
		pi_host::testing::run(|_| async {
			let mut provider = test_provider(TestProviderInput {
				id: "chat-only",
				..TestProviderInput::default()
			});
			provider.get_all_models = Some(Rc::new(Vec::new));
			let models = create_models(None);
			models.set_provider(Rc::new(provider));

			let ids = |list: Vec<Model>| list.into_iter().map(|model| model.base.id).collect::<Vec<_>>();
			assert_eq!(ids(models.get_models(Some("chat-only"))), ["model-a"]);
			assert_eq!(
				models
					.get_model("chat-only", "model-a")
					.map(|model| model.base.id)
					.as_deref(),
				Some("model-a")
			);
			assert_eq!(
				ids(models.get_available(Some("chat-only"), None).await.unwrap()),
				["model-a"]
			);
			assert_eq!(models.get_all_models(Some("chat-only")), Vec::<AnyModel>::new());
		});
	}

	#[test]
	fn refresh_updates_every_configured_dynamic_provider_and_reports_failures() {
		pi_host::testing::run(|_| async {
			let list = Rc::new(RefCell::new(vec![test_model("dyn", "before")]));
			let refreshes = Rc::new(Cell::new(0));
			let models = test_models(None);
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "dyn",
				get_models: Some({
					let list = list.clone();
					Rc::new(move || list.borrow().clone())
				}),
				refresh_models: {
					let (list, refreshes) = (list.clone(), refreshes.clone());
					refresh_models(move |refresh| {
						let (list, refreshes) = (list.clone(), refreshes.clone());
						async move {
							if !refresh.allow_network {
								return Ok(());
							}
							refreshes.set(refreshes.get() + 1);
							(refresh.publish)(ModelsPublication {
								persist: None,
								update: Some(Rc::new(move || {
									*list.borrow_mut() = vec![test_model("dyn", "after")];
								})),
							})
							.await?;
							Ok(())
						}
					})
				},
				..TestProviderInput::default()
			})));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "static",
				models: Some(vec![test_model("static", "s1")]),
				..TestProviderInput::default()
			})));

			assert!(models.get_model("dyn", "before").is_some());
			let first = models.refresh(None).await.unwrap();
			assert_eq!(first.errors.len(), 0);
			assert_eq!(refreshes.get(), 1);
			assert!(models.get_model("dyn", "after").is_some());
			assert!(models.get_model("dyn", "before").is_none());

			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "flaky",
				refresh_models: refresh_models(|refresh| async move {
					if refresh.allow_network {
						return Err(PiError::new("fetch failed"));
					}
					Ok(())
				}),
				..TestProviderInput::default()
			})));
			let second = models.refresh(None).await.unwrap();
			assert_eq!(refreshes.get(), 2);
			assert_eq!(
				second.errors.get("flaky").map(|error| error.message.as_str()),
				Some("fetch failed")
			);
		});
	}

	#[test]
	fn restricts_refresh_work_to_selected_providers() {
		pi_host::testing::run(|_| async {
			let calls = Rc::new(RefCell::new(Vec::<String>::new()));
			let models = test_models(None);
			for id in ["one", "two"] {
				let calls = calls.clone();
				models.set_provider(Rc::new(test_provider(TestProviderInput {
					id,
					refresh_models: refresh_models(move |refresh| {
						calls.borrow_mut().push(format!(
							"{id}:{}",
							if refresh.allow_network { "network" } else { "cache" }
						));
						async { Ok(()) }
					}),
					..TestProviderInput::default()
				})));
			}

			let result = models
				.refresh(Some(ModelsRefreshOptions {
					providers: Some(vec!["two".into(), "unknown".into()]),
					..ModelsRefreshOptions::default()
				}))
				.await
				.unwrap();

			assert_eq!(result.errors.len(), 0);
			assert_eq!(*calls.borrow(), ["two:cache", "two:network"]);
		});
	}

	#[test]
	fn restores_cached_models_before_waiting_for_network_auth() {
		pi_host::testing::run(|_| async {
			let store = Rc::new(InMemoryModelsStore::new());
			store
				.write(
					"dynamic",
					ModelsStoreEntry {
						models: vec![chat(&test_model("dynamic", "cached"))],
						..ModelsStoreEntry::default()
					},
					None,
				)
				.await
				.unwrap();
			let auth_started = Gate::new();
			let blocked_auth = Gate::new();
			let provider = create_provider(CreateProviderOptions {
				id: "dynamic".into(),
				auth: ProviderAuth {
					api_key: Some(ApiKeyAuth {
						name: "Blocked auth".into(),
						login: None,
						check: None,
						resolve: {
							let (auth_started, blocked_auth) = (auth_started.clone(), blocked_auth.clone());
							Rc::new(move |_| {
								auth_started.open();
								let blocked = blocked_auth.wait();
								Box::pin(async move {
									blocked.await;
									Ok(resolved_key("key"))
								})
							})
						},
					}),
					oauth: None,
				},
				models: Vec::new(),
				fetch_models: Some(Rc::new(|_| Box::pin(async { Err(PiError::new("must not fetch")) }))),
				api: Some(ProviderApi::Single(Rc::new(EmptyStreams))),
				..CreateProviderOptions::default()
			})
			.unwrap();
			let models = test_models(Some(CreateModelsOptions {
				models_store: Some(store.clone() as Rc<dyn ModelsStore>),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(provider));
			let controller = AbortController::new();
			let pending = spawned(models.refresh(Some(ModelsRefreshOptions {
				providers: Some(vec!["dynamic".into()]),
				signal: Some(controller.signal()),
				..ModelsRefreshOptions::default()
			})));
			auth_started.wait().await;

			assert!(models.get_model("dynamic", "cached").is_some());
			controller.abort();
			assert!(pending.await.unwrap().aborted);
			blocked_auth.open();
		});
	}

	#[test]
	fn lets_providers_choose_persistent_deletion_and_ephemeral_publication_atomically() {
		pi_host::testing::run(|_| async {
			let entry = Rc::new(RefCell::new(Some(ModelsStoreEntry {
				models: vec![chat(&test_model("dynamic", "stored"))],
				..ModelsStoreEntry::default()
			})));
			let store = EntryStore { entry: entry.clone() };
			let state = Rc::new(RefCell::new("initial"));
			let models = test_models(Some(CreateModelsOptions {
				models_store: Some(Rc::new(store)),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "dynamic",
				refresh_models: {
					let (entry, state) = (entry.clone(), state.clone());
					refresh_models(move |context| {
						let (entry, state) = (entry.clone(), state.clone());
						async move {
							assert_eq!(
								context
									.stored
									.as_ref()
									.and_then(|stored| stored.models.first())
									.map(|model| model.model_id()),
								Some("stored")
							);
							let deleted = state.clone();
							(context.publish)(ModelsPublication {
								persist: Some(None),
								update: Some(Rc::new(move || {
									assert!(entry.borrow().is_none());
									*deleted.borrow_mut() = "deleted";
								})),
							})
							.await?;
							(context.publish)(ModelsPublication {
								persist: None,
								update: Some(Rc::new(move || {
									*state.borrow_mut() = "ephemeral";
								})),
							})
							.await?;
							Ok(())
						}
					})
				},
				..TestProviderInput::default()
			})));

			let result = models
				.refresh(Some(ModelsRefreshOptions {
					allow_network: Some(false),
					..ModelsRefreshOptions::default()
				}))
				.await
				.unwrap();

			assert_eq!(result.errors.len(), 0);
			assert!(entry.borrow().is_none());
			assert_eq!(*state.borrow(), "ephemeral");
		});
	}

	#[test]
	fn persists_dynamic_catalogs_and_restores_them_without_network_access() {
		pi_host::testing::run(|_| async {
			let credentials: Rc<dyn CredentialStore> = Rc::new(TestCredentialStore::default());
			let models_store: Rc<dyn ModelsStore> = Rc::new(InMemoryModelsStore::new());
			store_credential(&*credentials, "dynamic", api_key_credential("key")).await;
			let create_dynamic_provider = |fetched: Option<Vec<AnyModel>>| {
				create_provider(CreateProviderOptions {
					id: "dynamic".into(),
					auth: ProviderAuth {
						api_key: Some(env_key_auth(None)),
						oauth: None,
					},
					models: Vec::new(),
					fetch_models: Some(Rc::new(move |_| {
						let fetched = fetched.clone();
						Box::pin(async move { fetched.ok_or_else(|| PiError::new("must not fetch")) })
					})),
					api: Some(ProviderApi::Single(Rc::new(EmptyStreams))),
					..CreateProviderOptions::default()
				})
				.unwrap()
			};
			let options = CreateModelsOptions {
				credentials: Some(credentials.clone()),
				models_store: Some(models_store.clone()),
				..CreateModelsOptions::default()
			};

			let online = test_models(Some(options.clone()));
			online.set_provider(Rc::new(create_dynamic_provider(Some(vec![chat(&test_model(
				"dynamic", "fetched",
			))]))));
			assert_eq!(online.refresh(None).await.unwrap().errors.len(), 0);
			assert!(online.get_model("dynamic", "fetched").is_some());

			let offline = test_models(Some(options));
			offline.set_provider(Rc::new(create_dynamic_provider(None)));
			let restored = offline
				.refresh(Some(ModelsRefreshOptions {
					allow_network: Some(false),
					..ModelsRefreshOptions::default()
				}))
				.await
				.unwrap();
			assert_eq!(restored.errors.len(), 0);
			assert!(offline.get_model("dynamic", "fetched").is_some());
		});
	}

	#[test]
	fn passes_effective_api_key_credentials_and_refresh_options_while_skipping_unconfigured_providers() {
		pi_host::testing::run(|_| async {
			let effective_credential = Rc::new(RefCell::new(None::<Option<Credential>>));
			let force_refresh = Rc::new(Cell::new(None::<bool>));
			let unconfigured_refreshes = Rc::new(Cell::new(0));
			let models = test_models(None);
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "configured",
				auth: Some(ProviderAuth {
					api_key: Some(env_key_auth(Some("ambient-key"))),
					oauth: None,
				}),
				refresh_models: {
					let (effective_credential, force_refresh) = (effective_credential.clone(), force_refresh.clone());
					refresh_models(move |context| {
						if context.allow_network {
							*effective_credential.borrow_mut() = Some(context.credential.clone());
							force_refresh.set(context.force);
						}
						async { Ok(()) }
					})
				},
				..TestProviderInput::default()
			})));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "unconfigured",
				auth: Some(ProviderAuth {
					api_key: Some(env_key_auth(None)),
					oauth: None,
				}),
				refresh_models: {
					let unconfigured_refreshes = unconfigured_refreshes.clone();
					refresh_models(move |context| {
						if context.allow_network {
							unconfigured_refreshes.set(unconfigured_refreshes.get() + 1);
						}
						async { Ok(()) }
					})
				},
				..TestProviderInput::default()
			})));

			models
				.refresh(Some(ModelsRefreshOptions {
					force: Some(true),
					..ModelsRefreshOptions::default()
				}))
				.await
				.unwrap();
			assert_eq!(
				*effective_credential.borrow(),
				Some(Some(Credential::ApiKey(ApiKeyCredential {
					key: Some("ambient-key".into()),
					env: None,
					extra: Default::default(),
				})))
			);
			assert_eq!(force_refresh.get(), Some(true));
			assert_eq!(unconfigured_refreshes.get(), 0);
		});
	}

	#[test]
	fn passes_the_registered_providers_base_url_including_wrapper_overrides_to_refresh_models() {
		pi_host::testing::run(|_| async {
			// A models.json baseUrl (e.g. a regional OpenRouter host) is applied by a wrapper around the
			// built-in provider. The built-in refresh must still request that URL.
			let base_urls = Rc::new(RefCell::new(Vec::<Option<String>>::new()));
			let inner = Provider {
				base_url: Some("https://default.example.test/v1".into()),
				..test_provider(TestProviderInput {
					id: "dynamic",
					refresh_models: {
						let base_urls = base_urls.clone();
						refresh_models(move |context| {
							base_urls.borrow_mut().push(context.base_url.clone());
							async { Ok(()) }
						})
					},
					..TestProviderInput::default()
				})
			};
			let models = test_models(None);
			models.set_provider(Rc::new(Provider {
				base_url: Some("https://regional.example.test/v1".into()),
				..inner
			}));

			models.refresh(None).await.unwrap();
			assert_eq!(
				*base_urls.borrow(),
				[
					Some("https://regional.example.test/v1".to_string()),
					Some("https://regional.example.test/v1".to_string())
				]
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, refresh_stored_oauth_credential"]
	fn refreshes_expired_oauth_before_refreshing_models() {
		pi_host::testing::run(|_| async {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			let model_refresh_credential = Rc::new(RefCell::new(None::<Credential>));
			store_credential(
				&*credentials,
				"oauth-dynamic",
				Credential::OAuth(oauth_credential("expired", "refresh", 0.0)),
			)
			.await;
			let models = create_models(Some(CreateModelsOptions {
				credentials: Some(credentials.clone() as Rc<dyn CredentialStore>),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "oauth-dynamic",
				auth: Some(ProviderAuth {
					api_key: None,
					oauth: Some(test_oauth(oauth_refresh(|_| async {
						Ok(oauth_credential("fresh", "rotated", pi_host::now_ms() + 60_000.0))
					}))),
				}),
				refresh_models: {
					let model_refresh_credential = model_refresh_credential.clone();
					refresh_models(move |context| {
						if context.allow_network {
							*model_refresh_credential.borrow_mut() = context.credential.clone();
						}
						async { Ok(()) }
					})
				},
				..TestProviderInput::default()
			})));

			assert_eq!(models.refresh(None).await.unwrap().errors.len(), 0);
			assert_eq!(
				stored_tokens(model_refresh_credential.borrow().clone()),
				("fresh".to_string(), "rotated".to_string())
			);
			assert_eq!(
				stored_tokens(credentials.read("oauth-dynamic", None).await.unwrap()),
				("fresh".to_string(), "rotated".to_string())
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, refresh_stored_oauth_credential"]
	fn persists_an_oauth_refresh_that_started_before_the_model_refresh_was_cancelled() {
		pi_host::testing::run(|_| async {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			store_credential(
				&*credentials,
				"p1",
				Credential::OAuth(oauth_credential("old", "old-refresh", 0.0)),
			)
			.await;
			let controller = AbortController::new();
			let models = create_models(Some(CreateModelsOptions {
				credentials: Some(credentials.clone() as Rc<dyn CredentialStore>),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				auth: Some(ProviderAuth {
					api_key: None,
					oauth: Some(test_oauth({
						let controller = controller.clone();
						oauth_refresh(move |credential| {
							controller.abort();
							async move {
								Ok(OAuthCredential {
									access: "new".into(),
									refresh: "new-refresh".into(),
									expires: pi_host::now_ms() + 60_000.0,
									..credential
								})
							}
						})
					})),
				}),
				refresh_models: refresh_models(|_| async { Ok(()) }),
				..TestProviderInput::default()
			})));

			let result = models
				.refresh(Some(ModelsRefreshOptions {
					signal: Some(controller.signal()),
					..ModelsRefreshOptions::default()
				}))
				.await
				.unwrap();
			assert!(result.aborted);
			wait_for(|| {
				let read = credentials.read("p1", None);
				Box::pin(async move { stored_tokens(read.await.unwrap()).1 == "new-refresh" })
			})
			.await;
		});
	}

	#[test]
	fn always_gives_providers_a_concrete_signal() {
		pi_host::testing::run(|_| async {
			let received_signal = Rc::new(RefCell::new(None::<AbortSignal>));
			let models = test_models(None);
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "dynamic",
				refresh_models: {
					let received_signal = received_signal.clone();
					refresh_models(move |context| {
						*received_signal.borrow_mut() = Some(context.signal.clone());
						async { Ok(()) }
					})
				},
				..TestProviderInput::default()
			})));

			let result = models.refresh(None).await.unwrap();
			assert!(!result.aborted);
			assert!(received_signal.borrow().is_some());
			assert_eq!(received_signal.borrow().as_ref().map(AbortSignal::aborted), Some(false));
		});
	}

	#[test]
	fn binds_model_store_waits_to_the_provider_refresh_signal() {
		pi_host::testing::run(|_| async {
			let storage_signals = Rc::new(RefCell::new(Vec::new()));
			let store = SignalStore {
				signals: storage_signals.clone(),
			};
			let provider_signal = Rc::new(RefCell::new(None::<AbortSignal>));
			let models = test_models(Some(CreateModelsOptions {
				models_store: Some(Rc::new(store)),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "dynamic",
				auth: Some(ProviderAuth {
					api_key: Some(env_key_auth(Some("key"))),
					oauth: None,
				}),
				refresh_models: {
					let provider_signal = provider_signal.clone();
					refresh_models(move |context| {
						*provider_signal.borrow_mut() = Some(context.signal.clone());
						async move {
							if !context.allow_network {
								return Ok(());
							}
							(context.publish)(ModelsPublication {
								persist: Some(Some(ModelsStoreEntry {
									models: vec![chat(&test_model("dynamic", "fresh"))],
									..ModelsStoreEntry::default()
								})),
								update: None,
							})
							.await?;
							Ok(())
						}
					})
				},
				..TestProviderInput::default()
			})));

			let result = models
				.refresh(Some(ModelsRefreshOptions {
					providers: Some(vec!["dynamic".into()]),
					..ModelsRefreshOptions::default()
				}))
				.await
				.unwrap();

			assert_eq!(result.errors.len(), 0);
			assert_eq!(storage_signals.borrow().len(), 3);
			let provider_signal = provider_signal.borrow().clone().unwrap();
			assert!(storage_signals.borrow().iter().all(|signal| {
				signal
					.as_ref()
					.is_some_and(|signal| AbortSignal::ptr_eq(signal, &provider_signal))
			}));
		});
	}

	#[test]
	fn returns_aborted_state_without_reporting_cancellation_as_a_provider_error() {
		pi_host::testing::run(|_| async {
			let controller = AbortController::new();
			let models = test_models(None);
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "dynamic",
				refresh_models: {
					let controller = controller.clone();
					refresh_models(move |context| {
						controller.abort();
						let aborted = context.signal.aborted();
						async move {
							if aborted {
								return Ok(());
							}
							Ok(())
						}
					})
				},
				..TestProviderInput::default()
			})));

			let result = models
				.refresh(Some(ModelsRefreshOptions {
					signal: Some(controller.signal()),
					..ModelsRefreshOptions::default()
				}))
				.await
				.unwrap();
			assert!(result.aborted);
			assert_eq!(result.errors.len(), 0);
		});
	}

	#[test]
	fn stops_waiting_on_abort_when_a_provider_ignores_its_signal() {
		pi_host::testing::run(|_| async {
			let controller = AbortController::new();
			let started = Gate::new();
			let (reject_refresh, stalled) = oneshot::channel::<PiError>();
			let stalled = Rc::new(RefCell::new(Some(stalled)));
			let calls = Rc::new(Cell::new(0));
			let models = test_models(None);
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "dynamic",
				refresh_models: {
					let (started, calls) = (started.clone(), calls.clone());
					refresh_models(move |_| {
						calls.set(calls.get() + 1);
						let stalled = if calls.get() == 1 {
							stalled.borrow_mut().take()
						} else {
							None
						};
						let started = started.clone();
						async move {
							let Some(stalled) = stalled else {
								return Ok(());
							};
							started.open();
							match stalled.await {
								Ok(error) => Err(error),
								Err(_) => Ok(()),
							}
						}
					})
				},
				..TestProviderInput::default()
			})));

			let pending = spawned(models.refresh(Some(ModelsRefreshOptions {
				signal: Some(controller.signal()),
				..ModelsRefreshOptions::default()
			})));
			started.wait().await;
			controller.abort();

			let result = pending.await.unwrap();
			assert!(result.aborted);
			assert_eq!(result.errors.len(), 0);

			let _ = reject_refresh.send(PiError::new("late provider failure"));
			next_timer_tick().await;
			assert_eq!(result.errors.len(), 0);
		});
	}

	#[test]
	fn rejects_late_publication_from_a_superseded_non_cooperative_provider() {
		pi_host::testing::run(|_| async {
			let store = Rc::new(InMemoryModelsStore::new());
			let state = Rc::new(RefCell::new("initial".to_string()));
			let calls = Rc::new(Cell::new(0));
			let first_started = Gate::new();
			let first_blocked = Gate::new();
			let models = test_models(Some(CreateModelsOptions {
				models_store: Some(store.clone() as Rc<dyn ModelsStore>),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "dynamic",
				refresh_models: {
					let (state, calls) = (state.clone(), calls.clone());
					let (first_started, first_blocked) = (first_started.clone(), first_blocked.clone());
					refresh_models(move |context| {
						let (state, calls) = (state.clone(), calls.clone());
						let (first_started, first_blocked) = (first_started.clone(), first_blocked.clone());
						async move {
							if !context.allow_network {
								return Ok(());
							}
							calls.set(calls.get() + 1);
							let current = calls.get();
							if current == 1 {
								first_started.open();
								first_blocked.wait().await;
							}
							let value = format!("generation-{current}");
							let update_value = value.clone();
							(context.publish)(ModelsPublication {
								persist: Some(Some(ModelsStoreEntry {
									models: vec![chat(&test_model("dynamic", &value))],
									..ModelsStoreEntry::default()
								})),
								update: Some(Rc::new(move || {
									*state.borrow_mut() = update_value.clone();
								})),
							})
							.await?;
							Ok(())
						}
					})
				},
				..TestProviderInput::default()
			})));

			let only_dynamic = || {
				Some(ModelsRefreshOptions {
					providers: Some(vec!["dynamic".into()]),
					..ModelsRefreshOptions::default()
				})
			};
			let first = spawned(models.refresh(only_dynamic()));
			first_started.wait().await;
			let second = models.refresh(only_dynamic());
			second.await.unwrap();
			first.await.unwrap();
			first_blocked.open();
			next_timer_tick().await;

			assert_eq!(*state.borrow(), "generation-2");
			let stored = store.read("dynamic", None).await.unwrap();
			assert_eq!(
				stored
					.as_ref()
					.and_then(|entry| entry.models.first())
					.map(|model| model.model_id()),
				Some("generation-2")
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn passes_caller_signals_to_provider_auth_callbacks() {
		pi_host::testing::run(|_| async {
			let controller = AbortController::new();
			let received = Rc::new(RefCell::new(Vec::<AbortSignal>::new()));
			let api_key = ApiKeyAuth {
				name: "Signal auth".into(),
				login: Some({
					let received = received.clone();
					Rc::new(move |interaction| {
						received.borrow_mut().push(interaction.signal);
						Box::pin(async {
							Ok(ApiKeyCredential {
								key: Some("saved".into()),
								env: None,
								extra: Default::default(),
							})
						})
					})
				}),
				check: Some({
					let received = received.clone();
					Rc::new(move |input| {
						received.borrow_mut().push(input.signal);
						Box::pin(async {
							Ok(Some(AuthCheck {
								source: None,
								r#type: AuthType::ApiKey,
							}))
						})
					})
				}),
				resolve: {
					let received = received.clone();
					Rc::new(move |input| {
						received.borrow_mut().push(input.signal);
						Box::pin(async { Ok(resolved_key("resolved")) })
					})
				},
			};
			let models = create_models(None);
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				auth: Some(ProviderAuth {
					api_key: Some(api_key),
					oauth: None,
				}),
				..TestProviderInput::default()
			})));

			models.check_auth("p1", with_signal(controller.signal())).await.unwrap();
			models
				.get_auth(
					GetAuthTarget::Provider("p1"),
					overrides_with_signal(controller.signal()),
				)
				.await
				.unwrap();
			models
				.login("p1", AuthType::ApiKey, interaction(Some(controller.signal())), None)
				.await
				.unwrap();

			assert_eq!(received.borrow().len(), 3);
			assert!(
				received
					.borrow()
					.iter()
					.all(|signal| AbortSignal::ptr_eq(signal, &controller.signal()))
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn stops_waiting_for_non_cooperative_auth_callbacks() {
		pi_host::testing::run(|_| async {
			let check_started = Gate::new();
			let blocked_check = Gate::new();
			let resolve_started = Gate::new();
			let blocked_resolve = Gate::new();
			let models = create_models(None);
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				auth: Some(ProviderAuth {
					api_key: Some(ApiKeyAuth {
						name: "Blocked auth".into(),
						login: None,
						check: Some({
							let (started, blocked) = (check_started.clone(), blocked_check.clone());
							Rc::new(move |_| {
								started.open();
								let blocked = blocked.wait();
								Box::pin(async move {
									blocked.await;
									Ok(Some(AuthCheck {
										source: None,
										r#type: AuthType::ApiKey,
									}))
								})
							})
						}),
						resolve: {
							let (started, blocked) = (resolve_started.clone(), blocked_resolve.clone());
							Rc::new(move |_| {
								started.open();
								let blocked = blocked.wait();
								Box::pin(async move {
									blocked.await;
									Ok(resolved_key("key"))
								})
							})
						},
					}),
					oauth: None,
				}),
				..TestProviderInput::default()
			})));

			let available_controller = AbortController::new();
			let available = spawned(models.get_available(None, with_signal(available_controller.signal())));
			check_started.wait().await;
			available_controller.abort();
			assert_eq!(available.await.unwrap_err().name, "AbortError");

			let auth_controller = AbortController::new();
			let auth = spawned(models.get_auth(
				GetAuthTarget::Provider("p1"),
				overrides_with_signal(auth_controller.signal()),
			));
			resolve_started.wait().await;
			auth_controller.abort();
			assert_eq!(auth.await.unwrap_err().name, "AbortError");

			blocked_check.open();
			blocked_resolve.open();
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore"]
	fn cancels_queued_credential_mutations_without_running_them_later() {
		pi_host::testing::run(|_| async {
			let credentials = InMemoryCredentialStore::new();
			let first_blocked = Gate::new();
			let second_ran = Rc::new(Cell::new(false));
			let first = spawned(credentials.modify(
				"p1",
				{
					let blocked = first_blocked.wait();
					Box::new(move |_| {
						Box::pin(async move {
							blocked.await;
							Ok(Some(api_key_credential("first")))
						})
					})
				},
				None,
			));
			let controller = AbortController::new();
			let second = spawned(credentials.modify(
				"p1",
				{
					let second_ran = second_ran.clone();
					Box::new(move |_| {
						second_ran.set(true);
						Box::pin(async { Ok(Some(api_key_credential("second"))) })
					})
				},
				with_signal(controller.signal()),
			));

			controller.abort();
			assert_eq!(second.await.unwrap_err().name, "AbortError");
			first_blocked.open();
			first.await.unwrap();
			next_timer_tick().await;

			assert!(!second_ran.get());
			assert_eq!(
				credentials.read("p1", None).await.unwrap(),
				Some(api_key_credential("first"))
			);
		});
	}

	// Radius bug report 01a10855-43d8-7447-a710-2acf5ee0b2ee: refresh_token_invalidated after a cancelled refresh.
	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn persists_an_oauth_refresh_that_started_before_the_request_was_cancelled() {
		pi_host::testing::run(|_| async {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			store_credential(
				&*credentials,
				"p1",
				Credential::OAuth(oauth_credential("old", "old-refresh", 0.0)),
			)
			.await;
			let controller = AbortController::new();
			let models = create_models(Some(CreateModelsOptions {
				credentials: Some(credentials.clone() as Rc<dyn CredentialStore>),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				auth: Some(ProviderAuth {
					api_key: None,
					oauth: Some(test_oauth({
						let controller = controller.clone();
						oauth_refresh(move |credential| {
							// The provider has rotated old-refresh by the time the request is cancelled.
							controller.abort();
							async move {
								Ok(OAuthCredential {
									access: "new".into(),
									refresh: "new-refresh".into(),
									expires: pi_host::now_ms() + 60_000.0,
									..credential
								})
							}
						})
					})),
				}),
				..TestProviderInput::default()
			})));

			let error = models
				.get_auth(
					GetAuthTarget::Provider("p1"),
					overrides_with_signal(controller.signal()),
				)
				.await
				.unwrap_err();
			assert_eq!(error.name, "AbortError");
			wait_for(|| {
				let read = credentials.read("p1", None);
				Box::pin(async move { stored_tokens(read.await.unwrap()).1 == "new-refresh" })
			})
			.await;
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn resolves_auth_stored_credential_owns_the_provider_ambient_only_when_nothing_stored() {
		pi_host::testing::run(|_| async {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			let models = create_models(Some(CreateModelsOptions {
				credentials: Some(credentials.clone() as Rc<dyn CredentialStore>),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				auth: Some(ProviderAuth {
					api_key: Some(env_key_auth(Some("env-key"))),
					oauth: Some(test_oauth(None)),
				}),
				..TestProviderInput::default()
			})));
			let model = chat(&test_model("p1", "model-a"));
			let api_key = |result: PiResult<Option<AuthResult>>| result.unwrap().and_then(|result| result.auth.api_key);

			// model and provider-id overloads resolve the same provider-scoped auth
			assert_eq!(
				api_key(models.get_auth(GetAuthTarget::Model(&model), None).await).as_deref(),
				Some("env-key")
			);
			assert_eq!(
				api_key(
					models
						.get_auth(GetAuthTarget::Provider(model.model_provider()), None)
						.await
				)
				.as_deref(),
				Some("env-key")
			);
			let explicit = Some(AuthResolutionOverrides {
				api_key: Some("explicit-key".into()),
				..AuthResolutionOverrides::default()
			});
			assert_eq!(
				api_key(models.get_auth(GetAuthTarget::Model(&model), explicit).await).as_deref(),
				Some("explicit-key")
			);

			// stored oauth credential (persisted via the single write path): beats ambient env
			store_credential(
				&*credentials,
				"p1",
				Credential::OAuth(oauth_credential(
					"oauth-token",
					"r",
					pi_host::now_ms() + 10.0 * 60_000.0,
				)),
			)
			.await;
			let resolution = models
				.get_auth(GetAuthTarget::Provider(model.model_provider()), None)
				.await
				.unwrap()
				.unwrap();
			assert_eq!(resolution.auth.api_key.as_deref(), Some("oauth-token"));
			assert_eq!(resolution.source.as_deref(), Some("OAuth"));

			// stored api-key credential resolves through apiKey auth, beats env
			store_credential(&*credentials, "p1", api_key_credential("stored-key")).await;
			let api_key_resolution = models
				.get_auth(GetAuthTarget::Provider(model.model_provider()), None)
				.await
				.unwrap()
				.unwrap();
			assert_eq!(api_key_resolution.auth.api_key.as_deref(), Some("stored-key"));
			assert_eq!(api_key_resolution.source.as_deref(), Some("stored"));
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn checks_provider_auth_without_refreshing_oauth_and_filters_available_models() {
		pi_host::testing::run(|_| async {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			let refreshes = Rc::new(Cell::new(0));
			let models = create_models(Some(CreateModelsOptions {
				credentials: Some(credentials.clone() as Rc<dyn CredentialStore>),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "ambient",
				auth: Some(ProviderAuth {
					api_key: Some(env_key_auth(Some("env-key"))),
					oauth: None,
				}),
				..TestProviderInput::default()
			})));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "missing",
				auth: Some(ProviderAuth {
					api_key: Some(env_key_auth(None)),
					oauth: None,
				}),
				..TestProviderInput::default()
			})));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "oauth",
				auth: Some(ProviderAuth {
					api_key: None,
					oauth: Some(test_oauth({
						let refreshes = refreshes.clone();
						oauth_refresh(move |credential| {
							refreshes.set(refreshes.get() + 1);
							async move { Ok(credential) }
						})
					})),
				}),
				..TestProviderInput::default()
			})));
			store_credential(
				&*credentials,
				"oauth",
				Credential::OAuth(oauth_credential("expired", "refresh", 0.0)),
			)
			.await;

			assert_eq!(
				models.check_auth("ambient", None).await.unwrap(),
				Some(AuthCheck {
					source: Some("env".into()),
					r#type: AuthType::ApiKey,
				})
			);
			assert_eq!(models.check_auth("missing", None).await.unwrap(), None);
			assert_eq!(
				models.check_auth("oauth", None).await.unwrap(),
				Some(AuthCheck {
					source: Some("OAuth".into()),
					r#type: AuthType::OAuth,
				})
			);
			assert_eq!(refreshes.get(), 0);
			let providers = |list: Vec<Model>| {
				list.into_iter()
					.map(|model| model.base.provider.to_string())
					.collect::<Vec<_>>()
			};
			assert_eq!(
				providers(models.get_available(None, None).await.unwrap()),
				["ambient", "oauth"]
			);
			assert_eq!(
				providers(models.get_available(Some("ambient"), None).await.unwrap()),
				["ambient"]
			);
		});
	}

	#[test]
	fn runs_provider_login_and_logout_through_the_credential_store() {
		pi_host::testing::run(|_| async {
			let credentials: Rc<dyn CredentialStore> = Rc::new(TestCredentialStore::default());
			let mut api_key = env_key_auth(None);
			api_key.login = Some(Rc::new(|_| {
				Box::pin(async {
					Ok(ApiKeyCredential {
						key: Some("logged-in".into()),
						env: None,
						extra: Default::default(),
					})
				})
			}));
			let models = test_models(Some(CreateModelsOptions {
				credentials: Some(credentials.clone()),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				auth: Some(ProviderAuth {
					api_key: Some(api_key),
					oauth: None,
				}),
				..TestProviderInput::default()
			})));

			let credential = models
				.login("p1", AuthType::ApiKey, interaction(None), None)
				.await
				.unwrap();
			assert_eq!(credential, api_key_credential("logged-in"));
			assert_eq!(credentials.read("p1", None).await.unwrap(), Some(credential));

			models.logout("p1", None).await.unwrap();
			assert_eq!(credentials.read("p1", None).await.unwrap(), None);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn a_stored_credential_without_a_matching_handler_blocks_ambient_fallback() {
		pi_host::testing::run(|_| async {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			let models = create_models(Some(CreateModelsOptions {
				credentials: Some(credentials.clone() as Rc<dyn CredentialStore>),
				..CreateModelsOptions::default()
			}));
			// provider has only apiKey auth, but an oauth credential is stored (stale config)
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				auth: Some(ProviderAuth {
					api_key: Some(env_key_auth(Some("env-key"))),
					oauth: None,
				}),
				..TestProviderInput::default()
			})));
			store_credential(&*credentials, "p1", Credential::OAuth(oauth_credential("a", "r", 0.0))).await;

			assert_eq!(
				models.get_auth(GetAuthTarget::Provider("p1"), None).await.unwrap(),
				None
			);
		});
	}

	/// An OAuth refresh that counts its calls and returns `new-token`, valid for an hour.
	fn counted_new_token_refresh(refresh_calls: &Rc<Cell<u32>>) -> Option<OAuthRefreshFunction> {
		let refresh_calls = refresh_calls.clone();
		oauth_refresh(move |credential| {
			refresh_calls.set(refresh_calls.get() + 1);
			async move {
				Ok(OAuthCredential {
					access: "new-token".into(),
					expires: pi_host::now_ms() + 60.0 * 60_000.0,
					..credential
				})
			}
		})
	}

	/// `createModels({ credentials })` with one provider whose only auth is `oauth`.
	fn oauth_models(credentials: Rc<dyn CredentialStore>, oauth: OAuthAuth) -> Rc<dyn MutableModels> {
		let models = create_models(Some(CreateModelsOptions {
			credentials: Some(credentials),
			..CreateModelsOptions::default()
		}));
		models.set_provider(Rc::new(test_provider(TestProviderInput {
			id: "p1",
			auth: Some(ProviderAuth {
				api_key: None,
				oauth: Some(oauth),
			}),
			..TestProviderInput::default()
		})));
		models
	}

	async fn resolved_api_key(
		models: &Rc<dyn MutableModels>,
		overrides: Option<AuthResolutionOverrides>,
	) -> Option<String> {
		models
			.get_auth(GetAuthTarget::Provider("p1"), overrides)
			.await
			.unwrap()
			.and_then(|resolution| resolution.auth.api_key)
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn refreshes_expired_oauth_credentials_and_persists_the_rotated_credential() {
		pi_host::testing::run(|_| async {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			let oauth = test_oauth(oauth_refresh(|credential| async move {
				Ok(OAuthCredential {
					access: "new-token".into(),
					expires: pi_host::now_ms() + 60.0 * 60_000.0,
					..credential
				})
			}));
			let models = oauth_models(credentials.clone(), oauth);
			store_credential(
				&*credentials,
				"p1",
				Credential::OAuth(oauth_credential("old-token", "r", 0.0)),
			)
			.await;

			assert_eq!(resolved_api_key(&models, None).await.as_deref(), Some("new-token"));
			assert_eq!(
				stored_tokens(credentials.read("p1", None).await.unwrap()).0,
				"new-token"
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn refreshes_oauth_credentials_with_less_than_five_minutes_remaining() {
		pi_host::testing::run(|_| async {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			let refresh_calls = Rc::new(Cell::new(0));
			let models = oauth_models(
				credentials.clone(),
				test_oauth(counted_new_token_refresh(&refresh_calls)),
			);
			store_credential(
				&*credentials,
				"p1",
				Credential::OAuth(oauth_credential("old-token", "r", pi_host::now_ms() + 60_000.0)),
			)
			.await;

			assert_eq!(resolved_api_key(&models, None).await.as_deref(), Some("new-token"));
			assert_eq!(refresh_calls.get(), 1);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn honors_a_callers_longer_oauth_minimum_validity() {
		pi_host::testing::run(|_| async {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			let refresh_calls = Rc::new(Cell::new(0));
			let models = oauth_models(
				credentials.clone(),
				test_oauth(counted_new_token_refresh(&refresh_calls)),
			);
			store_credential(
				&*credentials,
				"p1",
				Credential::OAuth(oauth_credential("old-token", "r", pi_host::now_ms() + 10.0 * 60_000.0)),
			)
			.await;

			let overrides = Some(AuthResolutionOverrides {
				min_oauth_validity_ms: Some(30.0 * 60_000.0),
				..AuthResolutionOverrides::default()
			});
			assert_eq!(resolved_api_key(&models, overrides).await.as_deref(), Some("new-token"));
			assert_eq!(refresh_calls.get(), 1);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn rejects_with_code_oauth_when_refresh_fails_preserving_the_stored_credential() {
		pi_host::testing::run(|_| async {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			let oauth = test_oauth(oauth_refresh(|_| async { Err(PiError::new("invalid_grant")) }));
			let models = oauth_models(credentials.clone(), oauth);
			store_credential(
				&*credentials,
				"p1",
				Credential::OAuth(oauth_credential("old", "r", 0.0)),
			)
			.await;

			let error = models.get_auth(GetAuthTarget::Provider("p1"), None).await.unwrap_err();
			assert_eq!(ModelsError::code_of(&error), Some(ModelsErrorCode::Oauth));
			// credential preserved for retry / re-login
			assert_eq!(stored_tokens(credentials.read("p1", None).await.unwrap()).0, "old");
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn serializes_concurrent_oauth_refreshes_through_store_modify_no_double_refresh() {
		pi_host::testing::run(|_| async {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			store_credential(
				&*credentials,
				"p1",
				Credential::OAuth(oauth_credential("old", "r1", 0.0)),
			)
			.await;

			let refreshes = Rc::new(Cell::new(0));
			let oauth = test_oauth({
				let refreshes = refreshes.clone();
				oauth_refresh(move |_| {
					let refreshes = refreshes.clone();
					async move {
						refreshes.set(refreshes.get() + 1);
						pi_host::sleep(10.0, None).await?;
						Ok(oauth_credential(
							&format!("new-{}", refreshes.get()),
							"r2",
							pi_host::now_ms() + 60.0 * 60_000.0,
						))
					}
				})
			});
			let models = oauth_models(credentials.clone(), oauth);
			let model = test_model("p1", "model-a");

			let (a, b) = futures::join!(
				models.get_auth(GetAuthTarget::Provider(&model.provider), None),
				models.get_auth(GetAuthTarget::Provider(&model.provider), None)
			);
			assert_eq!(refreshes.get(), 1);
			assert_eq!(
				a.unwrap().and_then(|resolution| resolution.auth.api_key).as_deref(),
				Some("new-1")
			);
			assert_eq!(
				b.unwrap().and_then(|resolution| resolution.auth.api_key).as_deref(),
				Some("new-1")
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn valid_oauth_tokens_resolve_without_touching_modify() {
		pi_host::testing::run(|_| async {
			let modifies = Rc::new(Cell::new(0));
			let base = InMemoryCredentialStore::new();
			store_credential(
				&base,
				"p1",
				Credential::OAuth(oauth_credential("valid", "r", pi_host::now_ms() + 10.0 * 60_000.0)),
			)
			.await;
			let credentials = CountingStore {
				base,
				modifies: modifies.clone(),
			};
			let models = oauth_models(Rc::new(credentials), test_oauth(None));

			assert_eq!(resolved_api_key(&models, None).await.as_deref(), Some("valid"));
			assert_eq!(modifies.get(), 0);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn wraps_credential_store_failures_in_models_error() {
		pi_host::testing::run(|_| async {
			// read failure
			let models = create_models(Some(CreateModelsOptions {
				credentials: Some(Rc::new(ReadFailingStore)),
				..CreateModelsOptions::default()
			}));
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				auth: Some(ProviderAuth {
					api_key: Some(env_key_auth(Some("env-key"))),
					oauth: None,
				}),
				..TestProviderInput::default()
			})));
			let error = models.get_auth(GetAuthTarget::Provider("p1"), None).await.unwrap_err();
			assert_eq!(ModelsError::code_of(&error), Some(ModelsErrorCode::Auth));

			// modify failure during refresh
			let oauth_models = oauth_models(Rc::new(ModifyFailingStore), test_oauth(None));
			let error = oauth_models
				.get_auth(GetAuthTarget::Provider("p1"), None)
				.await
				.unwrap_err();
			assert_eq!(ModelsError::code_of(&error), Some(ModelsErrorCode::Auth));
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn keeps_the_underlying_reason_in_wrapped_oauth_refresh_errors() {
		pi_host::testing::run(|_| async {
			let credentials = Rc::new(InMemoryCredentialStore::new());
			store_credential(
				&*credentials,
				"p1",
				Credential::OAuth(oauth_credential("old", "r", 0.0)),
			)
			.await;
			let oauth = test_oauth(oauth_refresh(|_| async {
				Err(PiError::new("token refresh failed (400): invalid_grant"))
			}));
			let models = oauth_models(credentials.clone(), oauth);

			let error = models.get_auth(GetAuthTarget::Provider("p1"), None).await.unwrap_err();
			assert!(
				error
					.message
					.contains("OAuth refresh failed for p1: token refresh failed (400): invalid_grant"),
				"{}",
				error.message
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn wraps_api_key_auth_failures_in_models_error() {
		pi_host::testing::run(|_| async {
			let failing = ApiKeyAuth {
				name: "Failing".into(),
				login: None,
				check: None,
				resolve: Rc::new(|_| Box::pin(async { Err(PiError::new("nope")) })),
			};
			let models = create_models(None);
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				auth: Some(ProviderAuth {
					api_key: Some(failing),
					oauth: None,
				}),
				..TestProviderInput::default()
			})));
			let error = models.get_auth(GetAuthTarget::Provider("p1"), None).await.unwrap_err();
			assert_eq!(ModelsError::code_of(&error), Some(ModelsErrorCode::Auth));
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn uses_explicit_request_api_key_and_env_during_provider_auth_resolution() {
		pi_host::testing::run(|_| async {
			let calls: Calls = Rc::default();
			let api_key = ApiKeyAuth {
				name: "Scoped".into(),
				login: None,
				check: None,
				resolve: Rc::new(|input: ApiKeyAuthInput| {
					Box::pin(async move {
						let stored_account = input
							.credential
							.as_ref()
							.and_then(|credential| credential.env.as_ref())
							.and_then(|env| env.get("ACCOUNT_ID"))
							.cloned();
						let account = match stored_account {
							Some(account) => Some(account),
							None => input.ctx.env("ACCOUNT_ID").await,
						};
						let key = input.credential.and_then(|credential| credential.key);
						let (Some(key), Some(account)) = (key, account) else {
							return Ok(None);
						};
						Ok(Some(AuthResult {
							auth: ModelAuth {
								api_key: Some(key),
								base_url: Some(format!("https://example.test/{account}")),
								..ModelAuth::default()
							},
							env: Some(IndexMap::from([("ACCOUNT_ID".to_string(), account)])),
							source: None,
						}))
					})
				}),
			};
			let models = create_models(None);
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				auth: Some(ProviderAuth {
					api_key: Some(api_key),
					oauth: None,
				}),
				calls: Some(calls.clone()),
				..TestProviderInput::default()
			})));
			let model = test_model("p1", "model-a");

			models
				.complete_simple(
					&model,
					&context(),
					simple_options(|options| {
						options.api_key = Some("explicit-key".into());
						options.env = Some(IndexMap::from([("ACCOUNT_ID".to_string(), "acct".to_string())]));
					}),
				)
				.await;

			let calls = calls.borrow();
			assert_eq!(calls[0].model.base_url, "https://example.test/acct");
			let options = calls[0].options.as_ref().unwrap();
			assert_eq!(options.api_key.as_deref(), Some("explicit-key"));
			assert_eq!(
				options.env,
				Some(IndexMap::from([("ACCOUNT_ID".to_string(), "acct".to_string())]))
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn merges_resolved_auth_into_stream_options_explicit_options_win_per_field() {
		pi_host::testing::run(|_| async {
			let calls: Calls = Rc::default();
			let api_key = ApiKeyAuth {
				name: "Test".into(),
				login: None,
				check: None,
				resolve: Rc::new(|_| {
					Box::pin(async {
						Ok(Some(AuthResult {
							auth: ModelAuth {
								api_key: Some("resolved-key".into()),
								headers: Some(headers(&[
									("Authorization", "Bearer resolved-key"),
									("x-a", "auth"),
									("x-b", "auth"),
								])),
								base_url: Some("https://auth.test/v1".into()),
							},
							..AuthResult::default()
						}))
					})
				}),
			};
			let models = create_models(None);
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				auth: Some(ProviderAuth {
					api_key: Some(api_key),
					oauth: None,
				}),
				calls: Some(calls.clone()),
				..TestProviderInput::default()
			})));
			let model = test_model("p1", "model-a");

			let result = models
				.complete_simple(
					&model,
					&context(),
					simple_options(|options| {
						options.api_key = Some("explicit-key".into());
						options.headers = Some(headers(&[("authorization", "Explicit token"), ("x-b", "explicit")]));
					}),
				)
				.await
				.unwrap();
			assert_eq!(result.stop_reason, StopReason::Stop);
			assert_eq!(calls.borrow().len(), 1);
			{
				let calls = calls.borrow();
				let options = calls[0].options.as_ref().unwrap();
				assert_eq!(options.api_key.as_deref(), Some("explicit-key"));
				assert_eq!(
					options.headers,
					Some(headers(&[
						("authorization", "Explicit token"),
						("x-a", "auth"),
						("x-b", "explicit"),
					]))
				);
				assert_eq!(calls[0].model.base_url, "https://auth.test/v1");
			}

			// without explicit options, resolved auth applies
			let result2 = models.complete_simple(&model, &context(), None).await.unwrap();
			assert_eq!(result2.stop_reason, StopReason::Stop);
			assert_eq!(
				calls.borrow()[1].options.as_ref().unwrap().api_key.as_deref(),
				Some("resolved-key")
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn adds_model_headers_only_for_model_auth_and_transforms_assembled_headers_once() {
		pi_host::testing::run(|_| async {
			let calls: Calls = Rc::default();
			let models = create_models(None);
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				auth: Some(ProviderAuth {
					api_key: Some(env_key_auth(Some("key"))),
					oauth: None,
				}),
				calls: Some(calls.clone()),
				..TestProviderInput::default()
			})));
			let mut model = test_model("p1", "model-a");
			model.base.headers = Some(IndexMap::from([
				("x-model".to_string(), "model".to_string()),
				("x-shared".to_string(), "model".to_string()),
			]));

			let provider_auth = models.get_auth(GetAuthTarget::Provider("p1"), None).await.unwrap();
			assert_eq!(provider_auth.unwrap().auth.headers, None);
			let model_auth = models
				.get_auth(GetAuthTarget::Model(&chat(&model)), None)
				.await
				.unwrap();
			assert_eq!(
				model_auth.unwrap().auth.headers,
				Some(headers(&[("x-model", "model"), ("x-shared", "model")]))
			);

			let transforms = Rc::new(Cell::new(0));
			models
				.complete_simple(
					&model,
					&context(),
					simple_options(|options| {
						options.headers = Some(headers(&[("x-explicit", "explicit"), ("X-Shared", "explicit")]));
						let transforms = transforms.clone();
						options.transform_headers = Some(Rc::new(move |mut assembled: ProviderHeaders| {
							transforms.set(transforms.get() + 1);
							assert_eq!(
								assembled,
								headers(&[
									("x-model", "model"),
									("x-explicit", "explicit"),
									("X-Shared", "explicit")
								])
							);
							assembled.insert("x-transformed".into(), Some("yes".into()));
							Box::pin(async move { Ok(assembled) })
						}));
					}),
				)
				.await;

			assert_eq!(transforms.get(), 1);
			assert_eq!(
				calls.borrow()[0].options.as_ref().unwrap().headers,
				Some(headers(&[
					("x-model", "model"),
					("x-explicit", "explicit"),
					("X-Shared", "explicit"),
					("x-transformed", "yes"),
				]))
			);
		});
	}

	#[test]
	fn produces_an_error_stream_for_unknown_providers_instead_of_throwing() {
		pi_host::testing::run(|_| async {
			let models = create_models(None);
			let result = models
				.complete_simple(&test_model("ghost", "model-a"), &context(), None)
				.await
				.unwrap();
			assert_eq!(result.stop_reason, StopReason::Error);
			assert!(
				result
					.error_message
					.as_deref()
					.is_some_and(|message| message.contains("Unknown provider: ghost"))
			);
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: InMemoryCredentialStore, resolve_provider_auth"]
	fn streams_through_the_provider() {
		pi_host::testing::run(|_| async {
			let models = create_models(None);
			models.set_provider(Rc::new(test_provider(TestProviderInput {
				id: "p1",
				..TestProviderInput::default()
			})));
			let model = test_model("p1", "model-a");

			let mut events = Vec::new();
			let stream = models.stream_simple(&model, &context(), None);
			while let Some(event) = stream.next().await {
				events.push(match event {
					AssistantMessageEvent::Start { .. } => "start",
					AssistantMessageEvent::Done { .. } => "done",
					AssistantMessageEvent::Error { .. } => "error",
					_ => "other",
				});
			}
			assert_eq!(events, ["start", "done"]);
			let message = stream.result().await.unwrap();
			assert_eq!(message.stop_reason, StopReason::Stop);
		});
	}
}
