// providers/images/register-builtins: the built-in image-generation APIs of the images api registry, in the Rust core.

import type * as ts from "@at-inc/pi-ai/compat";
import { wasm } from "@at-inc/pi-core";
import { rethrow } from "../../internal/errors.ts";

export const generateImagesOpenAICodex: typeof ts.generateImagesOpenAICodex = (model, context, options) =>
	wasm.aiGenerateImagesBuiltin("openai-codex-images", model, context, options).catch(rethrow);

export const generateImagesOpenRouter: typeof ts.generateImagesOpenRouter = (model, context, options) =>
	wasm.aiGenerateImagesBuiltin("openrouter-images", model, context, options).catch(rethrow);

export const registerBuiltInImagesApiProviders: typeof ts.registerBuiltInImagesApiProviders = () =>
	wasm.aiRegisterBuiltInImagesApiProviders();
