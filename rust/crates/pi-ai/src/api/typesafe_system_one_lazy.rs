//! Port of `packages/ai/src/api/typesafe-system-one.lazy.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-bedrock-misc
// Until the API module is ported, the wrapper never panics: every operation fails with "API not ported".
use std::rc::Rc;

use pi_host::{LocalBoxFuture, PiError, PiResult};

use crate::types::{ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult, ProviderClassifier};

struct NotPorted;

impl ProviderClassifier for NotPorted {
	fn classify(
		&self,
		_model: &ClassifierModel,
		_context: &ClassifierContext,
		_options: Option<ClassifierOptions>,
	) -> LocalBoxFuture<'static, PiResult<ClassifierResult>> {
		Box::pin(async { Err(PiError::new("API not ported")) })
	}
}

/// `typesafeSystemOneApi()`: every classification rejects with `API not ported`.
pub fn typesafe_system_one_api() -> Rc<dyn ProviderClassifier> {
	Rc::new(NotPorted)
}
