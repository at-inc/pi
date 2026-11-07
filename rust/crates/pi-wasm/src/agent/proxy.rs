//! Exports of `proxy`: `streamProxy` over the Rust proxy stream, whose events reach JS through an `EventSink` as
//! the pi-ai streams' do. The request goes through the host's `fetch`, read at call time.

use pi_agent::ProxyStreamOptions;
use pi_ai::{Context, Model};
use wasm_bindgen::prelude::*;

use super::get;
use crate::events::EventSink;
use crate::options::{JsOptions, arg};

/// `streamProxy(model, context, options)`, sending the events to `sink`. The context is read as a `Context` and
/// normalized, which leaves a transcript unchanged.
#[wasm_bindgen(js_name = agentStreamProxy)]
pub fn stream_proxy(model: JsValue, context: JsValue, options: JsValue, sink: EventSink) -> Result<(), JsValue> {
	let model: Model = arg(model)?;
	let context: Context = arg(context)?;
	let signal = crate::abort::from_js(&get(&options, "signal"));
	let mut options = JsOptions::new(options);
	let options = ProxyStreamOptions {
		temperature: options.take("temperature"),
		sampling_params: options.take("samplingParams"),
		max_tokens: options.take("maxTokens"),
		reasoning: options.take("reasoning"),
		cache_retention: options.take("cacheRetention"),
		session_id: options.take("sessionId"),
		headers: options.take("headers"),
		metadata: options.take("metadata"),
		transport: options.take("transport"),
		thinking_budgets: options.take("thinkingBudgets"),
		max_retry_delay_ms: options.take("maxRetryDelayMs"),
		signal,
		// `${undefined}` in the TS template strings.
		auth_token: options.take("authToken").unwrap_or_else(|| "undefined".into()),
		proxy_url: options.take("proxyUrl").unwrap_or_else(|| "undefined".into()),
	};
	let context = pi_ai::normalize_context(&context);
	crate::events::send(pi_agent::stream_proxy(&model, &context, options), sink);
	Ok(())
}
