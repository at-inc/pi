//! Port of `packages/ai/src/providers/cloudflare-stream.ts`.
//! PORT-STATUS: ported
//!
//! Differences from the TS:
//! - `resolveCloudflareModel<TModel extends { baseUrl: string }>` is generic over the model structs that deref to
//!   `BaseModel`. It returns a copy; the TS returns the same object when nothing changes.
//! - The wrappers are private structs implementing `ProviderStreams` / `ProviderClassifier`. Like the TS object
//!   literals, the streams wrapper drops the wrapped module's deferred-response methods.

use std::ops::DerefMut;
use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiResult};

use crate::types::{
	BaseModel, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult, Model, ProviderClassifier,
	ProviderEnv, ProviderStreamOptions, ProviderStreams, SimpleStreamOptions, TranscriptContext,
};
use crate::utils::event_stream::AssistantMessageEventStream;

const CLOUDFLARE_ACCOUNT_ID: &str = "CLOUDFLARE_ACCOUNT_ID";
const CLOUDFLARE_GATEWAY_ID: &str = "CLOUDFLARE_GATEWAY_ID";

pub fn resolve_cloudflare_model<A, M>(model: &M, env: Option<&ProviderEnv>) -> M
where
	M: Clone + DerefMut<Target = BaseModel<A>>,
{
	let Some(env) = env else {
		return model.clone();
	};
	let account_placeholder = format!("{{{CLOUDFLARE_ACCOUNT_ID}}}");
	let gateway_placeholder = format!("{{{CLOUDFLARE_GATEWAY_ID}}}");
	let base_url = replace_all(
		&model.base_url,
		&account_placeholder,
		env.get(CLOUDFLARE_ACCOUNT_ID).unwrap_or(&account_placeholder),
	);
	let base_url = replace_all(
		&base_url,
		&gateway_placeholder,
		env.get(CLOUDFLARE_GATEWAY_ID).unwrap_or(&gateway_placeholder),
	);
	let mut resolved = model.clone();
	resolved.base_url = base_url;
	resolved
}

/// `String.prototype.replaceAll` with a string pattern: the replacement expands `$$`, `$&`, `` $` `` and `$'`, and
/// any other `$` stays literal because a string pattern has no capture groups.
fn replace_all(haystack: &str, pattern: &str, replacement: &str) -> String {
	let mut out = String::with_capacity(haystack.len());
	let mut last = 0;
	for (start, matched) in haystack.match_indices(pattern) {
		out.push_str(&haystack[last..start]);
		last = start + matched.len();
		let mut rest = replacement;
		while let Some(dollar) = rest.find('$') {
			out.push_str(&rest[..dollar]);
			rest = &rest[dollar + 1..];
			let expanded = match rest.chars().next() {
				Some('$') => "$",
				Some('&') => matched,
				Some('`') => &haystack[..start],
				Some('\'') => &haystack[last..],
				_ => {
					out.push('$');
					continue;
				}
			};
			out.push_str(expanded);
			rest = &rest[1..];
		}
		out.push_str(rest);
	}
	out.push_str(&haystack[last..]);
	out
}

struct CloudflareStreams {
	streams: Rc<dyn ProviderStreams>,
}

impl ProviderStreams for CloudflareStreams {
	fn stream(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<ProviderStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		let model = resolve_cloudflare_model(model, options.as_ref().and_then(|options| options.env.as_ref()));
		self.streams.stream(&model, context, options)
	}

	fn stream_simple(
		&self,
		model: &Model,
		context: &TranscriptContext,
		options: Option<SimpleStreamOptions>,
	) -> PiResult<AssistantMessageEventStream> {
		let model = resolve_cloudflare_model(model, options.as_ref().and_then(|options| options.env.as_ref()));
		self.streams.stream_simple(&model, context, options)
	}
}

/// Wrap an API implementation so Cloudflare account/gateway endpoint
/// placeholders materialize from the resolved provider env before dispatch.
pub fn cloudflare_streams(streams: Rc<dyn ProviderStreams>) -> Rc<dyn ProviderStreams> {
	Rc::new(CloudflareStreams { streams })
}

struct CloudflareClassifier {
	classifier: Rc<dyn ProviderClassifier>,
}

impl ProviderClassifier for CloudflareClassifier {
	fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<ClassifierOptions>,
	) -> LocalBoxFuture<'static, PiResult<ClassifierResult>> {
		let model = resolve_cloudflare_model(model, options.as_ref().and_then(|options| options.env.as_ref()));
		self.classifier.classify(&model, context, options)
	}
}

/// Classifier counterpart of {@link cloudflareStreams}.
pub fn cloudflare_classifier(classifier: Rc<dyn ProviderClassifier>) -> Rc<dyn ProviderClassifier> {
	Rc::new(CloudflareClassifier { classifier })
}
