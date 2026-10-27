import OpenAI from "openai";
import type { ImageGenerateParams, ImagesResponse } from "openai/resources/images.js";
import type { AssistantImages, ImageContent, ImagesFunction } from "../types.ts";
import { combineAbortSignals } from "../utils/abort-signals.ts";
import { formatProviderError, normalizeProviderError } from "../utils/error-body.ts";
import { headersToRecord, providerHeadersToRecord } from "../utils/headers.ts";
import { getIdentityHeaders } from "../utils/identity.ts";
import { uuidv7 } from "../utils/uuid.ts";

const DEFAULT_IMAGE_TIMEOUT_MS = 150_000;
const MAX_IMAGE_BASE64_CHARS = 24 * 1024 * 1024;

type CodexImageRequest = Pick<ImageGenerateParams, "model" | "prompt" | "n"> & {
	images?: { image_url: string }[];
};

/** Generate or edit images through the native Codex Images API with ChatGPT OAuth. */
export const generateImages: ImagesFunction = async (model, context, options) => {
	const output: AssistantImages = {
		api: model.api,
		provider: model.provider,
		model: model.id,
		output: [],
		stopReason: "stop",
		timestamp: Date.now(),
	};
	const timeoutMs = options?.timeoutMs ?? DEFAULT_IMAGE_TIMEOUT_MS;
	const timeoutSignal = timeoutMs > 0 ? AbortSignal.timeout(timeoutMs) : undefined;
	const requestSignal = combineAbortSignals([options?.signal, timeoutSignal]);
	try {
		const apiKey = options?.apiKey;
		if (!apiKey) throw new Error(`No API key for provider: ${model.provider}`);
		let accountId: string | undefined;
		try {
			const payload = JSON.parse(atob(apiKey.split(".")[1])) as {
				"https://api.openai.com/auth"?: { chatgpt_account_id?: string };
			};
			accountId = payload?.["https://api.openai.com/auth"]?.chatgpt_account_id;
		} catch {}
		if (!accountId) throw new Error("Failed to extract accountId from token");

		const baseUrl = (model.baseUrl?.trim() || "https://chatgpt.com/backend-api")
			.replace(/\/+$/, "")
			.replace(/\/codex\/responses$/, "/codex");
		const client = new OpenAI({
			apiKey,
			baseURL: baseUrl.endsWith("/codex") ? baseUrl : `${baseUrl}/codex`,
			dangerouslyAllowBrowser: true,
			fetch: options?.fetch,
			defaultHeaders: providerHeadersToRecord(
				{
					originator: "pi",
					"chatgpt-account-id": accountId,
					"x-codex-image-turn-id": uuidv7(),
				},
				getIdentityHeaders(model, options?.identity),
				model.headers,
				options?.headers,
			),
		});
		const images = context.input
			.filter((part) => part.type === "image")
			.map((image) => ({ image_url: `data:${image.mimeType};base64,${image.data}` }));
		// Codex rejects the hosted image_generation tool choice and can return only text without it.
		// Use the native contract in openai/codex's codex-rs/codex-api/src/endpoint/images.rs instead.
		let body: CodexImageRequest = {
			model: "gpt-image-2",
			prompt: context.input
				.filter((part) => part.type === "text")
				.map((part) => part.text)
				.join("\n"),
			n: 1,
			...(images.length ? { images } : {}),
		};
		const nextBody = await options?.onPayload?.(body, model);
		if (nextBody !== undefined) {
			if (typeof nextBody !== "object" || nextBody === null || Array.isArray(nextBody)) {
				throw new Error("Codex image generation payload hook must return an object");
			}
			body = nextBody as CodexImageRequest;
		}
		const { data, response } = await client
			.post<ImagesResponse>(images.length ? "/images/edits" : "/images/generations", {
				body,
				signal: requestSignal.signal,
				timeout: timeoutMs > 0 ? timeoutMs : undefined,
				// Generations are billable: never retry an ambiguous failure.
				maxRetries: 0,
			})
			.withResponse();
		await options?.onResponse?.({ status: response.status, headers: headersToRecord(response.headers) }, model);
		output.responseId = response.headers.get("x-codex-imagegen-request-id") ?? undefined;
		if (!Array.isArray(data?.data) || data.data.length === 0) {
			throw new Error("ChatGPT image generation completed without an image");
		}
		let totalChars = 0;
		output.output = data.data.map((image): ImageContent => {
			const value = image?.b64_json;
			if (
				typeof value !== "string" ||
				!value.length ||
				value.length % 4 !== 0 ||
				!/^[A-Za-z0-9+/]*={0,2}$/.test(value)
			) {
				throw new Error("ChatGPT returned malformed image data");
			}
			totalChars += value.length;
			if (totalChars > MAX_IMAGE_BASE64_CHARS) {
				throw new Error("ChatGPT image generation output exceeds the 24 MiB base64 limit");
			}
			return { type: "image", data: value, mimeType: "image/png" };
		});
	} catch (error) {
		if (error instanceof OpenAI.APIError && error.headers && error.status !== undefined) {
			await options?.onResponse?.({ status: error.status, headers: headersToRecord(error.headers) }, model);
			output.responseId = error.headers.get("x-codex-imagegen-request-id") ?? undefined;
		}
		const timedOut = timeoutSignal?.aborted && !options?.signal?.aborted;
		output.stopReason = options?.signal?.aborted ? "aborted" : "error";
		output.errorMessage = timedOut
			? `ChatGPT image generation timed out after ${timeoutMs}ms`
			: formatProviderError(normalizeProviderError(error));
	} finally {
		requestSignal.cleanup();
	}
	return output;
};
