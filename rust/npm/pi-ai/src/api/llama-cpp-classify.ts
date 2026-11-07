// api/llama-cpp-classify: `classify` and the module's helpers over the Rust API module.

import type * as ts from "@at-inc/pi-ai/api/llama-cpp-classify";
import { wasm } from "@at-inc/pi-core";
import { nativeClassifier } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/llama-cpp-classify";

const api = nativeClassifier(wasm.aiLlamaCppClassifyApi());

export const classify: typeof ts.classify = (model, context, options) => api.classify(model, context, options);

export const llamaServerRoot: typeof ts.llamaServerRoot = (baseUrl) => wasm.aiLlamaServerRoot(baseUrl);

export const renderQuestion: typeof ts.renderQuestion = (context, id) => wasm.aiRenderQuestion(context, id);

export const labelProbabilities: typeof ts.labelProbabilities = (logprobs, temperature) =>
	Array.from(wasm.aiLabelProbabilities(Float64Array.from(logprobs), temperature));

export const peakConfidence: typeof ts.peakConfidence = (probabilities) =>
	wasm.aiPeakConfidence(Float64Array.from(probabilities));

export const answerFromProbabilities: typeof ts.answerFromProbabilities = (question, keys, probabilities) =>
	wasm.aiAnswerFromProbabilities(question, [...keys], Float64Array.from(probabilities));
