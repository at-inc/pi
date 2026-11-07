//! Exports of `images`, `images-api-registry`, `image-models` and `providers/images/register-builtins`: the global image
//! generation dispatched through the images api registry, and the image catalog reads.

use std::rc::Rc;

use js_sys::{Promise, Reflect};
use pi_ai::images_api_registry::{ImagesApiProvider, ImagesApiProviderInternal};
use pi_ai::{ImageModel, ProviderImages};
use wasm_bindgen::prelude::*;

use super::provider_bridge::{JsImages, images_options};
use crate::options::{arg, throw};

/// `generateImages(model, context, options)` (`images`).
#[wasm_bindgen(js_name = aiGenerateImages)]
pub fn generate_images(model: JsValue, context: JsValue, options: JsValue) -> Promise {
	let args = (|| Ok::<_, JsValue>((arg::<ImageModel>(model.clone())?, arg(context)?)))();
	let options = images_options(options, &model);
	let pending = args.map(|(model, context)| pi_ai::images::generate_images(&model, &context, options));
	future(async move { crate::json::to_js(&pending.map_err(crate::error::from_js)?.await?) })
}

/// `registerImagesApiProvider(provider, sourceId)`: a JS `{ api, generateImages }`, called as JS.
#[wasm_bindgen(js_name = aiRegisterImagesApiProvider)]
pub fn register_images_api_provider(provider: JsValue, source_id: Option<String>) -> Result<(), JsValue> {
	let api = arg(Reflect::get(&provider, &"api".into()).unwrap_or(JsValue::UNDEFINED))?;
	let images = Rc::new(JsImages { object: provider });
	pi_ai::images_api_registry::register_images_api_provider(
		ImagesApiProvider {
			api,
			generate_images: Rc::new(move |model, context, options| images.generate_images(model, context, options)),
		},
		source_id.as_deref(),
	);
	Ok(())
}

/// A registered images api provider (`getImagesApiProvider(api)`).
#[wasm_bindgen]
pub struct ImagesApiProviderHandle {
	inner: ImagesApiProviderInternal,
}

#[wasm_bindgen]
impl ImagesApiProviderHandle {
	/// `provider.api`.
	#[wasm_bindgen(getter)]
	pub fn api(&self) -> String {
		self.inner.api.to_string()
	}

	/// `provider.generateImages(model, context, options)`.
	#[wasm_bindgen(js_name = generateImages)]
	pub fn generate_images(&self, model: JsValue, context: JsValue, options: JsValue) -> Promise {
		let generate = self.inner.generate_images.clone();
		let args = (|| Ok::<_, JsValue>((arg::<ImageModel>(model.clone())?, arg(context)?)))();
		let options = images_options(options, &model);
		future(async move {
			let (model, context) = args.map_err(crate::error::from_js)?;
			crate::json::to_js(&generate(&model, &context, options).await?)
		})
	}
}

/// `getImagesApiProvider(api)`.
#[wasm_bindgen(js_name = aiGetImagesApiProvider)]
pub fn get_images_api_provider(api: &str) -> Option<ImagesApiProviderHandle> {
	pi_ai::images_api_registry::get_images_api_provider(api).map(|inner| ImagesApiProviderHandle { inner })
}

/// `registerBuiltInImagesApiProviders()` (`providers/images/register-builtins`).
#[wasm_bindgen(js_name = aiRegisterBuiltInImagesApiProviders)]
pub fn register_built_in_images_api_providers() {
	pi_ai::providers::images::register_builtins::register_built_in_images_api_providers();
}

/// `generateImagesOpenAICodex` or `generateImagesOpenRouter` (`providers/images/register-builtins`), by API.
#[wasm_bindgen(js_name = aiGenerateImagesBuiltin)]
pub fn generate_images_builtin(api: &str, model: JsValue, context: JsValue, options: JsValue) -> Promise {
	use pi_ai::providers::images::register_builtins::{generate_images_open_router, generate_images_openai_codex};
	let generate = match api {
		"openai-codex-images" => generate_images_openai_codex,
		_ => generate_images_open_router,
	};
	let args = (|| Ok::<_, JsValue>((arg::<ImageModel>(model.clone())?, arg(context)?)))();
	let options = images_options(options, &model).map(|options| options.base);
	future(async move {
		let (model, context) = args.map_err(crate::error::from_js)?;
		crate::json::to_js(&generate(&model, &context, options).await?)
	})
}

/// `getImageModel(provider, modelId)` (`image-models`).
#[allow(deprecated, reason = "the export of the deprecated image-models module")]
#[wasm_bindgen(js_name = aiGetImageModel)]
pub fn get_image_model(provider: &str, model_id: &str) -> Result<JsValue, JsValue> {
	pi_ai::image_models::get_image_model(provider, model_id)
		.map_or(Ok(JsValue::UNDEFINED), |model| throw(crate::json::to_js(&model)))
}

/// `getImageProviders()` (`image-models`).
#[allow(deprecated, reason = "the export of the deprecated image-models module")]
#[wasm_bindgen(js_name = aiGetImageProviders)]
pub fn get_image_providers() -> Vec<String> {
	pi_ai::image_models::get_image_providers()
}

/// `getImageModels(provider)` (`image-models`).
#[allow(deprecated, reason = "the export of the deprecated image-models module")]
#[wasm_bindgen(js_name = aiGetImageModels)]
pub fn get_image_models(provider: &str) -> Result<JsValue, JsValue> {
	throw(crate::json::to_js(&pi_ai::image_models::get_image_models(provider)))
}

fn future(future: impl std::future::Future<Output = pi_host::PiResult<JsValue>> + 'static) -> Promise {
	wasm_bindgen_futures::future_to_promise(async move { future.await.map_err(|error| crate::error::to_js(&error)) })
}
