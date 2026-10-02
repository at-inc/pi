import type { AssistantMessage } from "@at-inc/pi-ai";
import type { EntryRecord } from "@earendil-works/pi-durable";

export function lastCacheHitRate(entries: readonly EntryRecord[]): number | undefined {
	for (const entry of [...entries].reverse()) {
		const message = entry.model?.[0];
		if (entry.kind !== "pi.assistant" || message?.role !== "assistant") continue;
		const assistant = message as AssistantMessage;
		if (assistant.stopReason === "aborted" || assistant.stopReason === "error") continue;
		const promptTokens = assistant.usage.input + assistant.usage.cacheRead + assistant.usage.cacheWrite;
		return promptTokens > 0 ? (assistant.usage.cacheRead / promptTokens) * 100 : undefined;
	}
	return undefined;
}
