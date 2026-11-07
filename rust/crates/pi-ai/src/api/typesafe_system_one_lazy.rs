//! Port of `packages/ai/src/api/typesafe-system-one.lazy.ts`.
//! PORT-STATUS: ported

use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiResult};

use super::typesafe_system_one;
use crate::types::{ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult, ProviderClassifier};

struct LazyClassifier;

impl ProviderClassifier for LazyClassifier {
	fn classify(
		&self,
		model: &ClassifierModel,
		context: &ClassifierContext,
		options: Option<ClassifierOptions>,
	) -> LocalBoxFuture<'static, PiResult<ClassifierResult>> {
		typesafe_system_one::classify(model, context, options)
	}
}

/// `typesafe_system_one_api()`: classifies through the `typesafe-system-one` module.
pub fn typesafe_system_one_api() -> Rc<dyn ProviderClassifier> {
	Rc::new(LazyClassifier)
}
