//! Port of `packages/ai/src/models.generated.ts`.
//! PORT-STATUS: ported
//!
//! The aggregation over the per-provider catalogs that `build.rs` embeds. Each map lists providers in the
//! generated file's key order and borrows the provider's static catalog.

use std::sync::LazyLock;

use indexmap::IndexMap;

use crate::model_catalog::{ChatModelCatalog, ClassifierModelCatalog, ImageModelCatalog};
use crate::providers::amazon_bedrock_models::*;
use crate::providers::ant_ling_models::*;
use crate::providers::anthropic_models::*;
use crate::providers::azure_models::*;
use crate::providers::baseten_models::*;
use crate::providers::cerebras_models::*;
use crate::providers::cloudflare_ai_gateway_models::*;
use crate::providers::cloudflare_workers_ai_models::*;
use crate::providers::deepseek_models::*;
use crate::providers::fireworks_models::*;
use crate::providers::github_copilot_models::*;
use crate::providers::google_models::*;
use crate::providers::google_vertex_models::*;
use crate::providers::groq_models::*;
use crate::providers::huggingface_models::*;
use crate::providers::kimi_coding_models::*;
use crate::providers::meta_models::*;
use crate::providers::minimax_cn_models::*;
use crate::providers::minimax_models::*;
use crate::providers::mistral_models::*;
use crate::providers::moonshotai_cn_models::*;
use crate::providers::moonshotai_models::*;
use crate::providers::nvidia_models::*;
use crate::providers::openai_codex_models::*;
use crate::providers::openai_models::*;
use crate::providers::opencode_go_models::*;
use crate::providers::opencode_models::*;
use crate::providers::openrouter_models::*;
use crate::providers::qwen_token_plan_cn_models::*;
use crate::providers::qwen_token_plan_individual_models::*;
use crate::providers::qwen_token_plan_models::*;
use crate::providers::radius_models::*;
use crate::providers::together_models::*;
use crate::providers::typesafe_models::*;
use crate::providers::vercel_ai_gateway_models::*;
use crate::providers::xai_models::*;
use crate::providers::xiaomi_models::*;
use crate::providers::xiaomi_token_plan_ams_models::*;
use crate::providers::xiaomi_token_plan_cn_models::*;
use crate::providers::xiaomi_token_plan_sgp_models::*;
use crate::providers::zai_coding_cn_models::*;
use crate::providers::zai_models::*;

pub static MODELS: LazyLock<IndexMap<&'static str, &'static ChatModelCatalog>> = LazyLock::new(|| {
	IndexMap::from([
		("amazon-bedrock", &*AMAZON_BEDROCK_IMAGE_MODELS),
		("ant-ling", &*ANT_LING_IMAGE_MODELS),
		("anthropic", &*ANTHROPIC_IMAGE_MODELS),
		("azure", &*AZURE_IMAGE_MODELS),
		("baseten", &*BASETEN_IMAGE_MODELS),
		("cerebras", &*CEREBRAS_IMAGE_MODELS),
		("cloudflare-ai-gateway", &*CLOUDFLARE_AI_GATEWAY_IMAGE_MODELS),
		("cloudflare-workers-ai", &*CLOUDFLARE_WORKERS_AI_IMAGE_MODELS),
		("deepseek", &*DEEPSEEK_IMAGE_MODELS),
		("fireworks", &*FIREWORKS_IMAGE_MODELS),
		("github-copilot", &*GITHUB_COPILOT_IMAGE_MODELS),
		("google", &*GOOGLE_IMAGE_MODELS),
		("google-vertex", &*GOOGLE_VERTEX_IMAGE_MODELS),
		("groq", &*GROQ_IMAGE_MODELS),
		("huggingface", &*HUGGINGFACE_IMAGE_MODELS),
		("kimi-coding", &*KIMI_CODING_IMAGE_MODELS),
		("meta", &*META_IMAGE_MODELS),
		("minimax", &*MINIMAX_IMAGE_MODELS),
		("minimax-cn", &*MINIMAX_CN_IMAGE_MODELS),
		("mistral", &*MISTRAL_IMAGE_MODELS),
		("moonshotai", &*MOONSHOTAI_IMAGE_MODELS),
		("moonshotai-cn", &*MOONSHOTAI_CN_IMAGE_MODELS),
		("nvidia", &*NVIDIA_IMAGE_MODELS),
		("openai", &*OPENAI_IMAGE_MODELS),
		("openai-codex", &*OPENAI_CODEX_IMAGE_MODELS),
		("opencode", &*OPENCODE_IMAGE_MODELS),
		("opencode-go", &*OPENCODE_GO_IMAGE_MODELS),
		("openrouter", &*OPENROUTER_IMAGE_MODELS),
		("qwen-token-plan", &*QWEN_TOKEN_PLAN_IMAGE_MODELS),
		("qwen-token-plan-cn", &*QWEN_TOKEN_PLAN_CN_IMAGE_MODELS),
		("qwen-token-plan-individual", &*QWEN_TOKEN_PLAN_INDIVIDUAL_IMAGE_MODELS),
		("radius", &*RADIUS_IMAGE_MODELS),
		("together", &*TOGETHER_IMAGE_MODELS),
		("typesafe", &*TYPESAFE_IMAGE_MODELS),
		("vercel-ai-gateway", &*VERCEL_AI_GATEWAY_IMAGE_MODELS),
		("xai", &*XAI_IMAGE_MODELS),
		("xiaomi", &*XIAOMI_IMAGE_MODELS),
		("xiaomi-token-plan-ams", &*XIAOMI_TOKEN_PLAN_AMS_IMAGE_MODELS),
		("xiaomi-token-plan-cn", &*XIAOMI_TOKEN_PLAN_CN_IMAGE_MODELS),
		("xiaomi-token-plan-sgp", &*XIAOMI_TOKEN_PLAN_SGP_IMAGE_MODELS),
		("zai", &*ZAI_IMAGE_MODELS),
		("zai-coding-cn", &*ZAI_CODING_CN_IMAGE_MODELS),
	])
});

pub static IMAGE_MODELS: LazyLock<IndexMap<&'static str, &'static ImageModelCatalog>> = LazyLock::new(|| {
	IndexMap::from([
		("amazon-bedrock", &*AMAZON_BEDROCK_IMAGE_IMAGE_MODELS),
		("ant-ling", &*ANT_LING_IMAGE_IMAGE_MODELS),
		("anthropic", &*ANTHROPIC_IMAGE_IMAGE_MODELS),
		("azure", &*AZURE_IMAGE_IMAGE_MODELS),
		("baseten", &*BASETEN_IMAGE_IMAGE_MODELS),
		("cerebras", &*CEREBRAS_IMAGE_IMAGE_MODELS),
		("cloudflare-ai-gateway", &*CLOUDFLARE_AI_GATEWAY_IMAGE_IMAGE_MODELS),
		("cloudflare-workers-ai", &*CLOUDFLARE_WORKERS_AI_IMAGE_IMAGE_MODELS),
		("deepseek", &*DEEPSEEK_IMAGE_IMAGE_MODELS),
		("fireworks", &*FIREWORKS_IMAGE_IMAGE_MODELS),
		("github-copilot", &*GITHUB_COPILOT_IMAGE_IMAGE_MODELS),
		("google", &*GOOGLE_IMAGE_IMAGE_MODELS),
		("google-vertex", &*GOOGLE_VERTEX_IMAGE_IMAGE_MODELS),
		("groq", &*GROQ_IMAGE_IMAGE_MODELS),
		("huggingface", &*HUGGINGFACE_IMAGE_IMAGE_MODELS),
		("kimi-coding", &*KIMI_CODING_IMAGE_IMAGE_MODELS),
		("meta", &*META_IMAGE_IMAGE_MODELS),
		("minimax", &*MINIMAX_IMAGE_IMAGE_MODELS),
		("minimax-cn", &*MINIMAX_CN_IMAGE_IMAGE_MODELS),
		("mistral", &*MISTRAL_IMAGE_IMAGE_MODELS),
		("moonshotai", &*MOONSHOTAI_IMAGE_IMAGE_MODELS),
		("moonshotai-cn", &*MOONSHOTAI_CN_IMAGE_IMAGE_MODELS),
		("nvidia", &*NVIDIA_IMAGE_IMAGE_MODELS),
		("openai", &*OPENAI_IMAGE_IMAGE_MODELS),
		("openai-codex", &*OPENAI_CODEX_IMAGE_IMAGE_MODELS),
		("opencode", &*OPENCODE_IMAGE_IMAGE_MODELS),
		("opencode-go", &*OPENCODE_GO_IMAGE_IMAGE_MODELS),
		("openrouter", &*OPENROUTER_IMAGE_IMAGE_MODELS),
		("qwen-token-plan", &*QWEN_TOKEN_PLAN_IMAGE_IMAGE_MODELS),
		("qwen-token-plan-cn", &*QWEN_TOKEN_PLAN_CN_IMAGE_IMAGE_MODELS),
		(
			"qwen-token-plan-individual",
			&*QWEN_TOKEN_PLAN_INDIVIDUAL_IMAGE_IMAGE_MODELS,
		),
		("radius", &*RADIUS_IMAGE_IMAGE_MODELS),
		("together", &*TOGETHER_IMAGE_IMAGE_MODELS),
		("typesafe", &*TYPESAFE_IMAGE_IMAGE_MODELS),
		("vercel-ai-gateway", &*VERCEL_AI_GATEWAY_IMAGE_IMAGE_MODELS),
		("xai", &*XAI_IMAGE_IMAGE_MODELS),
		("xiaomi", &*XIAOMI_IMAGE_IMAGE_MODELS),
		("xiaomi-token-plan-ams", &*XIAOMI_TOKEN_PLAN_AMS_IMAGE_IMAGE_MODELS),
		("xiaomi-token-plan-cn", &*XIAOMI_TOKEN_PLAN_CN_IMAGE_IMAGE_MODELS),
		("xiaomi-token-plan-sgp", &*XIAOMI_TOKEN_PLAN_SGP_IMAGE_IMAGE_MODELS),
		("zai", &*ZAI_IMAGE_IMAGE_MODELS),
		("zai-coding-cn", &*ZAI_CODING_CN_IMAGE_IMAGE_MODELS),
	])
});

pub static CLASSIFIER_MODELS: LazyLock<IndexMap<&'static str, &'static ClassifierModelCatalog>> = LazyLock::new(|| {
	IndexMap::from([
		("amazon-bedrock", &*AMAZON_BEDROCK_IMAGE_CLASSIFIER_MODELS),
		("ant-ling", &*ANT_LING_IMAGE_CLASSIFIER_MODELS),
		("anthropic", &*ANTHROPIC_IMAGE_CLASSIFIER_MODELS),
		("azure", &*AZURE_IMAGE_CLASSIFIER_MODELS),
		("baseten", &*BASETEN_IMAGE_CLASSIFIER_MODELS),
		("cerebras", &*CEREBRAS_IMAGE_CLASSIFIER_MODELS),
		("cloudflare-ai-gateway", &*CLOUDFLARE_AI_GATEWAY_IMAGE_CLASSIFIER_MODELS),
		("cloudflare-workers-ai", &*CLOUDFLARE_WORKERS_AI_IMAGE_CLASSIFIER_MODELS),
		("deepseek", &*DEEPSEEK_IMAGE_CLASSIFIER_MODELS),
		("fireworks", &*FIREWORKS_IMAGE_CLASSIFIER_MODELS),
		("github-copilot", &*GITHUB_COPILOT_IMAGE_CLASSIFIER_MODELS),
		("google", &*GOOGLE_IMAGE_CLASSIFIER_MODELS),
		("google-vertex", &*GOOGLE_VERTEX_IMAGE_CLASSIFIER_MODELS),
		("groq", &*GROQ_IMAGE_CLASSIFIER_MODELS),
		("huggingface", &*HUGGINGFACE_IMAGE_CLASSIFIER_MODELS),
		("kimi-coding", &*KIMI_CODING_IMAGE_CLASSIFIER_MODELS),
		("meta", &*META_IMAGE_CLASSIFIER_MODELS),
		("minimax", &*MINIMAX_IMAGE_CLASSIFIER_MODELS),
		("minimax-cn", &*MINIMAX_CN_IMAGE_CLASSIFIER_MODELS),
		("mistral", &*MISTRAL_IMAGE_CLASSIFIER_MODELS),
		("moonshotai", &*MOONSHOTAI_IMAGE_CLASSIFIER_MODELS),
		("moonshotai-cn", &*MOONSHOTAI_CN_IMAGE_CLASSIFIER_MODELS),
		("nvidia", &*NVIDIA_IMAGE_CLASSIFIER_MODELS),
		("openai", &*OPENAI_IMAGE_CLASSIFIER_MODELS),
		("openai-codex", &*OPENAI_CODEX_IMAGE_CLASSIFIER_MODELS),
		("opencode", &*OPENCODE_IMAGE_CLASSIFIER_MODELS),
		("opencode-go", &*OPENCODE_GO_IMAGE_CLASSIFIER_MODELS),
		("openrouter", &*OPENROUTER_IMAGE_CLASSIFIER_MODELS),
		("qwen-token-plan", &*QWEN_TOKEN_PLAN_IMAGE_CLASSIFIER_MODELS),
		("qwen-token-plan-cn", &*QWEN_TOKEN_PLAN_CN_IMAGE_CLASSIFIER_MODELS),
		(
			"qwen-token-plan-individual",
			&*QWEN_TOKEN_PLAN_INDIVIDUAL_IMAGE_CLASSIFIER_MODELS,
		),
		("radius", &*RADIUS_IMAGE_CLASSIFIER_MODELS),
		("together", &*TOGETHER_IMAGE_CLASSIFIER_MODELS),
		("typesafe", &*TYPESAFE_IMAGE_CLASSIFIER_MODELS),
		("vercel-ai-gateway", &*VERCEL_AI_GATEWAY_IMAGE_CLASSIFIER_MODELS),
		("xai", &*XAI_IMAGE_CLASSIFIER_MODELS),
		("xiaomi", &*XIAOMI_IMAGE_CLASSIFIER_MODELS),
		("xiaomi-token-plan-ams", &*XIAOMI_TOKEN_PLAN_AMS_IMAGE_CLASSIFIER_MODELS),
		("xiaomi-token-plan-cn", &*XIAOMI_TOKEN_PLAN_CN_IMAGE_CLASSIFIER_MODELS),
		("xiaomi-token-plan-sgp", &*XIAOMI_TOKEN_PLAN_SGP_IMAGE_CLASSIFIER_MODELS),
		("zai", &*ZAI_IMAGE_CLASSIFIER_MODELS),
		("zai-coding-cn", &*ZAI_CODING_CN_IMAGE_CLASSIFIER_MODELS),
	])
});
