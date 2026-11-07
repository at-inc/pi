//! Port of `packages/ai/test/cloudflare-stream.test.ts`.

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::providers::cloudflare_stream::cloudflare_streams;
use pi_ai::types::{
	BaseModel, Context, Model, ModelCost, ModelInputModality, ProviderEnv, ProviderRequestOptions,
	ProviderStreamOptions, ProviderStreams, SimpleStreamOptions, StreamOptions, TranscriptContext,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::transcript::normalize_context;
use pi_host::PiResult;

fn model() -> Model {
	Model {
		base: BaseModel {
			id: "model".into(),
			name: "model".into(),
			api: "openai-completions".into(),
			provider: "cloudflare-ai-gateway".into(),
			base_url: "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/openai"
				.into(),
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

fn context() -> TranscriptContext {
	normalize_context(&Context::default())
}

struct CapturingStreams {
	captured: Rc<RefCell<Vec<String>>>,
}

impl ProviderStreams for CapturingStreams {
	fn stream(
		&self,
		model: &Model,
		_context: &TranscriptContext,
		_options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		self.captured.borrow_mut().push(model.base_url.clone());
		Ok(AssistantMessageEventStream::new())
	}

	fn stream_simple(
		&self,
		model: &Model,
		_context: &TranscriptContext,
		_options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		self.captured.borrow_mut().push(model.base_url.clone());
		Ok(AssistantMessageEventStream::new())
	}
}

fn with_env(env: Option<ProviderEnv>) -> StreamOptions {
	StreamOptions {
		base: ProviderRequestOptions {
			env,
			..ProviderRequestOptions::default()
		},
		..StreamOptions::default()
	}
}

mod cloudflare_provider_streams {
	use super::*;

	#[test]
	fn materializes_the_model_endpoint_before_dispatch() {
		pi_host::testing::run(|_| async {
			let captured = Rc::new(RefCell::new(Vec::new()));
			let streams = cloudflare_streams(Rc::new(CapturingStreams {
				captured: captured.clone(),
			}));
			let env = ProviderEnv::from([
				("CLOUDFLARE_ACCOUNT_ID".to_string(), "account".to_string()),
				("CLOUDFLARE_GATEWAY_ID".to_string(), "gateway".to_string()),
			]);

			streams
				.stream(
					&model(),
					&context(),
					Some(ProviderStreamOptions {
						base: with_env(Some(env.clone())),
						..ProviderStreamOptions::default()
					}),
				)
				.unwrap();
			streams
				.stream_simple(
					&model(),
					&context(),
					Some(SimpleStreamOptions {
						base: with_env(Some(env)),
						..SimpleStreamOptions::default()
					}),
				)
				.unwrap();

			assert_eq!(
				*captured.borrow(),
				vec![
					"https://gateway.ai.cloudflare.com/v1/account/gateway/openai",
					"https://gateway.ai.cloudflare.com/v1/account/gateway/openai",
				]
			);
		});
	}

	#[test]
	fn keeps_placeholders_when_the_provider_env_does_not_resolve_them() {
		pi_host::testing::run(|_| async {
			let captured = Rc::new(RefCell::new(Vec::new()));
			let streams = cloudflare_streams(Rc::new(CapturingStreams {
				captured: captured.clone(),
			}));

			streams
				.stream_simple(&model(), &context(), Some(SimpleStreamOptions::default()))
				.unwrap();

			assert_eq!(*captured.borrow(), vec![model().base_url.clone()]);
		});
	}
}
