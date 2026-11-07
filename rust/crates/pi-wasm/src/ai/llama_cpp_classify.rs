//! Exports of `api/llama-cpp-classify` and `api/llama-cpp-classify.lazy`.

use pi_ai::api::{llama_cpp_classify, llama_cpp_classify_lazy};
use pi_ai::{ClassifierContext, ClassifierQuestion};
use wasm_bindgen::prelude::*;

use super::models::NativeClassifier;
use crate::options::{arg, throw};

/// `llamaCppClassifyApi()`: the module as lazily loaded `ProviderClassifier`; the facade's `classify` calls it too.
#[wasm_bindgen(js_name = aiLlamaCppClassifyApi)]
pub fn api() -> NativeClassifier {
	NativeClassifier::new(llama_cpp_classify_lazy::llama_cpp_classify_api())
}

/// `llamaServerRoot(baseUrl)`.
#[wasm_bindgen(js_name = aiLlamaServerRoot)]
pub fn llama_server_root(base_url: &str) -> String {
	llama_cpp_classify::llama_server_root(base_url)
}

/// `renderQuestion(context, id)`.
#[wasm_bindgen(js_name = aiRenderQuestion)]
pub fn render_question(context: JsValue, id: &str) -> Result<JsValue, JsValue> {
	let context: ClassifierContext = arg(context)?;
	throw(llama_cpp_classify::render_question(&context, id).and_then(|question| crate::json::to_js(&question)))
}

/// `labelProbabilities(logprobs, temperature)`.
#[wasm_bindgen(js_name = aiLabelProbabilities)]
pub fn label_probabilities(logprobs: Vec<f64>, temperature: f64) -> Vec<f64> {
	llama_cpp_classify::label_probabilities(&logprobs, temperature)
}

/// `peakConfidence(probabilities)`.
#[wasm_bindgen(js_name = aiPeakConfidence)]
pub fn peak_confidence(probabilities: Vec<f64>) -> f64 {
	llama_cpp_classify::peak_confidence(&probabilities)
}

/// `answerFromProbabilities(question, keys, probabilities)`.
#[wasm_bindgen(js_name = aiAnswerFromProbabilities)]
pub fn answer_from_probabilities(
	question: JsValue,
	keys: Vec<String>,
	probabilities: Vec<f64>,
) -> Result<JsValue, JsValue> {
	let question: ClassifierQuestion = arg(question)?;
	throw(crate::json::to_js(&llama_cpp_classify::answer_from_probabilities(
		&question,
		&keys,
		&probabilities,
	)))
}
