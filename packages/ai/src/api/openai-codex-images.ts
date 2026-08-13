import type { Tool as OpenAITool } from "openai/resources/responses/responses.js";
import type {
	AssistantImages,
	AssistantMessage,
	Context,
	ImageContent,
	ImagesContext,
	ImagesFunction,
	ImagesModel,
	ImagesOptions,
	Model,
} from "../types.ts";
import { combineAbortSignals } from "../utils/abort-signals.ts";
import { stream } from "./openai-codex-responses.ts";

export const OPENAI_CODEX_IMAGE_DRIVER_MODEL_ID = "gpt-5.4-mini";
const DEFAULT_IMAGE_TIMEOUT_MS = 150_000;
const MAX_IMAGE_BASE64_CHARS = 24 * 1024 * 1024;

export const generateImages: ImagesFunction<"openai-codex-images", ImagesOptions> = async (
	model: ImagesModel<"openai-codex-images">,
	context: ImagesContext,
	options?: ImagesOptions,
) => {
	const output: AssistantImages = {
		api: model.api,
		provider: model.provider,
		model: model.id,
		output: [],
		stopReason: "stop",
		timestamp: Date.now(),
	};

	const images = new Map<number, ImageContent>();
	const driver: Model<"openai-codex-responses"> = {
		id: OPENAI_CODEX_IMAGE_DRIVER_MODEL_ID,
		name: "GPT-5.4 mini",
		api: "openai-codex-responses",
		provider: model.provider,
		baseUrl: model.baseUrl,
		reasoning: true,
		input: ["text", "image"],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 272_000,
		maxTokens: 128_000,
	};
	const responsesContext: Context = {
		systemPrompt: "Use the image generation tool exactly once to fulfill the user's request.",
		messages: [{ role: "user", content: context.input, timestamp: Date.now() }],
	};
	const action = context.input.some((item) => item.type === "image") ? "edit" : "generate";
	const timeoutMs = options?.timeoutMs ?? DEFAULT_IMAGE_TIMEOUT_MS;
	const timeoutSignal = timeoutMs > 0 ? AbortSignal.timeout(timeoutMs) : undefined;
	const requestSignal = combineAbortSignals([options?.signal, timeoutSignal]);

	let result: AssistantMessage;
	try {
		result = await stream(driver, responsesContext, {
			apiKey: options?.apiKey,
			fetch: options?.fetch,
			env: options?.env,
			headers: options?.headers,
			signal: requestSignal.signal,
			timeoutMs,
			maxRetryDelayMs: options?.maxRetryDelayMs,
			telemetryContext: options?.telemetryContext,
			transport: "sse",
			// Image requests are isolated and billable, so never persist state or retry an ambiguous failure.
			cacheRetention: "none",
			maxRetries: 0,
			onResponse: async (response) => await options?.onResponse?.(response, model),
			onPayload: async (payload) => {
				const inspected = (await options?.onPayload?.(payload, model)) ?? payload;
				if (typeof inspected !== "object" || inspected === null || Array.isArray(inspected)) {
					throw new Error("Codex image generation payload hook must return an object");
				}
				const body = { ...(inspected as Record<string, unknown>) };
				delete body.previous_response_id;
				delete body.prompt_cache_key;
				return {
					...body,
					model: OPENAI_CODEX_IMAGE_DRIVER_MODEL_ID,
					store: false,
					stream: true,
					tools: [{ type: "image_generation", action }] satisfies OpenAITool[],
					tool_choice: "required",
					parallel_tool_calls: false,
				};
			},
			onImageGenerationCall: (outputIndex, item) => {
				if (item.status !== "completed") return;
				if (typeof item.result !== "string" || !isValidBase64(item.result)) {
					throw new Error("ChatGPT returned malformed image data");
				}
				images.set(outputIndex, { type: "image", data: item.result, mimeType: "image/png" });
				// Senpi found that native results can be repeated across item-done and terminal events; cap the
				// deduplicated aggregate so a provider response cannot retain unbounded base64 payloads.
				if ([...images.values()].reduce((total, image) => total + image.data.length, 0) > MAX_IMAGE_BASE64_CHARS) {
					images.clear();
					throw new Error("ChatGPT image generation output exceeds the 24 MiB base64 limit");
				}
			},
		}).result();
	} finally {
		requestSignal.cleanup();
	}

	output.responseId = result.responseId;
	output.usage = result.usage;
	if (result.stopReason === "error" || result.stopReason === "aborted") {
		const timedOut = timeoutSignal?.aborted && !options?.signal?.aborted;
		output.stopReason = timedOut ? "error" : result.stopReason;
		output.errorMessage = timedOut ? `ChatGPT image generation timed out after ${timeoutMs}ms` : result.errorMessage;
		return output;
	}
	if (images.size === 0) {
		output.stopReason = "error";
		output.errorMessage = "ChatGPT image generation completed without an image";
		return output;
	}

	output.output = [...images.values()];
	return output;
};

function isValidBase64(value: string): boolean {
	return value.length > 0 && value.length % 4 === 0 && /^[A-Za-z0-9+/]*={0,2}$/.test(value);
}
