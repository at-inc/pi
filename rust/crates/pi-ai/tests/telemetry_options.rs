//! Port of `packages/ai/test/telemetry-options.test.ts`.
//!
//! Differences from the TS:
//! - "survives provider and Models stream/deferred dispatch" and "survives direct and Models image dispatch" are
//!   `#[ignore = "needs w2-ai-auth: resolve_provider_auth"]`: their `Models` half resolves auth through the stub on
//!   this branch. They pass against the auth branch.
//! - The `@ts-expect-error` check that `telemetryContext` no longer exists is not ported: it is a compile-time check,
//!   and `ProviderRequestOptions` has no such field by construction.
//! - Span identity (`toBe`) is address equality of the `Rc<dyn TelemetrySpan>`.
//! - `completedStream`'s `queueMicrotask` push is a spawned task; the stream is never ended, so its result resolves
//!   from the pushed `done` event, as in TS.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::simple_options::build_base_options;
use pi_ai::auth::types::{ApiKeyAuth, AuthResult, ModelAuth, ProviderAuth};
use pi_ai::images::generate_images;
use pi_ai::images_api_registry::{ImagesApiProvider, register_images_api_provider};
use pi_ai::models::{
	CreateProviderOptions, ModelsApiStreamOptions, ModelsDeferredCancelOptions, ModelsDeferredFetchOptions,
	ModelsImagesOptions, ModelsSimpleStreamOptions, ProviderApi, create_models, create_provider,
};
use pi_ai::types::{
	AnyModel, AssistantImages, AssistantMessage, AssistantMessageEvent, BaseModel, CancelDeferredFunction, Context,
	DeferredCancelOptions, DeferredFetchOptions, DeferredHandle, FetchDeferredFunction, ImageModel, ImagesContext,
	ImagesOptions, ImagesStopReason, Model, ModelCost, ModelInputModality, ModelType, ProviderImages,
	ProviderImagesOptions, ProviderRequestOptions, ProviderStreamOptions, ProviderStreams, SimpleStreamOptions,
	StopReason, StreamOptions, TextContent, TranscriptContext, Usage, UserContent,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::transcript::normalize_context;
use pi_host::testing::run;
use pi_host::{LocalBoxFuture, PiResult};
use pi_telemetry::{NOOP_TELEMETRY_CONTEXT, TelemetrySpan};

fn telemetry() -> Rc<dyn TelemetrySpan> {
	NOOP_TELEMETRY_CONTEXT.with(Rc::clone)
}

fn is_telemetry(value: Option<&Rc<dyn TelemetrySpan>>) -> bool {
	value.is_some_and(|value| std::ptr::addr_eq(Rc::as_ptr(value), Rc::as_ptr(&telemetry())))
}

fn context() -> TranscriptContext {
	normalize_context(&Context::default())
}

fn images_context() -> ImagesContext {
	ImagesContext {
		input: vec![UserContent::Text(TextContent {
			text: "circle".into(),
			..TextContent::default()
		})],
	}
}

fn model() -> Model {
	Model {
		base: BaseModel {
			id: "model".into(),
			name: "Model".into(),
			api: "telemetry-test".into(),
			provider: "telemetry-provider".into(),
			base_url: "https://example.test".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		reasoning: false,
		context_window: 1000.0,
		max_tokens: 100.0,
		..Model::default()
	}
}

fn image_model() -> ImageModel {
	ImageModel {
		base: BaseModel {
			id: "image-model".into(),
			name: "Image Model".into(),
			api: "telemetry-test-images".into(),
			provider: "telemetry-image-provider".into(),
			base_url: "https://example.test".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		r#type: ModelType::Image,
		output: vec![ModelInputModality::Image],
		extra: Default::default(),
	}
}

fn completed_stream(request_model: &Model) -> AssistantMessageEventStream {
	let stream = AssistantMessageEventStream::new();
	let message = AssistantMessage {
		content: Vec::new(),
		api: request_model.api.clone(),
		provider: request_model.provider.clone(),
		model: request_model.id.clone(),
		usage: Usage::default(),
		stop_reason: StopReason::Stop,
		timestamp: 0.0,
		..AssistantMessage::default()
	};
	let target = stream.clone();
	pi_host::spawn_local(async move {
		target.push(AssistantMessageEvent::Done {
			reason: StopReason::Stop,
			message: Rc::new(RefCell::new(message)),
		});
	});
	stream
}

fn request_options() -> ProviderRequestOptions<Model> {
	ProviderRequestOptions {
		telemetry: Some(telemetry()),
		..ProviderRequestOptions::default()
	}
}

fn stream_options() -> StreamOptions {
	StreamOptions {
		base: request_options(),
		..StreamOptions::default()
	}
}

fn simple_options() -> SimpleStreamOptions {
	SimpleStreamOptions {
		base: stream_options(),
		..SimpleStreamOptions::default()
	}
}

fn provider_stream_options() -> ProviderStreamOptions {
	ProviderStreamOptions {
		base: stream_options(),
		..ProviderStreamOptions::default()
	}
}

fn deferred_options() -> DeferredFetchOptions {
	DeferredFetchOptions {
		base: request_options(),
		wait: None,
	}
}

fn images_options() -> ProviderImagesOptions {
	ProviderImagesOptions {
		base: ImagesOptions {
			base: ProviderRequestOptions {
				telemetry: Some(telemetry()),
				..ProviderRequestOptions::default()
			},
			metadata: None,
		},
		extra: Default::default(),
	}
}

/// `{ apiKey: { name: "Test", resolve: async () => ({ auth: {} }) } }`.
fn ambient_auth() -> ProviderAuth {
	ProviderAuth {
		api_key: Some(ApiKeyAuth {
			name: "Test".into(),
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
		}),
		oauth: None,
	}
}

type Observed = Rc<RefCell<Vec<Option<Rc<dyn TelemetrySpan>>>>>;

/// The chat API that records the telemetry option of every call.
struct ObservingStreams {
	observed: Observed,
}

impl ProviderStreams for ObservingStreams {
	fn stream(
		&self,
		model: &Model,
		_context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		self.observed
			.borrow_mut()
			.push(options.and_then(|options| options.base.base.telemetry));
		Ok(completed_stream(model))
	}

	fn stream_simple(
		&self,
		model: &Model,
		_context: &TranscriptContext,
		options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		self.observed
			.borrow_mut()
			.push(options.and_then(|options| options.base.base.telemetry));
		Ok(completed_stream(model))
	}

	fn fetch_deferred(&self) -> Option<FetchDeferredFunction> {
		let observed = self.observed.clone();
		Some(Rc::new(
			move |model: &Model, _: &DeferredHandle, options: Option<DeferredFetchOptions>| {
				observed
					.borrow_mut()
					.push(options.and_then(|options| options.base.telemetry));
				Ok(completed_stream(model))
			},
		))
	}

	fn cancel_deferred(&self) -> Option<CancelDeferredFunction> {
		let observed = self.observed.clone();
		Some(Rc::new(
			move |_: &Model, _: &DeferredHandle, options: Option<DeferredCancelOptions>| {
				observed
					.borrow_mut()
					.push(options.and_then(|options| options.telemetry));
				Box::pin(async { Ok(()) })
			},
		))
	}
}

/// The image API that records the telemetry option of every call.
struct ObservingImages {
	observed: Observed,
}

impl ProviderImages for ObservingImages {
	fn generate_images(
		&self,
		model: &ImageModel,
		_context: &ImagesContext,
		options: Option<ProviderImagesOptions>,
	) -> LocalBoxFuture<'static, PiResult<AssistantImages>> {
		self.observed
			.borrow_mut()
			.push(options.and_then(|options| options.base.base.telemetry));
		let result = empty_images(model);
		Box::pin(async move { Ok(result) })
	}
}

fn empty_images(model: &ImageModel) -> AssistantImages {
	AssistantImages {
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		output: Vec::new(),
		response_id: None,
		usage: None,
		stop_reason: ImagesStopReason::Stop,
		error_message: None,
		timestamp: 0.0,
	}
}

mod provider_request_options_telemetry {
	use super::*;

	#[test]
	fn is_inherited_by_every_request_option_surface_and_simple_stream_conversion() {
		run(|_| async {
			let options = request_options();
			assert!(is_telemetry(options.telemetry.as_ref()));
			assert!(is_telemetry(
				build_base_options(&model(), &context(), Some(&simple_options()), None)
					.base
					.telemetry
					.as_ref()
			));
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn survives_provider_and_models_stream_deferred_dispatch() {
		run(|_| async {
			let observed: Observed = Rc::default();
			let handle = DeferredHandle {
				provider: model().provider.to_string(),
				model_id: model().id.clone(),
				api: model().api.to_string(),
				id: "response".into(),
				..DeferredHandle::default()
			};
			let api: Rc<dyn ProviderStreams> = Rc::new(ObservingStreams {
				observed: observed.clone(),
			});
			let provider = Rc::new(
				create_provider(CreateProviderOptions {
					id: model().provider.to_string(),
					auth: ambient_auth(),
					models: vec![AnyModel::Chat(model())],
					api: Some(ProviderApi::Single(api)),
					..CreateProviderOptions::default()
				})
				.unwrap(),
			);

			(provider.stream)(&model(), &context(), Some(provider_stream_options()))
				.unwrap()
				.result()
				.await;
			(provider.stream_simple)(&model(), &context(), Some(simple_options()))
				.unwrap()
				.result()
				.await;
			provider.fetch_deferred.as_ref().unwrap()(&model(), &handle, Some(deferred_options()))
				.unwrap()
				.result()
				.await;
			provider.cancel_deferred.as_ref().unwrap()(&model(), &handle, Some(request_options()))
				.await
				.unwrap();

			let models = create_models(None);
			models.set_provider(provider);
			let chat = Context::default();
			models
				.stream(
					&model(),
					&chat,
					Some(ModelsApiStreamOptions::from(provider_stream_options())),
				)
				.result()
				.await;
			models
				.stream_simple(&model(), &chat, Some(ModelsSimpleStreamOptions::from(simple_options())))
				.result()
				.await;
			models
				.fetch_deferred(
					&model(),
					&handle,
					Some(ModelsDeferredFetchOptions::from(deferred_options())),
				)
				.await;
			models
				.cancel_deferred(
					&model(),
					&handle,
					Some(ModelsDeferredCancelOptions::from(request_options())),
				)
				.await
				.unwrap();

			let observed = observed.borrow();
			assert_eq!(observed.len(), 8);
			assert!(observed.iter().all(|value| is_telemetry(value.as_ref())));
		});
	}

	#[test]
	#[ignore = "needs w2-ai-auth: resolve_provider_auth"]
	fn survives_direct_and_models_image_dispatch() {
		run(|_| async {
			let observed: Observed = Rc::default();
			let recorded = observed.clone();
			register_images_api_provider(
				ImagesApiProvider {
					api: image_model().api.clone(),
					generate_images: Rc::new(move |request_model, _context, options| {
						recorded
							.borrow_mut()
							.push(options.and_then(|options| options.base.base.telemetry));
						let result = empty_images(request_model);
						Box::pin(async move { Ok(result) })
					}),
				},
				None,
			);
			generate_images(&image_model(), &images_context(), Some(images_options()))
				.await
				.unwrap();

			let models = create_models(None);
			let images: Rc<dyn ProviderImages> = Rc::new(ObservingImages {
				observed: observed.clone(),
			});
			models.set_provider(Rc::new(
				create_provider(CreateProviderOptions {
					id: image_model().provider.to_string(),
					auth: ambient_auth(),
					models: vec![AnyModel::Image(image_model())],
					images: Some(IndexMap::from([(image_model().api.to_string(), images)])),
					..CreateProviderOptions::default()
				})
				.unwrap(),
			));
			models
				.generate_images(
					&image_model(),
					&images_context(),
					Some(ModelsImagesOptions::from(images_options().base)),
				)
				.await;

			let observed = observed.borrow();
			assert_eq!(observed.len(), 2);
			assert!(observed.iter().all(|value| is_telemetry(value.as_ref())));
		});
	}
}
