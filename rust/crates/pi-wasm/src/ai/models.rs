//! Exports of `models`: `createProvider` and `createModels` over Rust implementations, the handles the facade
//! classes own, and the module's helper functions (`calculateCost`, `getSupportedThinkingLevels`, ...).

use std::future::Future;
use std::rc::Rc;

use indexmap::IndexMap;
use js_sys::{Array, Function, Object, Promise, Reflect};
use pi_ai::auth::resolve::AuthResolutionOverrides;
use pi_ai::auth::types::{
	ApiKeyAuth, ApiKeyAuthInput, AuthContext, AuthOperationOptions, Credential, OAuthAuth, OAuthCredential,
	ProviderAuth,
};
use pi_ai::{
	AnyModel, BaseModel, ClassifierModel, CreateModelsOptions, CreateProviderOptions, DeferredHandle, ImageModel,
	Model, ModelThinkingLevel, ModelType, ModelsApiStreamOptions, ModelsClassifierOptions, ModelsDeferredCancelOptions,
	ModelsDeferredFetchOptions, ModelsImagesOptions, ModelsRefreshOptions, ModelsSimpleStreamOptions, MutableModels,
	Provider, ProviderApi, ProviderClassifier, ProviderImages, ProviderModel, ProviderStreams, Usage,
};
use pi_ai::utils::diagnostics::Thrown;
use pi_ai::utils::models_error::{ModelsError, ModelsErrorCode};
use pi_host::{AbortController, AbortSignal, PiError, PiResult};
use serde::Serialize;
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use super::provider_bridge::{
	JsAuthContext, JsClassifier, JsImages, classifier_options, images_options, js_provider, refresh_context_from_js,
};
use crate::events::EventSink;
use crate::options::{JsOptions, arg, provider_stream_options, simple_stream_options, throw};

/// A Rust API implementation (`ProviderStreams`), the value of `openAICompletionsApi()` and the other `*Api()`
/// factories. The facade wraps it in a JS `ProviderStreams` object that keeps it, so `createProvider` hands the Rust
/// implementation back to Rust without a JS hop.
#[wasm_bindgen]
pub struct NativeStreams {
	inner: Rc<dyn ProviderStreams>,
	is_native: bool,
}

impl NativeStreams {
	pub fn new(inner: Rc<dyn ProviderStreams>) -> Self {
		Self { inner, is_native: true }
	}
}

#[wasm_bindgen]
impl NativeStreams {
	/// Another handle to the same implementation, for passing by value.
	pub fn share(&self) -> NativeStreams {
		NativeStreams::new(self.inner.clone())
	}

	/// A placeholder for an `api` map entry that is a JS implementation.
	pub fn placeholder() -> NativeStreams {
		NativeStreams {
			inner: Rc::new(JsStreams {
				object: JsValue::UNDEFINED,
			}),
			is_native: false,
		}
	}

	/// `streams.stream(model, context, options)`.
	pub fn stream(&self, model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context = super::transcript(context)?;
		let options = provider_stream_options(options, &model);
		crate::events::send(throw(self.inner.stream(&parsed, &context, options))?, sink);
		Ok(())
	}

	/// `streams.streamSimple(model, context, options)`.
	#[wasm_bindgen(js_name = streamSimple)]
	pub fn stream_simple(
		&self,
		model: JsValue,
		context: JsValue,
		options: JsValue,
		sink: EventSink,
	) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context = super::transcript(context)?;
		let options = simple_stream_options(options, &model);
		crate::events::send(throw(self.inner.stream_simple(&parsed, &context, options))?, sink);
		Ok(())
	}

	/// Whether the implementation has `fetchDeferred`.
	#[wasm_bindgen(js_name = hasFetchDeferred)]
	pub fn has_fetch_deferred(&self) -> bool {
		self.inner.fetch_deferred().is_some()
	}

	/// Whether the implementation has `cancelDeferred`.
	#[wasm_bindgen(js_name = hasCancelDeferred)]
	pub fn has_cancel_deferred(&self) -> bool {
		self.inner.cancel_deferred().is_some()
	}

	/// `streams.fetchDeferred(model, handle, options)`.
	#[wasm_bindgen(js_name = fetchDeferred)]
	pub fn fetch_deferred(
		&self,
		model: JsValue,
		handle: JsValue,
		options: JsValue,
		sink: EventSink,
	) -> Result<(), JsValue> {
		let fetch = self
			.inner
			.fetch_deferred()
			.ok_or_else(|| JsValue::from(js_sys::Error::new("API does not support deferred responses")))?;
		let (parsed, handle) = (super::model(&model)?, arg::<DeferredHandle>(handle)?);
		let options = JsOptions::new(options);
		let options = options.is_given().then(|| options.deferred_fetch_options(&model));
		crate::events::send(throw(fetch(&parsed, &handle, options))?, sink);
		Ok(())
	}

	/// `streams.cancelDeferred(model, handle, options)`.
	#[wasm_bindgen(js_name = cancelDeferred)]
	pub fn cancel_deferred(&self, model: JsValue, handle: JsValue, options: JsValue) -> Promise {
		let cancel = self.inner.cancel_deferred();
		let args = (|| Ok::<_, JsValue>((super::model(&model)?, arg::<DeferredHandle>(handle)?)))();
		let options = JsOptions::new(options);
		let options = options.is_given().then(|| options.deferred_cancel_options(&model));
		promise(async move {
			let cancel = cancel.ok_or_else(|| PiError::new("API cannot cancel deferred responses"))?;
			let (model, handle) = args.map_err(crate::error::from_js)?;
			cancel(&model, &handle, options).await?;
			Ok(JsValue::UNDEFINED)
		})
	}
}

/// A Rust image-generation implementation (`ProviderImages`), the value of `openAICodexImagesApi()` and the other image
/// `*Api()` factories; like `NativeStreams`, the facade's JS value keeps it for `createProvider({ images })`.
#[wasm_bindgen]
pub struct NativeImages {
	inner: Rc<dyn ProviderImages>,
	is_native: bool,
}

impl NativeImages {
	pub fn new(inner: Rc<dyn ProviderImages>) -> Self {
		Self { inner, is_native: true }
	}
}

#[wasm_bindgen]
impl NativeImages {
	/// Another handle to the same implementation, for passing by value.
	pub fn share(&self) -> NativeImages {
		NativeImages::new(self.inner.clone())
	}

	/// A placeholder for an `images` map entry that is a JS implementation.
	pub fn placeholder() -> NativeImages {
		NativeImages {
			inner: Rc::new(JsImages {
				object: JsValue::UNDEFINED,
			}),
			is_native: false,
		}
	}

	/// `images.generateImages(model, context, options)`.
	#[wasm_bindgen(js_name = generateImages)]
	pub fn generate_images(&self, model: JsValue, context: JsValue, options: JsValue) -> Promise {
		let inner = self.inner.clone();
		let args = (|| Ok::<_, JsValue>((arg::<ImageModel>(model.clone())?, arg(context)?)))();
		let options = images_options(options, &model);
		promise(async move {
			let (model, context) = args.map_err(crate::error::from_js)?;
			crate::json::to_js(&inner.generate_images(&model, &context, options).await?)
		})
	}
}

/// A Rust classifier implementation (`ProviderClassifier`), the value of `llamaCppClassifyApi()` and the other
/// classifier `*Api()` factories.
#[wasm_bindgen]
pub struct NativeClassifier {
	inner: Rc<dyn ProviderClassifier>,
	is_native: bool,
}

impl NativeClassifier {
	pub fn new(inner: Rc<dyn ProviderClassifier>) -> Self {
		Self { inner, is_native: true }
	}
}

#[wasm_bindgen]
impl NativeClassifier {
	/// Another handle to the same implementation, for passing by value.
	pub fn share(&self) -> NativeClassifier {
		NativeClassifier::new(self.inner.clone())
	}

	/// A placeholder for a `classifiers` map entry that is a JS implementation.
	pub fn placeholder() -> NativeClassifier {
		NativeClassifier {
			inner: Rc::new(JsClassifier {
				object: JsValue::UNDEFINED,
			}),
			is_native: false,
		}
	}

	/// `classifier.classify(model, context, options)`.
	pub fn classify(&self, model: JsValue, context: JsValue, options: JsValue) -> Promise {
		let inner = self.inner.clone();
		let args = (|| Ok::<_, JsValue>((arg::<ClassifierModel>(model.clone())?, arg(context)?)))();
		let options = classifier_options(options, &model);
		promise(async move {
			let (model, context) = args.map_err(crate::error::from_js)?;
			crate::json::to_js(&inner.classify(&model, &context, options).await?)
		})
	}
}

/// A `ProviderStreams` written in JS (`{ stream, streamSimple }`), called with the model, the transcript and the
/// options as JS values; its JS event stream feeds a Rust one (`events::from_js`).
pub struct JsStreams {
	object: JsValue,
}

impl JsStreams {
	/// The Rust `ProviderStreams` of a JS object with `stream` and `streamSimple` (and optionally the deferred methods).
	pub fn new(object: JsValue) -> Self {
		Self { object }
	}

	fn call(
		&self,
		method: &str,
		model: &pi_ai::Model,
		context: &pi_ai::TranscriptContext,
		options: PiResult<Option<(JsValue, Option<crate::abort::JsSignal>)>>,
	) -> PiResult<pi_ai::AssistantMessageEventStream> {
		let function: Function = Reflect::get(&self.object, &method.into())
			.map_err(crate::error::from_js)?
			.dyn_into()
			.map_err(|_| pi_host::PiError::type_error(format!("{method} is not a function")))?;
		let (options, signal) = options?.unwrap_or((JsValue::UNDEFINED, None));
		let args = js_sys::Array::of3(&crate::json::to_js(model)?, &crate::json::to_js(context)?, &options);
		let source = function.apply(&self.object, &args).map_err(crate::error::from_js)?;
		Ok(crate::events::from_js(source, signal))
	}
}

impl ProviderStreams for JsStreams {
	fn stream(
		&self,
		model: &pi_ai::Model,
		context: &pi_ai::TranscriptContext,
		options: Option<pi_ai::ProviderStreamOptions>,
	) -> PiResult<pi_ai::AssistantMessageEventStream> {
		let options = options
			.map(|options| crate::options::stream_options_to_js(&options.base, &options.extra))
			.transpose();
		self.call("stream", model, context, options)
	}

	fn stream_simple(
		&self,
		model: &pi_ai::Model,
		context: &pi_ai::TranscriptContext,
		options: Option<pi_ai::SimpleStreamOptions>,
	) -> PiResult<pi_ai::AssistantMessageEventStream> {
		let options = options
			.map(|options| crate::options::simple_options_to_js(&options))
			.transpose();
		self.call("streamSimple", model, context, options)
	}

	fn fetch_deferred(&self) -> Option<pi_ai::FetchDeferredFunction> {
		has_method(&self.object, "fetchDeferred").then(|| super::provider_bridge::fetch_deferred(self.object.clone()))
	}

	fn cancel_deferred(&self) -> Option<pi_ai::CancelDeferredFunction> {
		has_method(&self.object, "cancelDeferred").then(|| super::provider_bridge::cancel_deferred(self.object.clone()))
	}
}

fn has_method(object: &JsValue, key: &str) -> bool {
	Reflect::get(object, &key.into()).is_ok_and(|value| value.is_function())
}

/// The chat implementation of `input.api` (or one entry of an `api` map): a facade `*Api()` value carries its
/// `NativeStreams` and is used directly; any other object is a JS implementation.
fn provider_streams(native: Option<NativeStreams>, object: JsValue) -> Rc<dyn ProviderStreams> {
	match native {
		Some(native) => native.inner,
		None => Rc::new(JsStreams { object }),
	}
}

/// `createProvider(input)`. The facade passes the plain fields, `auth` and the model callbacks in `input`, and each
/// implementation map separately: `single` (with its `NativeStreams` when it is a facade value) for one chat
/// implementation, or `apis` with the matching `objects` and `natives` for a map keyed by `model.api`; `images` and
/// `classifiers` the same way. Each `natives` list has an entry per key: `share()` of a facade value, or a placeholder
/// marked `isNative: false` for a JS implementation.
#[allow(
	clippy::too_many_arguments,
	reason = "one argument per implementation list, as wasm-bindgen passes them"
)]
#[wasm_bindgen(js_name = aiCreateProvider)]
pub fn create_provider(
	input: JsValue,
	single: JsValue,
	single_native: Option<NativeStreams>,
	apis: Vec<String>,
	objects: Vec<JsValue>,
	natives: Vec<NativeStreams>,
	image_apis: Vec<String>,
	image_objects: Vec<JsValue>,
	image_natives: Vec<NativeImages>,
	classifier_apis: Vec<String>,
	classifier_objects: Vec<JsValue>,
	classifier_natives: Vec<NativeClassifier>,
) -> Result<ProviderHandle, JsValue> {
	let field = |name: &str| Reflect::get(&input, &name.into()).unwrap_or(JsValue::UNDEFINED);
	let api = if single.is_object() {
		Some(ProviderApi::Single(provider_streams(single_native, single)))
	} else if apis.is_empty() {
		None
	} else {
		let entries = apis.into_iter().zip(objects).zip(natives);
		Some(ProviderApi::ByApi(
			entries
				.map(|((api, object), native)| {
					let native = native.is_native.then_some(native);
					(api, provider_streams(native, object))
				})
				.collect(),
		))
	};
	let images = (!image_apis.is_empty()).then(|| {
		let entries = image_apis.into_iter().zip(image_objects).zip(image_natives);
		entries
			.map(|((api, object), native)| {
				let inner: Rc<dyn ProviderImages> = if native.is_native {
					native.inner
				} else {
					Rc::new(JsImages { object })
				};
				(api, inner)
			})
			.collect::<IndexMap<_, _>>()
	});
	let classifiers = (!classifier_apis.is_empty()).then(|| {
		let entries = classifier_apis
			.into_iter()
			.zip(classifier_objects)
			.zip(classifier_natives);
		entries
			.map(|((api, object), native)| {
				let inner: Rc<dyn ProviderClassifier> = if native.is_native {
					native.inner
				} else {
					Rc::new(JsClassifier { object })
				};
				(api, inner)
			})
			.collect::<IndexMap<_, _>>()
	});
	let function = |name: &str| field(name).dyn_into::<Function>().ok();
	let provider = throw(pi_ai::create_provider(CreateProviderOptions {
		id: arg(field("id"))?,
		name: optional(field("name"))?,
		base_url: optional(field("baseUrl"))?,
		headers: optional(field("headers"))?,
		auth: provider_auth(&field("auth")),
		models: optional::<Vec<ProviderModel>>(field("models"))?.unwrap_or_default(),
		fetch_models: function("fetchModels").map(|fetch| super::provider_bridge::fetch_models(fetch, input.clone())),
		filter_models: function("filterModels")
			.map(|filter| super::provider_bridge::filter_models(filter, input.clone())),
		filter_all_models: function("filterAllModels")
			.map(|filter| super::provider_bridge::filter_all_models(filter, input.clone())),
		api,
		images,
		classifiers,
	}))?;
	Ok(ProviderHandle {
		inner: Rc::new(provider),
	})
}

/// An optional argument field: `undefined` is `None`.
fn optional<T: serde::de::DeserializeOwned>(value: JsValue) -> Result<Option<T>, JsValue> {
	if value.is_undefined() {
		Ok(None)
	} else {
		arg(value).map(Some)
	}
}

/// `ProviderAuth` of a JS `auth` object. `apiKey.resolve` and `apiKey.check` are bridged; `login` and `oauth` belong
/// to the auth facade and are not bridged yet.
pub fn provider_auth(auth: &JsValue) -> ProviderAuth {
	let api_key = Reflect::get(auth, &"apiKey".into()).unwrap_or(JsValue::UNDEFINED);
	if !api_key.is_object() {
		return ProviderAuth::default();
	}
	let method = |name: &str| Reflect::get(&api_key, &name.into()).ok()?.dyn_into::<Function>().ok();
	let name = Reflect::get(&api_key, &"name".into())
		.ok()
		.and_then(|name| name.as_string())
		.unwrap_or_default();
	let resolve = method("resolve");
	let check = method("check");
	let this = api_key.clone();
	ProviderAuth {
		api_key: Some(ApiKeyAuth {
			name,
			login: None,
			check: check.map(|check| {
				let this = this.clone();
				Rc::new(move |input: ApiKeyAuthInput| {
					let (check, this) = (check.clone(), this.clone());
					Box::pin(async move { optional_result(&check, &this, input).await }) as pi_host::LocalBoxFuture<_>
				}) as pi_ai::auth::types::ApiKeyCheckFunction
			}),
			resolve: match resolve {
				Some(resolve) => Rc::new(move |input: ApiKeyAuthInput| {
					let (resolve, this) = (resolve.clone(), this.clone());
					Box::pin(async move { optional_result(&resolve, &this, input).await })
				}),
				None => Rc::new(|_| Box::pin(async { Ok(None) })),
			},
		}),
		oauth: None,
	}
}

/// Calls an api-key auth method with `{ ctx, credential, signal }`; `undefined` is `None`.
async fn optional_result<T: serde::de::DeserializeOwned>(
	function: &Function,
	this: &JsValue,
	input: ApiKeyAuthInput,
) -> PiResult<Option<T>> {
	let object = Object::new();
	let ctx: JsValue = AuthContextHandle { inner: input.ctx }.into();
	let _ = Reflect::set(&object, &"ctx".into(), &ctx);
	if let Some(credential) = &input.credential {
		let _ = Reflect::set(&object, &"credential".into(), &crate::json::to_js(credential)?);
	}
	let signal = crate::abort::to_js(&input.signal);
	let _ = Reflect::set(&object, &"signal".into(), &signal.js);
	let result = crate::callback::call(function, this, &[object.into()]).await?;
	if result.is_undefined() || result.is_null() {
		return Ok(None);
	}
	crate::json::from_js(result).map(Some)
}

/// The `AuthContext` an api-key auth method receives as `ctx`.
#[wasm_bindgen]
pub struct AuthContextHandle {
	inner: Rc<dyn AuthContext>,
}

#[wasm_bindgen]
impl AuthContextHandle {
	/// `ctx.env(name)`.
	pub fn env(&self, name: String) -> Promise {
		let pending = self.inner.env(&name);
		wasm_bindgen_futures::future_to_promise(
			async move { Ok(pending.await.map_or(JsValue::UNDEFINED, JsValue::from)) },
		)
	}

	/// `ctx.fileExists(path)`.
	#[wasm_bindgen(js_name = fileExists)]
	pub fn file_exists(&self, path: String) -> Promise {
		let pending = self.inner.file_exists(&path);
		wasm_bindgen_futures::future_to_promise(async move { Ok(JsValue::from(pending.await)) })
	}
}

/// A Rust `Provider`, owned by the facade's `Provider` object.
#[wasm_bindgen]
pub struct ProviderHandle {
	inner: Rc<Provider>,
}

#[wasm_bindgen]
impl ProviderHandle {
	/// Another handle to the same provider, for passing by value.
	pub fn share(&self) -> ProviderHandle {
		ProviderHandle {
			inner: self.inner.clone(),
		}
	}

	/// The plain fields: `{ id, name, baseUrl?, headers? }`.
	pub fn fields(&self) -> Result<JsValue, JsValue> {
		let object = Object::new();
		let _ = Reflect::set(&object, &"id".into(), &self.inner.id.as_str().into());
		let _ = Reflect::set(&object, &"name".into(), &self.inner.name.as_str().into());
		if let Some(base_url) = &self.inner.base_url {
			let _ = Reflect::set(&object, &"baseUrl".into(), &base_url.as_str().into());
		}
		if let Some(headers) = &self.inner.headers {
			let _ = Reflect::set(&object, &"headers".into(), &throw(crate::json::to_js(headers))?);
		}
		Ok(object.into())
	}

	/// The names of the optional members the provider has (`getAllModels`, `refreshModels`, `filterModels`,
	/// `filterAllModels`, `fetchDeferred`, `cancelDeferred`, `generateImages`, `classify`).
	pub fn members(&self) -> Vec<String> {
		let provider = &self.inner;
		[
			("getAllModels", provider.get_all_models.is_some()),
			("refreshModels", provider.refresh_models.is_some()),
			("filterModels", provider.filter_models.is_some()),
			("filterAllModels", provider.filter_all_models.is_some()),
			("fetchDeferred", provider.fetch_deferred.is_some()),
			("cancelDeferred", provider.cancel_deferred.is_some()),
			("generateImages", provider.generate_images.is_some()),
			("classify", provider.classify.is_some()),
		]
		.into_iter()
		.filter_map(|(name, present)| present.then(|| name.to_string()))
		.collect()
	}

	/// The provider's api-key auth, for the JS view of `provider.auth`.
	#[wasm_bindgen(js_name = apiKeyAuth)]
	pub fn api_key_auth(&self) -> Option<ApiKeyAuthHandle> {
		self.inner.auth.api_key.clone().map(|inner| ApiKeyAuthHandle { inner })
	}

	/// The provider's OAuth auth, for the JS view of `provider.auth`.
	#[wasm_bindgen(js_name = oauthAuth)]
	pub fn oauth_auth(&self) -> Option<OAuthAuthHandle> {
		self.inner.auth.oauth.clone().map(|inner| OAuthAuthHandle { inner })
	}

	/// `provider.getModels()`.
	#[wasm_bindgen(js_name = getModels)]
	pub fn get_models(&self) -> Result<JsValue, JsValue> {
		throw(crate::json::to_js(&(self.inner.get_models)()))
	}

	/// `provider.getAllModels()`.
	#[wasm_bindgen(js_name = getAllModels)]
	pub fn get_all_models(&self) -> Result<JsValue, JsValue> {
		let models = match &self.inner.get_all_models {
			Some(get_all_models) => get_all_models(),
			None => (self.inner.get_models)().into_iter().map(AnyModel::Chat).collect(),
		};
		throw(crate::json::to_js(&models))
	}

	/// `provider.refreshModels(context)`.
	#[wasm_bindgen(js_name = refreshModels)]
	pub fn refresh_models(&self, context: JsValue) -> Promise {
		let refresh = self.inner.refresh_models.clone();
		let context = refresh_context_from_js(&context);
		promise(async move {
			if let Some(refresh) = refresh {
				refresh(context?).await?;
			}
			Ok(JsValue::UNDEFINED)
		})
	}

	/// `provider.filterModels(models, credential)`.
	#[wasm_bindgen(js_name = filterModels)]
	pub fn filter_models(&self, models: JsValue, credential: JsValue) -> Result<JsValue, JsValue> {
		let models: Vec<Model> = arg(models)?;
		let credential: Option<Credential> = optional(credential)?;
		let filtered = match &self.inner.filter_models {
			Some(filter) => filter(&models, credential.as_ref()),
			None => models,
		};
		throw(crate::json::to_js(&filtered))
	}

	/// `provider.filterAllModels(models, credential)`.
	#[wasm_bindgen(js_name = filterAllModels)]
	pub fn filter_all_models(&self, models: JsValue, credential: JsValue) -> Result<JsValue, JsValue> {
		let models: Vec<AnyModel> = arg(models)?;
		let credential: Option<Credential> = optional(credential)?;
		let filtered = match &self.inner.filter_all_models {
			Some(filter) => filter(&models, credential.as_ref()),
			None => models,
		};
		throw(crate::json::to_js(&filtered))
	}

	/// `provider.stream(model, context, options)`.
	pub fn stream(&self, model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context = super::transcript(context)?;
		let options = provider_stream_options(options, &model);
		crate::events::send(throw((self.inner.stream)(&parsed, &context, options))?, sink);
		Ok(())
	}

	/// `provider.streamSimple(model, context, options)`.
	#[wasm_bindgen(js_name = streamSimple)]
	pub fn stream_simple(
		&self,
		model: JsValue,
		context: JsValue,
		options: JsValue,
		sink: EventSink,
	) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context = super::transcript(context)?;
		let options = simple_stream_options(options, &model);
		crate::events::send(throw((self.inner.stream_simple)(&parsed, &context, options))?, sink);
		Ok(())
	}

	/// `provider.fetchDeferred(model, handle, options)`.
	#[wasm_bindgen(js_name = fetchDeferred)]
	pub fn fetch_deferred(
		&self,
		model: JsValue,
		handle: JsValue,
		options: JsValue,
		sink: EventSink,
	) -> Result<(), JsValue> {
		let fetch = self
			.inner
			.fetch_deferred
			.clone()
			.ok_or_else(|| JsValue::from(js_sys::Error::new("fetchDeferred is not a function")))?;
		let (parsed, handle) = (super::model(&model)?, arg::<DeferredHandle>(handle)?);
		let options = JsOptions::new(options);
		let options = options.is_given().then(|| options.deferred_fetch_options(&model));
		crate::events::send(throw(fetch(&parsed, &handle, options))?, sink);
		Ok(())
	}

	/// `provider.cancelDeferred(model, handle, options)`.
	#[wasm_bindgen(js_name = cancelDeferred)]
	pub fn cancel_deferred(&self, model: JsValue, handle: JsValue, options: JsValue) -> Promise {
		let cancel = self.inner.cancel_deferred.clone();
		let args = (|| Ok::<_, JsValue>((super::model(&model)?, arg::<DeferredHandle>(handle)?)))();
		let options = JsOptions::new(options);
		let options = options.is_given().then(|| options.deferred_cancel_options(&model));
		promise(async move {
			let cancel = cancel.ok_or_else(|| PiError::type_error("cancelDeferred is not a function"))?;
			let (model, handle) = args.map_err(crate::error::from_js)?;
			cancel(&model, &handle, options).await?;
			Ok(JsValue::UNDEFINED)
		})
	}

	/// `provider.generateImages(model, context, options)`.
	#[wasm_bindgen(js_name = generateImages)]
	pub fn generate_images(&self, model: JsValue, context: JsValue, options: JsValue) -> Promise {
		let generate = self.inner.generate_images.clone();
		let args = (|| Ok::<_, JsValue>((arg::<ImageModel>(model.clone())?, arg(context)?)))();
		let options = images_options(options, &model).map(|options| options.base);
		promise(async move {
			let generate = generate.ok_or_else(|| PiError::type_error("generateImages is not a function"))?;
			let (model, context) = args.map_err(crate::error::from_js)?;
			crate::json::to_js(&generate(&model, &context, options).await?)
		})
	}

	/// `provider.classify(model, context, options)`.
	pub fn classify(&self, model: JsValue, context: JsValue, options: JsValue) -> Promise {
		let classify = self.inner.classify.clone();
		let args = (|| Ok::<_, JsValue>((arg::<ClassifierModel>(model.clone())?, arg(context)?)))();
		let options = classifier_options(options, &model);
		promise(async move {
			let classify = classify.ok_or_else(|| PiError::type_error("classify is not a function"))?;
			let (model, context) = args.map_err(crate::error::from_js)?;
			crate::json::to_js(&classify(&model, &context, options).await?)
		})
	}
}

/// A Rust provider's api-key auth, behind `provider.auth.apiKey` of a Rust provider.
#[wasm_bindgen]
pub struct ApiKeyAuthHandle {
	inner: ApiKeyAuth,
}

#[wasm_bindgen]
impl ApiKeyAuthHandle {
	/// `apiKey.name`.
	#[wasm_bindgen(getter)]
	pub fn name(&self) -> String {
		self.inner.name.clone()
	}

	/// Whether the auth has `login`.
	#[wasm_bindgen(js_name = hasLogin)]
	pub fn has_login(&self) -> bool {
		self.inner.login.is_some()
	}

	/// Whether the auth has `check`.
	#[wasm_bindgen(js_name = hasCheck)]
	pub fn has_check(&self) -> bool {
		self.inner.check.is_some()
	}

	/// `apiKey.resolve({ ctx, credential, signal })`.
	pub fn resolve(&self, input: JsValue) -> Promise {
		let resolve = self.inner.resolve.clone();
		let input = api_key_input(&input);
		promise(async move { to_js_optional(resolve(input?).await?) })
	}

	/// `apiKey.check({ ctx, credential, signal })`.
	pub fn check(&self, input: JsValue) -> Promise {
		let check = self.inner.check.clone();
		let input = api_key_input(&input);
		promise(async move {
			let check = check.ok_or_else(|| PiError::type_error("check is not a function"))?;
			to_js_optional(check(input?).await?)
		})
	}
}

/// The Rust `ApiKeyAuthInput` of a JS `{ ctx, credential, signal }`.
fn api_key_input(input: &JsValue) -> PiResult<ApiKeyAuthInput> {
	let field = |name: &str| Reflect::get(input, &name.into()).unwrap_or(JsValue::UNDEFINED);
	let credential = field("credential");
	Ok(ApiKeyAuthInput {
		ctx: Rc::new(JsAuthContext { object: field("ctx") }),
		credential: if credential.is_undefined() || credential.is_null() {
			None
		} else {
			Some(crate::json::from_js(credential)?)
		},
		signal: signal_or_never(&field("signal")),
	})
}

/// A Rust provider's OAuth auth, behind `provider.auth.oauth` of a Rust provider. `login` belongs to the auth facade.
#[wasm_bindgen]
pub struct OAuthAuthHandle {
	inner: OAuthAuth,
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

	/// `oauth.refresh(credential, signal)`.
	pub fn refresh(&self, credential: JsValue, signal: JsValue) -> Promise {
		let refresh = self.inner.refresh.clone();
		let credential = crate::json::from_js::<OAuthCredential>(credential);
		let signal = signal_or_never(&signal);
		promise(async move { crate::json::to_js(&refresh(credential?, signal).await?) })
	}

	/// `oauth.toAuth(credential)`.
	#[wasm_bindgen(js_name = toAuth)]
	pub fn to_auth(&self, credential: JsValue) -> Promise {
		let to_auth = self.inner.to_auth.clone();
		let credential = crate::json::from_js::<OAuthCredential>(credential);
		promise(async move { crate::json::to_js(&to_auth(credential?).await?) })
	}
}

/// `createModels(options)`. Only `identity` is bridged; `credentials`, `modelsStore` and `authContext` belong to the
/// auth facade.
#[wasm_bindgen(js_name = aiCreateModels)]
pub fn create_models(options: JsValue) -> Result<ModelsHandle, JsValue> {
	Ok(ModelsHandle {
		inner: pi_ai::create_models(Some(models_options(&options)?)),
	})
}

/// `CreateModelsOptions` of the JS options; only `identity` is bridged.
fn models_options(options: &JsValue) -> Result<CreateModelsOptions, JsValue> {
	let identity = Reflect::get(options, &"identity".into()).unwrap_or(JsValue::UNDEFINED);
	Ok(CreateModelsOptions {
		identity: optional(identity)?,
		..CreateModelsOptions::default()
	})
}

/// `builtinModels(options)` (`providers/all`): a `Models` collection with every built-in provider.
#[wasm_bindgen(js_name = aiBuiltinModels)]
pub fn builtin_models(options: JsValue) -> Result<ModelsHandle, JsValue> {
	Ok(ModelsHandle {
		inner: pi_ai::providers::all::builtin_models(Some(models_options(&options)?)),
	})
}

/// A Rust provider of `providers/*` as a handle, for the factory exports.
pub fn provider_handle(provider: Provider) -> ProviderHandle {
	ProviderHandle {
		inner: Rc::new(provider),
	}
}

/// The message of `new ModelsError(code, message, { cause })`: `message` with the cause's detail, as the TS computes it.
#[wasm_bindgen(js_name = aiModelsErrorMessage)]
pub fn models_error_message(message: &str, cause: JsValue) -> String {
	let code = ModelsErrorCode::Provider;
	if cause.is_undefined() {
		return message.to_string();
	}
	if cause.is_instance_of::<js_sys::Error>() {
		let error = crate::error::from_js(cause);
		return ModelsError::new(code, message, Some(Thrown::Error(&error))).message;
	}
	// `null` stays a JSON null, which carries no detail; any other value's detail is its JS `String(value)`.
	let value = if cause.is_null() {
		Value::Null
	} else {
		Value::String(crate::error::js_string(&cause))
	};
	ModelsError::new(code, message, Some(Thrown::Value(&value))).message
}

/// A Rust `MutableModels`, owned by the facade's `Models` object.
#[wasm_bindgen]
pub struct ModelsHandle {
	inner: Rc<dyn MutableModels>,
}

#[wasm_bindgen]
impl ModelsHandle {
	/// `models.setProvider(provider)`.
	#[wasm_bindgen(js_name = setProvider)]
	pub fn set_provider(&self, provider: ProviderHandle) {
		self.inner.set_provider(provider.inner);
	}

	/// `models.setProvider(provider)` of a provider written in JS.
	#[wasm_bindgen(js_name = setJsProvider)]
	pub fn set_js_provider(&self, provider: JsValue) -> Result<(), JsValue> {
		self.inner.set_provider(Rc::new(js_provider(provider)?));
		Ok(())
	}

	/// The registered providers as handles, in registration order.
	#[wasm_bindgen(js_name = providerHandles)]
	pub fn provider_handles(&self) -> Vec<ProviderHandle> {
		self.inner
			.get_providers()
			.into_iter()
			.map(|inner| ProviderHandle { inner })
			.collect()
	}

	/// `models.deleteProvider(id)`.
	#[wasm_bindgen(js_name = deleteProvider)]
	pub fn delete_provider(&self, id: &str) {
		self.inner.delete_provider(id);
	}

	/// `models.clearProviders()`.
	#[wasm_bindgen(js_name = clearProviders)]
	pub fn clear_providers(&self) {
		self.inner.clear_providers();
	}

	/// `models.getModels(provider)`.
	#[wasm_bindgen(js_name = getModels)]
	pub fn get_models(&self, provider: Option<String>) -> Result<JsValue, JsValue> {
		throw(crate::json::to_js(&self.inner.get_models(provider.as_deref())))
	}

	/// `models.getModel(provider, id)`.
	#[wasm_bindgen(js_name = getModel)]
	pub fn get_model(&self, provider: &str, id: &str) -> Result<JsValue, JsValue> {
		to_js_optional(self.inner.get_model(provider, id)).map_err(|error| crate::error::to_js(&error))
	}

	/// `models.getAllModels(provider)`.
	#[wasm_bindgen(js_name = getAllModels)]
	pub fn get_all_models(&self, provider: Option<String>) -> Result<JsValue, JsValue> {
		throw(crate::json::to_js(&self.inner.get_all_models(provider.as_deref())))
	}

	/// `models.getModelsOfType(type, provider)`.
	#[wasm_bindgen(js_name = getModelsOfType)]
	pub fn get_models_of_type(&self, model_type: JsValue, provider: Option<String>) -> Result<JsValue, JsValue> {
		let models = self.inner.get_models_of_type(arg(model_type)?, provider.as_deref());
		throw(crate::json::to_js(&models))
	}

	/// `models.getModelOfType(type, provider, id)`.
	#[wasm_bindgen(js_name = getModelOfType)]
	pub fn get_model_of_type(&self, model_type: JsValue, provider: &str, id: &str) -> Result<JsValue, JsValue> {
		let model = self.inner.get_model_of_type(arg(model_type)?, provider, id);
		to_js_optional(model).map_err(|error| crate::error::to_js(&error))
	}

	/// `models.refresh(options)`: resolves `{ aborted, errors }` with `errors` as `[providerId, error]` pairs.
	pub fn refresh(&self, options: JsValue) -> Promise {
		let field = |name: &str| Reflect::get(&options, &name.into()).unwrap_or(JsValue::UNDEFINED);
		let parsed = (|| {
			Ok::<_, JsValue>(ModelsRefreshOptions {
				allow_network: optional(field("allowNetwork"))?,
				providers: optional(field("providers"))?,
				force: optional(field("force"))?,
				signal: crate::abort::from_js(&field("signal")),
			})
		})();
		let models = self.inner.clone();
		promise(async move {
			let options = parsed.map_err(crate::error::from_js)?;
			let result = models.refresh(Some(options)).await?;
			let errors: Array = result
				.errors
				.iter()
				.map(|(id, error)| JsValue::from(Array::of2(&id.as_str().into(), &crate::error::to_js(error))))
				.collect();
			let object = Object::new();
			let _ = Reflect::set(&object, &"aborted".into(), &JsValue::from(result.aborted));
			let _ = Reflect::set(&object, &"errors".into(), &errors);
			Ok(object.into())
		})
	}

	/// `models.checkAuth(providerId, options)`.
	#[wasm_bindgen(js_name = checkAuth)]
	pub fn check_auth(&self, provider_id: &str, options: JsValue) -> Promise {
		let pending = self.inner.check_auth(provider_id, operation_options(&options));
		promise(async move { to_js_optional(pending.await?) })
	}

	/// `models.getAvailable(providerId, options)`.
	#[wasm_bindgen(js_name = getAvailable)]
	pub fn get_available(&self, provider_id: Option<String>, options: JsValue) -> Promise {
		let pending = self
			.inner
			.get_available(provider_id.as_deref(), operation_options(&options));
		promise(async move { crate::json::to_js(&pending.await?) })
	}

	/// `models.getAvailableOfType(type, providerId, options)`.
	#[wasm_bindgen(js_name = getAvailableOfType)]
	pub fn get_available_of_type(&self, model_type: JsValue, provider_id: Option<String>, options: JsValue) -> Promise {
		let model_type: Result<ModelType, JsValue> = arg(model_type);
		let pending = model_type.map(|model_type| {
			self.inner
				.get_available_of_type(model_type, provider_id.as_deref(), operation_options(&options))
		});
		promise(async move {
			let pending = pending.map_err(crate::error::from_js)?;
			crate::json::to_js(&pending.await?)
		})
	}

	/// `models.getAllAvailable(providerId, options)`.
	#[wasm_bindgen(js_name = getAllAvailable)]
	pub fn get_all_available(&self, provider_id: Option<String>, options: JsValue) -> Promise {
		let pending = self
			.inner
			.get_all_available(provider_id.as_deref(), operation_options(&options));
		promise(async move { crate::json::to_js(&pending.await?) })
	}

	/// `models.getAuth(providerIdOrModel, overrides)`.
	#[wasm_bindgen(js_name = getAuth)]
	pub fn get_auth(&self, target: JsValue, overrides: JsValue) -> Promise {
		let field = |name: &str| Reflect::get(&overrides, &name.into()).unwrap_or(JsValue::UNDEFINED);
		let parsed = (|| {
			Ok::<_, JsValue>(AuthResolutionOverrides {
				api_key: optional(field("apiKey"))?,
				env: optional(field("env"))?,
				min_oauth_validity_ms: optional(field("minOAuthValidityMs"))?,
				signal: crate::abort::from_js(&field("signal")),
			})
		})();
		let pending = match (target.as_string(), parsed) {
			(_, Err(error)) => Err(error),
			(Some(provider), Ok(overrides)) => Ok(self
				.inner
				.get_auth(pi_ai::GetAuthTarget::Provider(&provider), Some(overrides))),
			(None, Ok(overrides)) => arg::<AnyModel>(target).map(|model| {
				self.inner
					.get_auth(pi_ai::GetAuthTarget::Model(&model), Some(overrides))
			}),
		};
		promise(async move {
			let pending = pending.map_err(crate::error::from_js)?;
			to_js_optional(pending.await?)
		})
	}

	/// `models.stream(model, context, options)`.
	pub fn stream(&self, model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context: pi_ai::Context = arg(context)?;
		let options = models_request(options, |options| ModelsApiStreamOptions {
			transform_headers: options.transform_headers(),
			base: options.provider_stream_options(&model),
		});
		crate::events::send(self.inner.stream(&parsed, &context, options), sink);
		Ok(())
	}

	/// `models.streamSimple(model, context, options)`.
	#[wasm_bindgen(js_name = streamSimple)]
	pub fn stream_simple(
		&self,
		model: JsValue,
		context: JsValue,
		options: JsValue,
		sink: EventSink,
	) -> Result<(), JsValue> {
		let parsed = super::model(&model)?;
		let context: pi_ai::Context = arg(context)?;
		let options = models_request(options, |options| ModelsSimpleStreamOptions {
			transform_headers: options.transform_headers(),
			base: options.simple_stream_options(&model),
		});
		crate::events::send(self.inner.stream_simple(&parsed, &context, options), sink);
		Ok(())
	}

	/// `models.streamDeferred(model, handle, options)`.
	#[wasm_bindgen(js_name = streamDeferred)]
	pub fn stream_deferred(
		&self,
		model: JsValue,
		handle: JsValue,
		options: JsValue,
		sink: EventSink,
	) -> Result<(), JsValue> {
		let (parsed, handle) = (super::model(&model)?, arg::<DeferredHandle>(handle)?);
		let options = models_request(options, |options| ModelsDeferredFetchOptions {
			transform_headers: options.transform_headers(),
			base: options.deferred_fetch_options(&model),
		});
		crate::events::send(self.inner.stream_deferred(&parsed, &handle, options), sink);
		Ok(())
	}

	/// `models.cancelDeferred(model, handle, options)`.
	#[wasm_bindgen(js_name = cancelDeferred)]
	pub fn cancel_deferred(&self, model: JsValue, handle: JsValue, options: JsValue) -> Promise {
		let args = (|| Ok::<_, JsValue>((super::model(&model)?, arg::<DeferredHandle>(handle)?)))();
		let options = models_request(options, |options| ModelsDeferredCancelOptions {
			transform_headers: options.transform_headers(),
			base: options.deferred_cancel_options(&model),
		});
		let pending = args.map(|(model, handle)| self.inner.cancel_deferred(&model, &handle, options));
		promise(async move {
			pending.map_err(crate::error::from_js)?.await?;
			Ok(JsValue::UNDEFINED)
		})
	}

	/// `models.generateImages(model, context, options)`. Never rejects for a readable model, as in the TS.
	#[wasm_bindgen(js_name = generateImages)]
	pub fn generate_images(&self, model: JsValue, context: JsValue, options: JsValue) -> Promise {
		let args = (|| Ok::<_, JsValue>((arg::<ImageModel>(model.clone())?, arg(context)?)))();
		let options = models_request(options, |options| ModelsImagesOptions {
			transform_headers: options.transform_headers(),
			base: options.images_options(&model).base,
		});
		let pending = args.map(|(model, context)| self.inner.generate_images(&model, &context, options));
		promise(async move { crate::json::to_js(&pending.map_err(crate::error::from_js)?.await) })
	}

	/// `models.classify(model, context, options)`. Never rejects for a readable model, as in the TS.
	pub fn classify(&self, model: JsValue, context: JsValue, options: JsValue) -> Promise {
		let args = (|| Ok::<_, JsValue>((arg::<ClassifierModel>(model.clone())?, arg(context)?)))();
		let options = models_request(options, |options| ModelsClassifierOptions {
			transform_headers: options.transform_headers(),
			base: options.classifier_options(&model),
		});
		let pending = args.map(|(model, context)| self.inner.classify(&model, &context, options));
		promise(async move { crate::json::to_js(&pending.map_err(crate::error::from_js)?.await) })
	}
}

/// The options of a `Models` request: the provider options plus `transformHeaders`; `None` when the caller passed none.
fn models_request<T>(options: JsValue, read: impl FnOnce(JsOptions) -> T) -> Option<T> {
	let options = JsOptions::new(options);
	options.is_given().then(|| read(options))
}

/// `AuthOperationOptions` (`{ signal }`) of a JS options object.
fn operation_options(options: &JsValue) -> Option<AuthOperationOptions> {
	let signal = Reflect::get(options, &"signal".into()).unwrap_or(JsValue::UNDEFINED);
	options.is_object().then(|| AuthOperationOptions {
		signal: crate::abort::from_js(&signal),
	})
}

/// The Rust signal of a JS signal, or one that never aborts.
fn signal_or_never(signal: &JsValue) -> AbortSignal {
	crate::abort::from_js(signal).unwrap_or_else(|| AbortController::new().signal())
}

/// A value as JS, `None` as `undefined`.
fn to_js_optional<T: Serialize>(value: Option<T>) -> PiResult<JsValue> {
	value.map_or(Ok(JsValue::UNDEFINED), |value| crate::json::to_js(&value))
}

/// A JS Promise of a Rust future, rejected with the JS error of a `PiError`.
fn promise(future: impl Future<Output = PiResult<JsValue>> + 'static) -> Promise {
	wasm_bindgen_futures::future_to_promise(async move { future.await.map_err(|error| crate::error::to_js(&error)) })
}

/// `hasApi(model, api)`.
#[wasm_bindgen(js_name = aiHasApi)]
pub fn has_api(model: JsValue, api: &str) -> Result<bool, JsValue> {
	Ok(pi_ai::has_api(&arg(model)?, api).is_some())
}

/// `calculateCost(model, usage)`: the cost the TS assigns to `usage.cost`; the facade copies it there.
#[wasm_bindgen(js_name = aiCalculateCost)]
pub fn calculate_cost(model: JsValue, usage: JsValue) -> Result<JsValue, JsValue> {
	let model: Box<BaseModel<Value>> = Box::new(arg(model)?);
	let mut usage: Usage = arg(usage)?;
	throw(crate::json::to_js(&pi_ai::calculate_cost(&model, &mut usage)))
}

/// `getSupportedThinkingLevels(model)`.
#[wasm_bindgen(js_name = aiGetSupportedThinkingLevels)]
pub fn get_supported_thinking_levels(model: JsValue) -> Result<JsValue, JsValue> {
	throw(crate::json::to_js(&pi_ai::get_supported_thinking_levels(&arg(model)?)))
}

/// `clampThinkingLevel(model, level)`.
#[wasm_bindgen(js_name = aiClampThinkingLevel)]
pub fn clamp_thinking_level(model: JsValue, level: JsValue) -> Result<JsValue, JsValue> {
	let level: ModelThinkingLevel = arg(level)?;
	throw(crate::json::to_js(&pi_ai::clamp_thinking_level(&arg(model)?, level)))
}

/// `modelsAreEqual(a, b)`.
#[wasm_bindgen(js_name = aiModelsAreEqual)]
pub fn models_are_equal(a: JsValue, b: JsValue) -> Result<bool, JsValue> {
	let (a, b): (Option<AnyModel>, Option<AnyModel>) = (optional_model(a)?, optional_model(b)?);
	Ok(pi_ai::models_are_equal(a.as_ref(), b.as_ref()))
}

/// `getModelType(model)`.
#[wasm_bindgen(js_name = aiGetModelType)]
pub fn get_model_type(model: JsValue) -> Result<String, JsValue> {
	let model: AnyModel = arg(model)?;
	Ok(pi_ai::utils::model_operations::get_model_type(&model).to_string())
}

/// `isModelType(model, type)`.
#[wasm_bindgen(js_name = aiIsModelType)]
pub fn is_model_type(model: JsValue, model_type: JsValue) -> Result<bool, JsValue> {
	let model: AnyModel = arg(model)?;
	let Ok(model_type) = crate::json::from_js::<ModelType>(model_type) else {
		return Ok(false);
	};
	Ok(pi_ai::utils::model_operations::is_model_type(&model, model_type))
}

/// `AI_TELEMETRY_SCHEMA`.
#[wasm_bindgen(js_name = aiTelemetrySchema)]
pub fn telemetry_schema() -> Result<JsValue, JsValue> {
	throw(crate::json::to_js(&*pi_ai::telemetry::AI_TELEMETRY_SCHEMA))
}

/// A model argument that may be `null`/`undefined`.
fn optional_model(value: JsValue) -> Result<Option<AnyModel>, JsValue> {
	if value.is_undefined() || value.is_null() {
		Ok(None)
	} else {
		arg(value).map(Some)
	}
}
