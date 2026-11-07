//! Port of `packages/ai/src/api/cloudflare-workers-ai-system-one.lazy.ts`.
//! PORT-STATUS: reviewed

use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiResult};

use super::cloudflare_workers_ai_system_one;
use crate::types::{ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult, ProviderClassifier};

struct LazyClassifier;

impl ProviderClassifier for LazyClassifier {
	fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<ClassifierOptions>,
	) -> LocalBoxFuture<'static, PiResult<ClassifierResult>> {
		cloudflare_workers_ai_system_one::classify(model, context, options)
	}
}

/// `cloudflare_workers_ai_system_one_api()`: classifies through the `cloudflare-workers-ai-system-one` module.
pub fn cloudflare_workers_ai_system_one_api() -> Rc<dyn ProviderClassifier> {
	Rc::new(LazyClassifier)
}
