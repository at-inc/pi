//! Port of `packages/ai/src/api/openai-decisions.lazy.ts`.
//! PORT-STATUS: ported
//!
//! Rust has no dynamic import: `classify` calls the statically linked module.

use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiResult};

use crate::api::openai_decisions::classify;
use crate::types::{ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult, ProviderClassifier};

struct OpenAIDecisionsLazy;

impl ProviderClassifier for OpenAIDecisionsLazy {
	fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<ClassifierOptions>,
	) -> LocalBoxFuture<'static, PiResult<ClassifierResult>> {
		let (model, context) = (model.clone(), context.clone());
		Box::pin(async move { Ok(classify(&model, &context, options.as_ref()).await) })
	}
}

/// `openAIDecisionsApi()`.
pub fn openai_decisions_api() -> Rc<dyn ProviderClassifier> {
	Rc::new(OpenAIDecisionsLazy)
}
