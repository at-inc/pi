//! Port of `packages/ai/src/api/llama-cpp-classify.lazy.ts`.
//! PORT-STATUS: reviewed

use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiResult};

use super::llama_cpp_classify;
use crate::types::{ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult, ProviderClassifier};

struct LazyClassifier;

impl ProviderClassifier for LazyClassifier {
	fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<ClassifierOptions>,
	) -> LocalBoxFuture<'static, PiResult<ClassifierResult>> {
		llama_cpp_classify::classify(model, context, options)
	}
}

/// `llamaCppClassifyApi()`: classifies through the `llama-cpp-classify` module.
pub fn llama_cpp_classify_api() -> Rc<dyn ProviderClassifier> {
	Rc::new(LazyClassifier)
}
