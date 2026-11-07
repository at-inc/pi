//! Auth objects across the boundary, both ways. A JS object the caller passes (`ProviderAuth`, `CredentialStore`,
//! `AuthContext`, `ModelsStore`, an interaction) becomes a Rust value that calls the JS methods, looked up at call
//! time like a TS call `object.method(...)`, so a method replaced or spied on later is the one called. A Rust value
//! the facade hands out is a handle class whose methods return Promises; the facade wraps it in a JS object of the
//! TS shape (`rust/npm/pi-ai/src/internal/auth.ts`).
//!
//! Every JS method is called when the Rust caller calls it, as in the TS, and its result is awaited later. A Rust
//! `AbortSignal` passed to JS is a JS `AbortSignal` that follows it while the call (or the interaction object that
//! carries it) is alive.

use std::rc::Rc;

use js_sys::{Array, Function, Object, Promise, Reflect};
use pi_ai::auth::types::{
	ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthContext, AuthEvent, AuthInteraction, AuthOperationOptions,
	AuthPrompt, AuthPromptKind, Credential, CredentialInfo, CredentialModifier, CredentialStore, LoginOptions,
	ModelAuth, OAuthAuth, OAuthCredential, ProviderAuth, ProviderAuthInteraction,
};
use pi_ai::models_store::{ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions};
use pi_host::{AbortController, AbortSignal, LocalBoxFuture, PiError, PiResult};
use serde::de::DeserializeOwned;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, future_to_promise};

use crate::abort::JsSignal;
use crate::error;
use crate::json::{from_js, to_js};

/// `object[name]`.
fn get(object: &JsValue, name: &str) -> JsValue {
	Reflect::get(object, &name.into()).unwrap_or(JsValue::UNDEFINED)
}

/// Sets `object[name]`; setting a property of a fresh plain object cannot fail.
fn set(object: &Object, name: &str, value: &JsValue) {
	let _ = Reflect::set(object, &name.into(), value);
}

/// `typeof object[name] === "function"`.
fn has_method(object: &JsValue, name: &str) -> bool {
	get(object, name).is_function()
}

/// `object[name](...args)`, called now and awaited by the returned future. `signals` stay alive until it settles.
fn call_method(
	object: &JsValue,
	name: &str,
	args: &[JsValue],
	signals: Vec<JsSignal>,
) -> LocalBoxFuture<'static, PiResult<JsValue>> {
	let called = get(object, name)
		.dyn_into::<Function>()
		.map_err(|_| PiError::type_error(format!("{name} is not a function")))
		.and_then(|function| {
			let args: Array = args.iter().collect();
			function.apply(object, &args).map_err(error::from_js)
		});
	Box::pin(async move {
		let result = JsFuture::from(Promise::resolve(&called?)).await.map_err(error::from_js);
		drop(signals);
		result
	})
}

/// A JS result read as `T`; `undefined` and `null` are `None`.
fn optional<T: DeserializeOwned>(value: JsValue) -> PiResult<Option<T>> {
	if value.is_undefined() || value.is_null() {
		Ok(None)
	} else {
		from_js(value).map(Some)
	}
}

/// An optional argument field: `undefined` is `None`.
pub fn optional_field<T: DeserializeOwned>(object: &JsValue, name: &str) -> Result<Option<T>, JsValue> {
	let value = get(object, name);
	if value.is_undefined() {
		Ok(None)
	} else {
		crate::options::arg(value).map(Some)
	}
}

/// A JS string result; any other value is `String(value)`, as TS template code would read it.
fn string(value: &JsValue) -> String {
	value
		.as_string()
		.unwrap_or_else(|| js_sys::JsString::from(value.clone()).as_string().unwrap_or_default())
}

/// A Promise of a Rust future's JS value; an error rejects with its JS error.
pub fn promise(future: impl std::future::Future<Output = PiResult<JsValue>> + 'static) -> Promise {
	future_to_promise(async move { future.await.map_err(|error| error::to_js(&error)) })
}

/// `{ signal }` of an operation's options, `undefined` without options.
fn operation_options(signal: Option<&AbortSignal>, present: bool) -> (JsValue, Vec<JsSignal>) {
	if !present {
		return (JsValue::UNDEFINED, Vec::new());
	}
	let object = Object::new();
	let mut signals = Vec::new();
	if let Some(signal) = signal {
		let signal = crate::abort::to_js(signal);
		set(&object, "signal", &signal.js);
		signals.push(signal);
	}
	(object.into(), signals)
}

/// `AuthOperationOptions` of a JS `{ signal }`.
pub fn auth_operation_options(options: &JsValue) -> Option<AuthOperationOptions> {
	if !options.is_object() {
		return None;
	}
	Some(AuthOperationOptions {
		signal: crate::abort::from_js(&get(options, "signal")),
	})
}

/// `ModelsStoreOperationOptions` of a JS `{ signal }`.
pub fn models_store_operation_options(options: &JsValue) -> Option<ModelsStoreOperationOptions> {
	if !options.is_object() {
		return None;
	}
	Some(ModelsStoreOperationOptions {
		signal: crate::abort::from_js(&get(options, "signal")),
	})
}

/// The JS value of a credential, `undefined` for none.
pub fn credential_to_js(credential: Option<&Credential>) -> PiResult<JsValue> {
	credential.map_or(Ok(JsValue::UNDEFINED), to_js)
}

/// A stored credential the JS side returned; `undefined` and `null` are none.
pub fn credential_from_js(value: JsValue) -> PiResult<Option<Credential>> {
	optional(value)
}

// Interactions.

/// The JS `AuthPrompt` of a Rust prompt. Its signal follows the Rust one after the prompt settles, since a UI keeps
/// it to dismiss the prompt when the login settles.
fn prompt_to_js(prompt: &AuthPrompt) -> PiResult<JsValue> {
	let object = to_js(&prompt.kind)?;
	if let Some(signal) = &prompt.signal {
		set(object.unchecked_ref(), "signal", &crate::abort::to_js_lasting(signal));
	}
	Ok(object)
}

/// The Rust `AuthPrompt` of a JS prompt.
fn prompt_from_js(prompt: JsValue) -> PiResult<AuthPrompt> {
	let signal = crate::abort::from_js(&get(&prompt, "signal"));
	let fields = Object::assign(&Object::new(), prompt.unchecked_ref());
	let _ = Reflect::delete_property(&fields, &"signal".into());
	let kind: AuthPromptKind = from_js(fields.into())?;
	Ok(AuthPrompt { signal, kind })
}

/// `prompt` and `notify` over a JS interaction object.
fn interaction_callbacks(
	object: &JsValue,
) -> (
	pi_ai::auth::types::AuthPromptFunction,
	pi_ai::auth::types::AuthNotifyFunction,
) {
	let this = object.clone();
	let prompt = Rc::new(move |prompt: AuthPrompt| {
		let call = prompt_to_js(&prompt).map(|prompt| call_method(&this, "prompt", &[prompt], Vec::new()));
		Box::pin(async move { Ok(string(&call?.await?)) }) as LocalBoxFuture<'static, PiResult<String>>
	});
	let this = object.clone();
	let notify = Rc::new(move |event: AuthEvent| {
		let Ok(event) = to_js(&event) else { return };
		if let Ok(notify) = get(&this, "notify").dyn_into::<Function>() {
			let _ = notify.call1(&this, &event);
		}
	});
	(prompt, notify)
}

/// The Rust `AuthInteraction` of a JS one (`Models.login`).
pub fn interaction_from_js(object: &JsValue) -> AuthInteraction {
	let (prompt, notify) = interaction_callbacks(object);
	AuthInteraction {
		signal: crate::abort::from_js(&get(object, "signal")),
		prompt,
		notify,
	}
}

/// The Rust `ProviderAuthInteraction` of a JS one; a missing signal never aborts.
pub fn provider_interaction_from_js(object: &JsValue) -> ProviderAuthInteraction {
	let (prompt, notify) = interaction_callbacks(object);
	ProviderAuthInteraction {
		signal: crate::abort::from_js(&get(object, "signal")).unwrap_or_else(|| AbortController::new().signal()),
		prompt,
		notify,
	}
}

/// The JS `ProviderAuthInteraction` of a Rust one: `{ signal, prompt, notify }`. The JS signal follows the Rust one
/// while JS holds the object.
pub fn provider_interaction_to_js(interaction: &ProviderAuthInteraction) -> JsValue {
	let object = Object::new();
	let signal = crate::abort::to_js(&interaction.signal);
	set(&object, "signal", &signal.js);
	let prompt_function = interaction.prompt.clone();
	let prompt = Closure::<dyn Fn(JsValue) -> Promise>::new(move |prompt: JsValue| {
		let _ = &signal;
		let pending = prompt_from_js(prompt).map(|prompt| prompt_function(prompt));
		promise(async move { Ok(pending?.await?.into()) })
	});
	set(&object, "prompt", &prompt.into_js_value());
	let notify_function = interaction.notify.clone();
	let notify = Closure::<dyn Fn(JsValue)>::new(move |event: JsValue| {
		if let Ok(event) = from_js::<AuthEvent>(event) {
			notify_function(event);
		}
	});
	set(&object, "notify", &notify.into_js_value());
	object.into()
}

/// The JS `LoginOptions` of Rust ones, `undefined` for none.
pub fn login_options_to_js(options: Option<&LoginOptions>) -> JsValue {
	let Some(options) = options else {
		return JsValue::UNDEFINED;
	};
	let object = Object::new();
	if let Some(get_device_id) = options.get_device_id.clone() {
		let function = Closure::<dyn Fn() -> String>::new(move || get_device_id()).into_js_value();
		set(&object, "getDeviceId", &function);
	}
	if let Some(agent_name) = &options.agent_name {
		set(&object, "agentName", &agent_name.as_str().into());
	}
	object.into()
}

/// The Rust `LoginOptions` of JS ones.
pub fn login_options_from_js(options: &JsValue) -> Option<LoginOptions> {
	if !options.is_object() {
		return None;
	}
	let get_device_id = get(options, "getDeviceId").dyn_into::<Function>().ok().map(|function| {
		let this = options.clone();
		Rc::new(move || function.call0(&this).map(|id| string(&id)).unwrap_or_default()) as Rc<dyn Fn() -> String>
	});
	Some(LoginOptions {
		get_device_id,
		agent_name: get(options, "agentName").as_string(),
	})
}

// Provider auth.

/// The `OAuthAuth` of a JS loader's result: an `OAuthAuth` object or a Promise of one. A Promise gives an auth whose
/// fields are empty and whose methods wait for it, which is all a `lazyOAuth` loader needs.
pub fn loaded_oauth_auth(value: JsValue) -> OAuthAuth {
	if !get(&value, "then").is_function() {
		return oauth_auth_from_js(&value);
	}
	let loaded = {
		let promise = JsFuture::from(Promise::resolve(&value));
		Box::pin(async move {
			promise
				.await
				.map(|auth| oauth_auth_from_js(&auth))
				.map_err(error::from_js)
		}) as LocalBoxFuture<'static, PiResult<OAuthAuth>>
	};
	let loaded = futures::FutureExt::shared(loaded);
	pi_ai::auth::helpers::lazy_oauth(pi_ai::auth::helpers::LazyOAuthInput {
		name: String::new(),
		is_subscription: None,
		login_label: None,
		load: Rc::new(move || Box::pin(loaded.clone())),
	})
}

/// `ProviderAuth` of a JS `auth` object: each present method of `apiKey` and `oauth` calls the JS object.
pub fn provider_auth(auth: &JsValue) -> ProviderAuth {
	let api_key = get(auth, "apiKey");
	let oauth = get(auth, "oauth");
	ProviderAuth {
		api_key: api_key.is_object().then(|| api_key_auth_from_js(&api_key)),
		oauth: oauth.is_object().then(|| oauth_auth_from_js(&oauth)),
	}
}

/// The JS `{ ctx, credential, signal }` input of an api-key auth method.
fn api_key_input_to_js(input: &ApiKeyAuthInput) -> PiResult<(JsValue, Vec<JsSignal>)> {
	let object = Object::new();
	set(&object, "ctx", &auth_context_to_js(input.ctx.clone()));
	let credential = match &input.credential {
		Some(credential) => to_js(credential)?,
		None => JsValue::UNDEFINED,
	};
	set(&object, "credential", &credential);
	let signal = crate::abort::to_js(&input.signal);
	set(&object, "signal", &signal.js);
	Ok((object.into(), vec![signal]))
}

/// `ApiKeyAuthInput` of a JS `{ ctx, credential, signal }`.
fn api_key_input_from_js(input: &JsValue) -> PiResult<ApiKeyAuthInput> {
	Ok(ApiKeyAuthInput {
		ctx: auth_context_from_js(&get(input, "ctx")),
		credential: optional::<ApiKeyCredential>(get(input, "credential"))?,
		signal: crate::abort::from_js(&get(input, "signal")).unwrap_or_else(|| AbortController::new().signal()),
	})
}

/// Calls an api-key auth method with `{ ctx, credential, signal }`; `undefined` is `None`.
fn api_key_call<T: DeserializeOwned + 'static>(
	this: &JsValue,
	name: &'static str,
	input: ApiKeyAuthInput,
) -> LocalBoxFuture<'static, PiResult<Option<T>>> {
	let call = api_key_input_to_js(&input).map(|(input, signals)| call_method(this, name, &[input], signals));
	Box::pin(async move { optional(call?.await?) })
}

/// `ApiKeyAuth` of a JS object. `login` and `check` exist when the object has them now.
fn api_key_auth_from_js(object: &JsValue) -> ApiKeyAuth {
	let this = object.clone();
	let login = has_method(object, "login").then(|| {
		let this = this.clone();
		Rc::new(move |interaction: ProviderAuthInteraction| {
			let call = call_method(&this, "login", &[provider_interaction_to_js(&interaction)], Vec::new());
			Box::pin(async move { from_js::<ApiKeyCredential>(call.await?) }) as LocalBoxFuture<'static, _>
		}) as pi_ai::auth::types::ApiKeyLoginFunction
	});
	let check = has_method(object, "check").then(|| {
		let this = this.clone();
		Rc::new(move |input: ApiKeyAuthInput| api_key_call(&this, "check", input))
			as pi_ai::auth::types::ApiKeyCheckFunction
	});
	let resolve_this = this.clone();
	ApiKeyAuth {
		name: get(object, "name").as_string().unwrap_or_default(),
		login,
		check,
		resolve: Rc::new(move |input: ApiKeyAuthInput| api_key_call(&resolve_this, "resolve", input)),
	}
}

/// `OAuthAuth` of a JS object.
pub fn oauth_auth_from_js(object: &JsValue) -> OAuthAuth {
	let this = object.clone();
	let login_this = this.clone();
	let refresh_this = this.clone();
	OAuthAuth {
		name: get(object, "name").as_string().unwrap_or_default(),
		is_subscription: get(object, "isSubscription").as_bool(),
		login_label: get(object, "loginLabel").as_string(),
		login: Rc::new(move |interaction, options| {
			let args = [
				provider_interaction_to_js(&interaction),
				login_options_to_js(options.as_ref()),
			];
			let call = call_method(&login_this, "login", &args, Vec::new());
			Box::pin(async move { from_js::<OAuthCredential>(call.await?) })
		}),
		refresh: Rc::new(move |credential, signal| {
			let js_signal = crate::abort::to_js(&signal);
			let call = to_js(&credential).map(|credential| {
				let args = [credential, js_signal.js.clone()];
				call_method(&refresh_this, "refresh", &args, vec![js_signal])
			});
			Box::pin(async move { from_js::<OAuthCredential>(call?.await?) })
		}),
		to_auth: Rc::new(move |credential| {
			let call = to_js(&credential).map(|credential| call_method(&this, "toAuth", &[credential], Vec::new()));
			Box::pin(async move { from_js::<ModelAuth>(call?.await?) })
		}),
	}
}

/// A Rust `ApiKeyAuth`, wrapped by the facade as a JS `ApiKeyAuth`.
#[wasm_bindgen]
pub struct ApiKeyAuthHandle {
	inner: ApiKeyAuth,
}

impl ApiKeyAuthHandle {
	pub fn new(inner: ApiKeyAuth) -> Self {
		Self { inner }
	}
}

#[wasm_bindgen]
impl ApiKeyAuthHandle {
	/// `apiKey.name`.
	#[wasm_bindgen(getter)]
	pub fn name(&self) -> String {
		self.inner.name.clone()
	}

	/// Whether the auth has `login`.
	#[wasm_bindgen(getter, js_name = hasLogin)]
	pub fn has_login(&self) -> bool {
		self.inner.login.is_some()
	}

	/// Whether the auth has `check`.
	#[wasm_bindgen(getter, js_name = hasCheck)]
	pub fn has_check(&self) -> bool {
		self.inner.check.is_some()
	}

	/// `apiKey.login(interaction)`.
	pub fn login(&self, interaction: JsValue) -> Promise {
		let login = self
			.inner
			.login
			.clone()
			.map(|login| login(provider_interaction_from_js(&interaction)));
		promise(async move {
			let login = login.ok_or_else(|| PiError::type_error("login is not a function"))?;
			to_js(&login.await?)
		})
	}

	/// `apiKey.check(input)`.
	pub fn check(&self, input: JsValue) -> Promise {
		let check = self.inner.check.clone();
		let pending = api_key_input_from_js(&input).map(|input| check.map(|check| check(input)));
		promise(async move {
			let check = pending?.ok_or_else(|| PiError::type_error("check is not a function"))?;
			check.await?.map_or(Ok(JsValue::UNDEFINED), |result| to_js(&result))
		})
	}

	/// `apiKey.resolve(input)`.
	pub fn resolve(&self, input: JsValue) -> Promise {
		let pending = api_key_input_from_js(&input).map(|input| self.inner.resolve(input));
		promise(async move { pending?.await?.map_or(Ok(JsValue::UNDEFINED), |result| to_js(&result)) })
	}
}

/// A Rust `OAuthAuth`, wrapped by the facade as a JS `OAuthAuth`.
#[wasm_bindgen]
pub struct OAuthAuthHandle {
	inner: OAuthAuth,
}

impl OAuthAuthHandle {
	pub fn new(inner: OAuthAuth) -> Self {
		Self { inner }
	}
}

#[wasm_bindgen]
impl OAuthAuthHandle {
	/// `oauth.name`.
	#[wasm_bindgen(getter)]
	pub fn name(&self) -> String {
		self.inner.name.clone()
	}

	/// `oauth.isSubscription`.
	#[wasm_bindgen(getter, js_name = isSubscription)]
	pub fn is_subscription(&self) -> Option<bool> {
		self.inner.is_subscription
	}

	/// `oauth.loginLabel`.
	#[wasm_bindgen(getter, js_name = loginLabel)]
	pub fn login_label(&self) -> Option<String> {
		self.inner.login_label.clone()
	}

	/// `oauth.login(interaction, options)`.
	pub fn login(&self, interaction: JsValue, options: JsValue) -> Promise {
		let login = self.inner.login(
			provider_interaction_from_js(&interaction),
			login_options_from_js(&options),
		);
		promise(async move { to_js(&login.await?) })
	}

	/// `oauth.refresh(credential, signal)`.
	pub fn refresh(&self, credential: JsValue, signal: JsValue) -> Promise {
		let signal = crate::abort::from_js(&signal).unwrap_or_else(|| AbortController::new().signal());
		let refresh = from_js::<OAuthCredential>(credential).map(|credential| self.inner.refresh(credential, signal));
		promise(async move { to_js(&refresh?.await?) })
	}

	/// `oauth.toAuth(credential)`.
	#[wasm_bindgen(js_name = toAuth)]
	pub fn to_auth(&self, credential: JsValue) -> Promise {
		let to_auth = from_js::<OAuthCredential>(credential).map(|credential| self.inner.to_auth(credential));
		promise(async move { to_js(&to_auth?.await?) })
	}
}

// Auth contexts.

/// An `AuthContext` over a JS object. A JS rejection reads as unset or missing, since the Rust trait cannot fail.
struct JsAuthContext {
	object: JsValue,
}

impl AuthContext for JsAuthContext {
	fn env(&self, name: &str) -> LocalBoxFuture<'static, Option<String>> {
		let call = call_method(&self.object, "env", &[name.into()], Vec::new());
		Box::pin(async move { call.await.ok().and_then(|value| value.as_string()) })
	}

	fn file_exists(&self, path: &str) -> LocalBoxFuture<'static, bool> {
		let call = call_method(&self.object, "fileExists", &[path.into()], Vec::new());
		Box::pin(async move { call.await.ok().is_some_and(|value| value.is_truthy()) })
	}
}

/// The Rust `AuthContext` of a JS one.
pub fn auth_context_from_js(object: &JsValue) -> Rc<dyn AuthContext> {
	Rc::new(JsAuthContext { object: object.clone() })
}

/// The JS `AuthContext` of a Rust one: `{ env, fileExists }` over an `AuthContextHandle`.
pub fn auth_context_to_js(context: Rc<dyn AuthContext>) -> JsValue {
	let handle = Rc::new(AuthContextHandle { inner: context });
	let object = Object::new();
	let env = handle.clone();
	set(
		&object,
		"env",
		&Closure::<dyn Fn(String) -> Promise>::new(move |name: String| env.env(name)).into_js_value(),
	);
	set(
		&object,
		"fileExists",
		&Closure::<dyn Fn(String) -> Promise>::new(move |path: String| handle.file_exists(path)).into_js_value(),
	);
	object.into()
}

/// A Rust `AuthContext`, wrapped by the facade as a JS `AuthContext`.
#[wasm_bindgen]
pub struct AuthContextHandle {
	inner: Rc<dyn AuthContext>,
}

impl AuthContextHandle {
	pub fn new(inner: Rc<dyn AuthContext>) -> Self {
		Self { inner }
	}
}

#[wasm_bindgen]
impl AuthContextHandle {
	/// `ctx.env(name)`.
	pub fn env(&self, name: String) -> Promise {
		let pending = self.inner.env(&name);
		future_to_promise(async move { Ok(pending.await.map_or(JsValue::UNDEFINED, JsValue::from)) })
	}

	/// `ctx.fileExists(path)`.
	#[wasm_bindgen(js_name = fileExists)]
	pub fn file_exists(&self, path: String) -> Promise {
		let pending = self.inner.file_exists(&path);
		future_to_promise(async move { Ok(JsValue::from(pending.await)) })
	}
}

// Credential stores.

/// A `CredentialStore` over a JS object.
struct JsCredentialStore {
	object: JsValue,
}

impl CredentialStore for JsCredentialStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		let (options, signals) = operation_options(options.as_ref().and_then(|o| o.signal.as_ref()), options.is_some());
		let call = call_method(&self.object, "read", &[provider_id.into(), options], signals);
		Box::pin(async move { credential_from_js(call.await?) })
	}

	fn list(&self, options: Option<AuthOperationOptions>) -> LocalBoxFuture<'static, PiResult<Vec<CredentialInfo>>> {
		let (options, signals) = operation_options(options.as_ref().and_then(|o| o.signal.as_ref()), options.is_some());
		let call = call_method(&self.object, "list", &[options], signals);
		Box::pin(async move { from_js(call.await?) })
	}

	fn modify(
		&self,
		provider_id: &str,
		r#fn: CredentialModifier,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<Credential>>> {
		let (options, signals) = operation_options(options.as_ref().and_then(|o| o.signal.as_ref()), options.is_some());
		let modifier = Closure::once_into_js(move |current: JsValue| -> Promise {
			promise(async move {
				let next = r#fn(credential_from_js(current)?).await?;
				credential_to_js(next.as_ref())
			})
		});
		let call = call_method(
			&self.object,
			"modify",
			&[provider_id.into(), modifier, options],
			signals,
		);
		Box::pin(async move { credential_from_js(call.await?) })
	}

	fn delete(
		&self,
		provider_id: &str,
		options: Option<AuthOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let (options, signals) = operation_options(options.as_ref().and_then(|o| o.signal.as_ref()), options.is_some());
		let call = call_method(&self.object, "delete", &[provider_id.into(), options], signals);
		Box::pin(async move { call.await.map(|_| ()) })
	}
}

/// The Rust `CredentialStore` of a JS one.
pub fn credential_store_from_js(object: &JsValue) -> Rc<dyn CredentialStore> {
	Rc::new(JsCredentialStore { object: object.clone() })
}

/// A Rust `CredentialStore`, wrapped by the facade's store classes.
#[wasm_bindgen]
pub struct CredentialStoreHandle {
	inner: Rc<dyn CredentialStore>,
}

impl CredentialStoreHandle {
	pub fn new(inner: Rc<dyn CredentialStore>) -> Self {
		Self { inner }
	}
}

#[wasm_bindgen]
impl CredentialStoreHandle {
	/// `store.read(providerId, options)`.
	pub fn read(&self, provider_id: &str, options: JsValue) -> Promise {
		let read = self.inner.read(provider_id, auth_operation_options(&options));
		promise(async move { credential_to_js(read.await?.as_ref()) })
	}

	/// `store.list(options)`.
	pub fn list(&self, options: JsValue) -> Promise {
		let list = self.inner.list(auth_operation_options(&options));
		promise(async move { to_js(&list.await?) })
	}

	/// `store.modify(providerId, fn, options)`: `fn` gets the current credential and resolves with the next one.
	pub fn modify(&self, provider_id: &str, r#fn: Function, options: JsValue) -> Promise {
		let modifier: CredentialModifier = Box::new(move |current| {
			let call = credential_to_js(current.as_ref())
				.and_then(|current| r#fn.call1(&JsValue::UNDEFINED, &current).map_err(error::from_js));
			Box::pin(async move {
				let next = JsFuture::from(Promise::resolve(&call?)).await.map_err(error::from_js)?;
				credential_from_js(next)
			})
		});
		let modify = self
			.inner
			.modify(provider_id, modifier, auth_operation_options(&options));
		promise(async move { credential_to_js(modify.await?.as_ref()) })
	}

	/// `store.delete(providerId, options)`.
	pub fn delete(&self, provider_id: &str, options: JsValue) -> Promise {
		let delete = self.inner.delete(provider_id, auth_operation_options(&options));
		promise(async move {
			delete.await?;
			Ok(JsValue::UNDEFINED)
		})
	}
}

// Models stores.

/// A `ModelsStore` over a JS object.
struct JsModelsStore {
	object: JsValue,
}

fn models_store_options(options: &Option<ModelsStoreOperationOptions>) -> (JsValue, Vec<JsSignal>) {
	operation_options(options.as_ref().and_then(|o| o.signal.as_ref()), options.is_some())
}

impl ModelsStore for JsModelsStore {
	fn read(
		&self,
		provider_id: &str,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<Option<ModelsStoreEntry>>> {
		let (options, signals) = models_store_options(&options);
		let call = call_method(&self.object, "read", &[provider_id.into(), options], signals);
		Box::pin(async move { optional(call.await?) })
	}

	fn write(
		&self,
		provider_id: &str,
		entry: ModelsStoreEntry,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let (options, signals) = models_store_options(&options);
		let call = to_js(&entry)
			.map(|entry| call_method(&self.object, "write", &[provider_id.into(), entry, options], signals));
		Box::pin(async move { call?.await.map(|_| ()) })
	}

	fn delete(
		&self,
		provider_id: &str,
		options: Option<ModelsStoreOperationOptions>,
	) -> LocalBoxFuture<'static, PiResult<()>> {
		let (options, signals) = models_store_options(&options);
		let call = call_method(&self.object, "delete", &[provider_id.into(), options], signals);
		Box::pin(async move { call.await.map(|_| ()) })
	}
}

/// The Rust `ModelsStore` of a JS one.
pub fn models_store_from_js(object: &JsValue) -> Rc<dyn ModelsStore> {
	Rc::new(JsModelsStore { object: object.clone() })
}

/// A Rust `ModelsStore`, wrapped by the facade's store classes.
#[wasm_bindgen]
pub struct ModelsStoreHandle {
	inner: Rc<dyn ModelsStore>,
}

impl ModelsStoreHandle {
	pub fn new(inner: Rc<dyn ModelsStore>) -> Self {
		Self { inner }
	}
}

#[wasm_bindgen]
impl ModelsStoreHandle {
	/// `store.read(providerId, options)`.
	pub fn read(&self, provider_id: &str, options: JsValue) -> Promise {
		let read = self.inner.read(provider_id, models_store_operation_options(&options));
		promise(async move { read.await?.map_or(Ok(JsValue::UNDEFINED), |entry| to_js(&entry)) })
	}

	/// `store.write(providerId, entry, options)`.
	pub fn write(&self, provider_id: &str, entry: JsValue, options: JsValue) -> Promise {
		let options = models_store_operation_options(&options);
		let write = from_js::<ModelsStoreEntry>(entry).map(|entry| self.inner.write(provider_id, entry, options));
		promise(async move {
			write?.await?;
			Ok(JsValue::UNDEFINED)
		})
	}

	/// `store.delete(providerId, options)`.
	pub fn delete(&self, provider_id: &str, options: JsValue) -> Promise {
		let delete = self.inner.delete(provider_id, models_store_operation_options(&options));
		promise(async move {
			delete.await?;
			Ok(JsValue::UNDEFINED)
		})
	}
}
