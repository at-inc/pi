//! Rust-only replacement for the parts of `@aws-sdk/client-bedrock-runtime` 3.1126.0 that
//! `api/bedrock-converse-stream.ts` relies on. See "Provider SDKs" in `rust/PORTING.md`.
//! PORT-STATUS: reviewed
//!
//! It is one client with the `ConverseStream` operation (`BedrockRuntimeClient.send(new ConverseStreamCommand(input))`):
//! request serialization from the `com.amazonaws.bedrockruntime` schema, endpoint and region resolution, SigV4 or
//! bearer auth, the user agent headers, the standard retry strategy with clock skew correction, error deserialization
//! and the binary event stream. Differences from the SDK:
//! - The command input and the stream items are JSON. A blob (`image.source.bytes`,
//!   `reasoningContent.redactedContent`, ...) is base64 text where the SDK holds a `Uint8Array`; the serializer
//!   writes the text as is and the deserializer returns canonical base64.
//! - User agent: `os/<platform>` has no release, and `lang/js md/nodejs#<version>` is `lang/rust md/rust#<rust-version>`.
//!   The other sections, the feature letters and `x-amz-user-agent` match the SDK.
//! - Transport: requests go through the host's fetch instead of the default `NodeHttp2Handler`, so there is no
//!   `host` header (HTTP/2 sends `:authority`), the abort error is `AbortError: Request aborted` before and while the
//!   request is in flight, and an abort while the body streams ends the event stream without an error, as the HTTP/2
//!   handler does. No request timeout or socket options are set, like the SDK defaults.
//! - Region: `config.region`, `AWS_REGION` or the profile's `region`; the instance metadata (IMDS) fallback is not
//!   ported, so the error is `Region is missing` where the SDK would ask IMDS first.
//! - Credentials: `config.credentials`, then `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/`AWS_SESSION_TOKEN` (unless a
//!   profile is set), then static keys of the shared config and credentials files. Assume role, web identity,
//!   credential process, SSO and login profiles fail with a `CredentialsProviderError` that names the unsupported kind;
//!   the ECS container and IMDS providers are not ported (`Could not load credentials from any providers`).
//! - Retries: the standard strategy (3 attempts, `AWS_MAX_ATTEMPTS`/`max_attempts`, throttling and transient
//!   classification, `retry-after`/`x-amz-retry-after` hints, `SMITHY_NEW_RETRIES_2026`). The retry quota (500
//!   capacity tokens per client) is not tracked: pi-ai builds a client per request, which cannot use it up.
//!   `AWS_RETRY_MODE`/`retry_mode` are not read: `adaptive` (the client-side rate limiter and the `F` user agent
//!   feature) is not ported, so every client runs the standard mode. Node's socket error codes are read from `code`
//!   down the `cause` chain, but the host's fetch reports a failed request as `TypeError: fetch failed` with its cause
//!   as text, so every request that got no response is transient; the SDK leaves non-socket failures such as TLS
//!   errors to its other checks. An invalid `AWS_MAX_ATTEMPTS`, `AWS_USE_FIPS_ENDPOINT` or
//!   `AWS_USE_DUALSTACK_ENDPOINT` (or profile key) falls through to the next source, as `loadConfig`'s chain does.
//! - Clock skew: `getUpdatedSystemClockOffset` and the SigV4 signer's handlers of `@aws-sdk/core` 3.977.9, with the
//!   `Age` and 15-minute guards and the 240-second `clockSkewCorrected` threshold, for SigV4 only (the bearer signer
//!   has no handlers). An error's `ServerTime` property is not read, since no Bedrock error has one.
//! - Headers: `recursionDetectionMiddleware` sets `X-Amzn-Trace-Id` from `_X_AMZN_TRACE_ID` inside Lambda; the
//!   InvokeStore and the `traceparent`, `tracestate` and `baggage` headers are not ported. Header names that differ
//!   only in case are merged like Node's `setHeader`: the later name and value, in the earlier one's place.
//! - Errors carry the SDK's own properties in `PiError.data`: `$fault` for a service exception (`is_service_exception`)
//!   and `$metadata` for an error thrown from a response. The hidden `$response` is not kept.
//! - Version: the schema and user agent are those of `@aws-sdk/client-bedrock-runtime` 3.1126.0, the version in
//!   `node_modules`.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::{Duration, UNIX_EPOCH};

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{PayloadChecksumKind, SignableBody, SignableRequest, SigningSettings, sign};
use aws_sigv4::sign::v4;
use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_types::event_stream::Message;
use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use bytes::Bytes;
use futures::StreamExt;
use indexmap::IndexMap;
use pi_host::js::{date_parse, js_key_order, json_stringify, number_to_string, string_to_number};
use pi_host::{AbortSignal, Body, HttpRequest, HttpResponse, LocalBoxFuture, LocalBoxStream, PiError, PiResult};
use regex::Regex;
use serde_json::{Map, Value, json};
use url::Url;

use crate::types::ProviderResponse;
use crate::utils::json_parse::json_parse;
use crate::utils::text::js_trim;

/// The version of `@aws-sdk/client-bedrock-runtime` this file reproduces.
pub const SDK_VERSION: &str = "3.1126.0";

const SIGNING_NAME: &str = "bedrock";
const DESERIALIZATION_HINT: &str =
	"Deserialization error: to see the raw response, inspect the hidden field {error}.$response on this object.";

/// The `com.amazonaws.bedrockruntime` shapes reachable from `ConverseStreamRequest` and `ConverseStreamResponse`, in
/// member order, one shape per line (lines starting with `+` continue the shape above). Dumped from the SDK's schemas; no
/// shape carries a `jsonName`, `timestampFormat`, `mediaType`, `sparse`, `idempotencyToken` or event binding trait.
const MODEL: &str = "\
AccessDeniedException struct message:string\n\
AnyToolChoice struct\n\
AppliedGuardrailDetails struct guardrailId:string guardrailVersion:string guardrailArn:string\n\
+ guardrailOrigin:list<string> guardrailOwnership:string\n\
AudioBlock struct format:string source:AudioSource error:ErrorBlock\n\
AudioSource union bytes:blob s3Location:S3Location\n\
AutoToolChoice struct\n\
CacheDetail struct ttl:string inputTokens:number\n\
CachePointBlock struct type:string ttl:string\n\
Citation struct title:string source:string sourceContent:list<CitationSourceContent> location:CitationLocation\n\
CitationGeneratedContent union text:string\n\
CitationLocation union web:WebLocation documentChar:DocumentCharLocation documentPage:DocumentPageLocation\n\
+ documentChunk:DocumentChunkLocation searchResultLocation:SearchResultLocation\n\
CitationSourceContent union text:string\n\
CitationSourceContentDelta struct text:string\n\
CitationsConfig struct enabled:boolean\n\
CitationsContentBlock struct content:list<CitationGeneratedContent> citations:list<Citation>\n\
CitationsDelta struct title:string source:string sourceContent:list<CitationSourceContentDelta>\n\
+ location:CitationLocation\n\
ConflictException struct message:string\n\
ContentBlock union text:string image:ImageBlock document:DocumentBlock video:VideoBlock audio:AudioBlock\n\
+ toolUse:ToolUseBlock toolResult:ToolResultBlock guardContent:GuardrailConverseContentBlock\n\
+ cachePoint:CachePointBlock reasoningContent:ReasoningContentBlock citationsContent:CitationsContentBlock\n\
+ searchResult:SearchResultBlock toolAddition:ToolAdditionBlock toolRemoval:ToolRemovalBlock\n\
ContentBlockDelta union text:string toolUse:ToolUseBlockDelta toolResult:list<ToolResultBlockDelta>\n\
+ reasoningContent:ReasoningContentBlockDelta citation:CitationsDelta image:ImageBlockDelta\n\
ContentBlockDeltaEvent struct delta:ContentBlockDelta contentBlockIndex:number\n\
ContentBlockStart union toolUse:ToolUseBlockStart toolResult:ToolResultBlockStart image:ImageBlockStart\n\
ContentBlockStartEvent struct start:ContentBlockStart contentBlockIndex:number\n\
ContentBlockStopEvent struct contentBlockIndex:number\n\
ConverseStreamMetadataEvent struct usage:TokenUsage metrics:ConverseStreamMetrics trace:ConverseStreamTrace\n\
+ performanceConfig:PerformanceConfiguration serviceTier:ServiceTier\n\
ConverseStreamMetrics struct latencyMs:number\n\
ConverseStreamOutput union messageStart:MessageStartEvent contentBlockStart:ContentBlockStartEvent\n\
+ contentBlockDelta:ContentBlockDeltaEvent contentBlockStop:ContentBlockStopEvent messageStop:MessageStopEvent\n\
+ metadata:ConverseStreamMetadataEvent internalServerException:InternalServerException\n\
+ modelStreamErrorException:ModelStreamErrorException validationException:ValidationException\n\
+ throttlingException:ThrottlingException serviceUnavailableException:ServiceUnavailableException\n\
ConverseStreamRequest struct modelId:string messages:list<Message> system:list<SystemContentBlock>\n\
+ inferenceConfig:InferenceConfiguration toolConfig:ToolConfiguration\n\
+ guardrailConfig:GuardrailStreamConfiguration additionalModelRequestFields:document\n\
+ promptVariables:map<PromptVariableValues> additionalModelResponseFieldPaths:list<string>\n\
+ requestMetadata:map<string> performanceConfig:PerformanceConfiguration serviceTier:ServiceTier\n\
+ outputConfig:OutputConfig\n\
ConverseStreamResponse struct stream:ConverseStreamOutput\n\
ConverseStreamTrace struct guardrail:GuardrailTraceAssessment promptRouter:PromptRouterTrace\n\
DocumentBlock struct name:string source:DocumentSource format:string context:string citations:CitationsConfig\n\
DocumentCharLocation struct documentIndex:number start:number end:number\n\
DocumentChunkLocation struct documentIndex:number start:number end:number\n\
DocumentContentBlock union text:string\n\
DocumentPageLocation struct documentIndex:number start:number end:number\n\
DocumentSource union bytes:blob s3Location:S3Location text:string content:list<DocumentContentBlock>\n\
ErrorBlock struct message:string\n\
GuardrailAssessment struct topicPolicy:GuardrailTopicPolicyAssessment\n\
+ contentPolicy:GuardrailContentPolicyAssessment wordPolicy:GuardrailWordPolicyAssessment\n\
+ sensitiveInformationPolicy:GuardrailSensitiveInformationPolicyAssessment\n\
+ contextualGroundingPolicy:GuardrailContextualGroundingPolicyAssessment\n\
+ automatedReasoningPolicy:GuardrailAutomatedReasoningPolicyAssessment\n\
+ invocationMetrics:GuardrailInvocationMetrics appliedGuardrailDetails:AppliedGuardrailDetails\n\
GuardrailAutomatedReasoningFinding union valid:GuardrailAutomatedReasoningValidFinding\n\
+ invalid:GuardrailAutomatedReasoningInvalidFinding satisfiable:GuardrailAutomatedReasoningSatisfiableFinding\n\
+ impossible:GuardrailAutomatedReasoningImpossibleFinding\n\
+ translationAmbiguous:GuardrailAutomatedReasoningTranslationAmbiguousFinding\n\
+ tooComplex:GuardrailAutomatedReasoningTooComplexFinding\n\
+ noTranslations:GuardrailAutomatedReasoningNoTranslationsFinding\n\
GuardrailAutomatedReasoningImpossibleFinding struct translation:GuardrailAutomatedReasoningTranslation\n\
+ contradictingRules:list<GuardrailAutomatedReasoningRule>\n\
+ logicWarning:GuardrailAutomatedReasoningLogicWarning\n\
GuardrailAutomatedReasoningInputTextReference struct text:string\n\
GuardrailAutomatedReasoningInvalidFinding struct translation:GuardrailAutomatedReasoningTranslation\n\
+ contradictingRules:list<GuardrailAutomatedReasoningRule>\n\
+ logicWarning:GuardrailAutomatedReasoningLogicWarning\n\
GuardrailAutomatedReasoningLogicWarning struct type:string premises:list<GuardrailAutomatedReasoningStatement>\n\
+ claims:list<GuardrailAutomatedReasoningStatement>\n\
GuardrailAutomatedReasoningNoTranslationsFinding struct\n\
GuardrailAutomatedReasoningPolicyAssessment struct findings:list<GuardrailAutomatedReasoningFinding>\n\
GuardrailAutomatedReasoningRule struct identifier:string policyVersionArn:string\n\
GuardrailAutomatedReasoningSatisfiableFinding struct translation:GuardrailAutomatedReasoningTranslation\n\
+ claimsTrueScenario:GuardrailAutomatedReasoningScenario\n\
+ claimsFalseScenario:GuardrailAutomatedReasoningScenario logicWarning:GuardrailAutomatedReasoningLogicWarning\n\
GuardrailAutomatedReasoningScenario struct statements:list<GuardrailAutomatedReasoningStatement>\n\
GuardrailAutomatedReasoningStatement struct logic:string naturalLanguage:string\n\
GuardrailAutomatedReasoningTooComplexFinding struct\n\
GuardrailAutomatedReasoningTranslation struct premises:list<GuardrailAutomatedReasoningStatement>\n\
+ claims:list<GuardrailAutomatedReasoningStatement>\n\
+ untranslatedPremises:list<GuardrailAutomatedReasoningInputTextReference>\n\
+ untranslatedClaims:list<GuardrailAutomatedReasoningInputTextReference> confidence:number\n\
GuardrailAutomatedReasoningTranslationAmbiguousFinding struct\n\
+ options:list<GuardrailAutomatedReasoningTranslationOption>\n\
+ differenceScenarios:list<GuardrailAutomatedReasoningScenario>\n\
GuardrailAutomatedReasoningTranslationOption struct translations:list<GuardrailAutomatedReasoningTranslation>\n\
GuardrailAutomatedReasoningValidFinding struct translation:GuardrailAutomatedReasoningTranslation\n\
+ claimsTrueScenario:GuardrailAutomatedReasoningScenario supportingRules:list<GuardrailAutomatedReasoningRule>\n\
+ logicWarning:GuardrailAutomatedReasoningLogicWarning\n\
GuardrailContentFilter struct type:string confidence:string action:string filterStrength:string\n\
+ detected:boolean\n\
GuardrailContentPolicyAssessment struct filters:list<GuardrailContentFilter>\n\
GuardrailContextualGroundingFilter struct type:string threshold:number score:number action:string\n\
+ detected:boolean\n\
GuardrailContextualGroundingPolicyAssessment struct filters:list<GuardrailContextualGroundingFilter>\n\
GuardrailConverseContentBlock union text:GuardrailConverseTextBlock image:GuardrailConverseImageBlock\n\
GuardrailConverseImageBlock struct format:string source:GuardrailConverseImageSource\n\
GuardrailConverseImageSource union bytes:blob\n\
GuardrailConverseTextBlock struct text:string qualifiers:list<string>\n\
GuardrailCoverage struct textCharacters:GuardrailTextCharactersCoverage images:GuardrailImageCoverage\n\
GuardrailCustomWord struct match:string action:string detected:boolean\n\
GuardrailImageCoverage struct guarded:number total:number\n\
GuardrailInvocationMetrics struct guardrailProcessingLatency:number usage:GuardrailUsage\n\
+ guardrailCoverage:GuardrailCoverage\n\
GuardrailManagedWord struct match:string type:string action:string detected:boolean\n\
GuardrailPiiEntityFilter struct match:string type:string action:string detected:boolean\n\
GuardrailRegexFilter struct action:string name:string match:string regex:string detected:boolean\n\
GuardrailSensitiveInformationPolicyAssessment struct piiEntities:list<GuardrailPiiEntityFilter>\n\
+ regexes:list<GuardrailRegexFilter>\n\
GuardrailStreamConfiguration struct guardrailIdentifier:string guardrailVersion:string trace:string\n\
+ streamProcessingMode:string\n\
GuardrailTextCharactersCoverage struct guarded:number total:number\n\
GuardrailTopic struct name:string type:string action:string detected:boolean\n\
GuardrailTopicPolicyAssessment struct topics:list<GuardrailTopic>\n\
GuardrailTraceAssessment struct modelOutput:list<string> inputAssessment:map<GuardrailAssessment>\n\
+ outputAssessments:map<list<GuardrailAssessment>> actionReason:string\n\
GuardrailUsage struct topicPolicyUnits:number contentPolicyUnits:number wordPolicyUnits:number\n\
+ sensitiveInformationPolicyUnits:number sensitiveInformationPolicyFreeUnits:number\n\
+ contextualGroundingPolicyUnits:number contentPolicyImageUnits:number automatedReasoningPolicyUnits:number\n\
+ automatedReasoningPolicies:number\n\
GuardrailWordPolicyAssessment struct customWords:list<GuardrailCustomWord>\n\
+ managedWordLists:list<GuardrailManagedWord>\n\
ImageBlock struct format:string source:ImageSource error:ErrorBlock\n\
ImageBlockDelta struct source:ImageSource error:ErrorBlock\n\
ImageBlockStart struct format:string\n\
ImageSource union bytes:blob s3Location:S3Location\n\
InferenceConfiguration struct maxTokens:number temperature:number topP:number stopSequences:list<string>\n\
InternalServerException struct message:string\n\
JsonSchemaDefinition struct schema:string name:string description:string\n\
Message struct role:string content:list<ContentBlock>\n\
MessageStartEvent struct role:string\n\
MessageStopEvent struct stopReason:string additionalModelResponseFields:document\n\
ModelErrorException struct message:string originalStatusCode:number resourceName:string\n\
ModelNotReadyException struct message:string\n\
ModelStreamErrorException struct message:string originalStatusCode:number originalMessage:string\n\
ModelTimeoutException struct message:string\n\
OutputConfig struct textFormat:OutputFormat effort:string\n\
OutputFormat struct type:string structure:OutputFormatStructure\n\
OutputFormatStructure union jsonSchema:JsonSchemaDefinition\n\
PerformanceConfiguration struct latency:string\n\
PromptRouterTrace struct invokedModelId:string\n\
PromptVariableValues union text:string\n\
ReasoningContentBlock union reasoningText:ReasoningTextBlock redactedContent:blob\n\
ReasoningContentBlockDelta union text:string redactedContent:blob signature:string\n\
ReasoningTextBlock struct text:string signature:string\n\
ResourceNotFoundException struct message:string\n\
S3Location struct uri:string bucketOwner:string\n\
SearchResultBlock struct source:string title:string content:list<SearchResultContentBlock>\n\
+ citations:CitationsConfig\n\
SearchResultContentBlock struct text:string\n\
SearchResultLocation struct searchResultIndex:number start:number end:number\n\
ServiceQuotaExceededException struct message:string\n\
ServiceTier struct type:string\n\
ServiceUnavailableException struct message:string\n\
SpecificToolChoice struct name:string\n\
SystemContentBlock union text:string guardContent:GuardrailConverseContentBlock cachePoint:CachePointBlock\n\
SystemTool struct name:string\n\
ThrottlingException struct message:string\n\
TokenUsage struct inputTokens:number outputTokens:number totalTokens:number cacheReadInputTokens:number\n\
+ cacheWriteInputTokens:number cacheDetails:list<CacheDetail>\n\
Tool union toolSpec:ToolSpecification systemTool:SystemTool cachePoint:CachePointBlock\n\
ToolAdditionBlock struct tool:ToolReference\n\
ToolChoice union auto:AutoToolChoice any:AnyToolChoice tool:SpecificToolChoice\n\
ToolConfiguration struct tools:list<Tool> toolChoice:ToolChoice\n\
ToolInputSchema union json:document\n\
ToolReference struct type:string name:string serverName:string\n\
ToolRemovalBlock struct tool:ToolReference\n\
ToolResultBlock struct toolUseId:string content:list<ToolResultContentBlock> status:string type:string\n\
ToolResultBlockDelta union text:string json:document\n\
ToolResultBlockStart struct toolUseId:string type:string status:string\n\
ToolResultContentBlock union json:document text:string image:ImageBlock document:DocumentBlock\n\
+ video:VideoBlock searchResult:SearchResultBlock\n\
ToolSpecification struct name:string inputSchema:ToolInputSchema description:string strict:boolean\n\
ToolUseBlock struct toolUseId:string name:string input:document type:string\n\
ToolUseBlockDelta struct input:string\n\
ToolUseBlockStart struct toolUseId:string name:string type:string\n\
ValidationException struct message:string\n\
VideoBlock struct format:string source:VideoSource\n\
VideoSource union bytes:blob s3Location:S3Location\n\
WebLocation struct url:string domain:string";

/// Error shapes and their `error` trait (`$fault`).
const ERROR_SHAPES: [(&str, &str); 12] = [
	("AccessDeniedException", "client"),
	("ConflictException", "client"),
	("InternalServerException", "server"),
	("ModelErrorException", "client"),
	("ModelNotReadyException", "client"),
	("ModelStreamErrorException", "client"),
	("ModelTimeoutException", "client"),
	("ResourceNotFoundException", "client"),
	("ServiceQuotaExceededException", "client"),
	("ServiceUnavailableException", "server"),
	("ThrottlingException", "client"),
	("ValidationException", "client"),
];

struct Shape {
	union: bool,
	members: Vec<(&'static str, &'static str)>,
}

static SHAPES: LazyLock<HashMap<&'static str, Shape>> = LazyLock::new(|| {
	let mut shapes = HashMap::new();
	let mut current: Option<(&'static str, Shape)> = None;
	for line in MODEL.lines() {
		let mut words = line.split_whitespace().peekable();
		if words.next_if_eq(&"+").is_none() {
			if let Some((name, shape)) = current.take() {
				shapes.insert(name, shape);
			}
			let name = words.next().expect("a model line starts with a shape name");
			let union = words.next() == Some("union");
			current = Some((
				name,
				Shape {
					union,
					members: Vec::new(),
				},
			));
		}
		let (_, shape) = current.as_mut().expect("the model starts with a shape");
		shape
			.members
			.extend(words.map(|word| word.split_once(':').expect("a member is name:type")));
	}
	if let Some((name, shape)) = current {
		shapes.insert(name, shape);
	}
	shapes
});

/// A member type of the model.
#[derive(Clone, Copy)]
enum Ty {
	String,
	Number,
	Boolean,
	Blob,
	Document,
	List(&'static str),
	Map(&'static str),
	Shape(&'static Shape),
}

fn ty(name: &'static str) -> Ty {
	match name {
		"string" => Ty::String,
		"number" => Ty::Number,
		"boolean" => Ty::Boolean,
		"blob" => Ty::Blob,
		"document" => Ty::Document,
		_ => {
			if let Some(item) = name.strip_prefix("list<").and_then(|rest| rest.strip_suffix('>')) {
				return Ty::List(item);
			}
			if let Some(value) = name.strip_prefix("map<").and_then(|rest| rest.strip_suffix('>')) {
				return Ty::Map(value);
			}
			Ty::Shape(SHAPES.get(name).expect("the model names only shapes it defines"))
		}
	}
}

fn shape(name: &str) -> &'static Shape {
	SHAPES
		.get(name)
		.expect("the client names only shapes the model defines")
}

fn is_primitive(ty: Ty) -> bool {
	matches!(ty, Ty::String | Ty::Number | Ty::Boolean)
}

// ---------------------------------------------------------------------------------------------------------------
// Serialization (`JsonShapeSerializer2` of the restJson1 protocol)
// ---------------------------------------------------------------------------------------------------------------

/// `writeAscii(s)`: each UTF-16 code unit is written as one byte (`json[i] = s.charCodeAt(j)` into a `Uint8Array`).
fn write_ascii(out: &mut Vec<u8>, text: &str) {
	out.extend(text.encode_utf16().map(|unit| unit as u8));
}

fn write_ascii_quoted(out: &mut Vec<u8>, text: &str) {
	out.push(b'"');
	write_ascii(out, text);
	out.push(b'"');
}

/// `writeJsonString(s)`, which escapes like `JSON.stringify`.
fn write_json_string(out: &mut Vec<u8>, text: &str) {
	out.extend_from_slice(json_stringify(text).as_bytes());
}

/// `writeValue(schema, value, container)` for a value that is not inside a struct (a struct skips null members).
fn write_value(out: &mut Vec<u8>, ty: Ty, value: &Value) {
	match value {
		Value::Null => out.extend_from_slice(b"null"),
		Value::Object(object) => match ty {
			Ty::Shape(shape) => write_struct(out, shape, object, None),
			Ty::Map(value_ty) => write_map(out, Some(value_ty), object),
			Ty::Document => write_map(out, None, object),
			_ => write_ascii(out, &json_stringify(value)),
		},
		Value::Array(items) => match ty {
			Ty::Shape(shape) => write_struct(out, shape, &Map::new(), None),
			Ty::List(item_ty) => write_list(out, Some(item_ty), items),
			Ty::Document => write_list(out, None, items),
			Ty::Map(value_ty) => {
				let indexed = items
					.iter()
					.enumerate()
					.map(|(index, item)| (index.to_string(), item.clone()))
					.collect();
				write_map(out, Some(value_ty), &indexed);
			}
			_ => write_ascii(out, &json_stringify(value)),
		},
		// Blobs hold base64 text in the Rust port (see the file header), written as the SDK writes the bytes.
		Value::String(text) if matches!(ty, Ty::Blob) => write_ascii_quoted(out, text),
		Value::String(text) => write_json_string(out, text),
		Value::Number(number) => write_ascii(
			out,
			&number_to_string(number.as_f64().expect("a JSON number reads as f64")),
		),
		Value::Bool(flag) => out.extend_from_slice(if *flag { b"true" } else { b"false" }),
	}
}

/// `writeStruct(ns, value)`. `only` limits the members to the request payload members.
fn write_struct(out: &mut Vec<u8>, shape: &Shape, value: &Map<String, Value>, only: Option<&[&str]>) {
	out.push(b'{');
	let mut wrote_any = false;
	let has_type = value.get("__type").is_some_and(Value::is_string);
	let mut written_keys: HashSet<&str> = HashSet::new();
	for &(name, member_ty) in &shape.members {
		if only.is_some_and(|only| !only.contains(&name)) {
			continue;
		}
		let Some(item) = value.get(name).filter(|item| !item.is_null()) else {
			continue;
		};
		if wrote_any {
			out.push(b',');
		}
		wrote_any = true;
		written_keys.insert(name);
		write_ascii_quoted(out, name);
		out.push(b':');
		write_value(out, ty(member_ty), item);
	}
	if !wrote_any && shape.union {
		if let Some(Value::Array(unknown)) = value.get("$unknown") {
			write_ascii_quoted(out, &js_string(unknown.first().unwrap_or(&Value::Null)));
			out.push(b':');
			// `writeValue(15, undefined, union)` writes nothing for a missing value.
			if let Some(item) = unknown.get(1) {
				if item.is_null() {
					out.extend_from_slice(b"null");
				} else {
					write_value(out, Ty::Document, item);
				}
			}
		}
	} else if has_type {
		for (key, item) in value {
			if !written_keys.insert(key.as_str()) {
				continue;
			}
			if wrote_any {
				out.push(b',');
			}
			wrote_any = true;
			write_ascii_quoted(out, key);
			out.push(b':');
			write_value(out, Ty::Document, item);
		}
	}
	out.push(b'}');
}

/// `writeList(ns, value, isDocument)`. `item_ty` is `None` for a document list.
fn write_list(out: &mut Vec<u8>, item_ty: Option<&'static str>, items: &[Value]) {
	let item_ty = item_ty.map(ty);
	if item_ty.is_some_and(is_primitive) {
		let kept: Vec<&Value> = items.iter().filter(|item| !item.is_null()).collect();
		out.extend_from_slice(json_stringify(&kept).as_bytes());
		return;
	}
	out.push(b'[');
	let mut wrote_first = false;
	for item in items {
		if item_ty.is_some() && item.is_null() {
			continue;
		}
		if wrote_first {
			out.push(b',');
		}
		write_value(out, item_ty.unwrap_or(Ty::Document), item);
		wrote_first = true;
	}
	out.push(b']');
}

/// `writeMap(ns, value, isDocument)`. `value_ty` is `None` for a document map.
fn write_map(out: &mut Vec<u8>, value_ty: Option<&'static str>, entries: &Map<String, Value>) {
	let value_ty = value_ty.map(ty);
	if value_ty.is_some_and(is_primitive) {
		let kept: Map<String, Value> = entries
			.iter()
			.filter(|(_, item)| !item.is_null())
			.map(|(key, item)| (key.clone(), item.clone()))
			.collect();
		out.extend_from_slice(json_stringify(&kept).as_bytes());
		return;
	}
	out.push(b'{');
	let mut first = true;
	for (key, item) in entries {
		if value_ty.is_some() && item.is_null() {
			continue;
		}
		if !first {
			out.push(b',');
		}
		first = false;
		write_json_string(out, key);
		out.push(b':');
		write_value(out, value_ty.unwrap_or(Ty::Document), item);
	}
	out.push(b'}');
}

/// `String(value)` of a JSON value.
fn js_string(value: &Value) -> String {
	match value {
		Value::Null => "null".into(),
		Value::Bool(flag) => flag.to_string(),
		Value::Number(number) => number_to_string(number.as_f64().expect("a JSON number reads as f64")),
		Value::String(text) => text.clone(),
		Value::Array(items) => items
			.iter()
			.map(|item| if item.is_null() { String::new() } else { js_string(item) })
			.collect::<Vec<_>>()
			.join(","),
		Value::Object(_) => "[object Object]".into(),
	}
}

/// `extendedEncodeURIComponent(str)` and `escapeUri(uri)`: every byte but `A-Za-z0-9-_.~` percent-encoded.
fn escape_uri(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	for byte in text.bytes() {
		if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
			out.push(byte as char);
		} else {
			out.push_str(&format!("%{byte:02X}"));
		}
	}
	out
}

/// The serialized `ConverseStream` request: the URL path, and the JSON body.
struct SerializedRequest {
	path: String,
	body: Vec<u8>,
}

/// `HttpBindingProtocol.serializeRequest` + `AwsRestJsonProtocol.serializeRequest` for `ConverseStream`
/// (`POST /model/{modelId}/converse-stream`; `modelId` is the only HTTP-bound member).
fn serialize_converse_stream(input: &Value, endpoint_path: &str) -> PiResult<SerializedRequest> {
	let mut input = input.clone();
	js_key_order(&mut input);
	let empty = Map::new();
	let input = input.as_object().unwrap_or(&empty);
	let operation_path = "/model/{modelId}/converse-stream";
	let mut path = if endpoint_path == "/" {
		operation_path.to_string()
	} else {
		format!("{endpoint_path}{operation_path}")
	};
	let request_shape = shape("ConverseStreamRequest");
	let mut payload_members = Vec::new();
	for &(name, _) in &request_shape.members {
		let value = input.get(name).filter(|value| !value.is_null());
		if name == "modelId" {
			let Some(value) = value else {
				if path.contains("{modelId}") {
					return Err(PiError::new("No value provided for input HTTP label: modelId."));
				}
				continue;
			};
			path = path.replacen("{modelId}", &escape_uri(&js_string(value)), 1);
			continue;
		}
		if value.is_some() {
			payload_members.push(name);
		}
	}
	let body = if payload_members.is_empty() {
		b"{}".to_vec()
	} else {
		let mut out = Vec::new();
		write_struct(&mut out, request_shape, input, Some(&payload_members));
		out
	};
	Ok(SerializedRequest { path, body })
}

// ---------------------------------------------------------------------------------------------------------------
// Deserialization (`JsonShapeDeserializer2`)
// ---------------------------------------------------------------------------------------------------------------

/// `read(schema, bytes)`: an empty body reads as `{}`, otherwise `JSON.parse` of the UTF-8 text.
fn read_json(ty: Ty, body: &[u8]) -> PiResult<Value> {
	if body.is_empty() {
		return Ok(Value::Object(Map::new()));
	}
	read_value(ty, json_parse(&String::from_utf8_lossy(body))?)
}

/// `_read(schema, value)`.
fn read_value(value_ty: Ty, value: Value) -> PiResult<Value> {
	match (value_ty, value) {
		(Ty::Shape(shape), Value::Object(record)) => read_struct(shape, record),
		(Ty::Shape(shape), Value::Array(items)) => {
			let record = items
				.into_iter()
				.enumerate()
				.map(|(index, item)| (index.to_string(), item))
				.collect();
			read_struct(shape, record)
		}
		(Ty::List(item_ty), Value::Array(items)) => {
			let item_ty = ty(item_ty);
			items
				.into_iter()
				.map(|item| read_value(item_ty, item))
				.collect::<PiResult<Vec<_>>>()
				.map(Value::Array)
		}
		(Ty::Map(value_ty), Value::Object(entries)) => {
			let value_ty = ty(value_ty);
			entries
				.into_iter()
				.map(|(key, item)| Ok((key, read_value(value_ty, item)?)))
				.collect::<PiResult<Map<_, _>>>()
				.map(Value::Object)
		}
		(Ty::Blob, Value::String(text)) => from_base64(&text).map(|bytes| Value::String(to_base64(&bytes))),
		(_, value) => Ok(value),
	}
}

/// `_readStruct(ns, record)`: known members that are not null, `$unknown` for a union with one unknown member, and
/// the remaining keys of a struct record that has a string `__type`.
fn read_struct(shape: &Shape, record: Map<String, Value>) -> PiResult<Value> {
	let has_type = record.get("__type").is_some_and(Value::is_string);
	let mut out = Map::new();
	for &(name, member_ty) in &shape.members {
		if let Some(item) = record.get(name).filter(|item| !item.is_null()) {
			out.insert(name.to_string(), read_value(ty(member_ty), item.clone())?);
		}
	}
	if shape.union {
		let unknown: Vec<&String> = record
			.keys()
			.filter(|key| key.as_str() != "__type" && !shape.members.iter().any(|(name, _)| name == key))
			.collect();
		if unknown.len() == 1 && out.is_empty() {
			let key = unknown[0].clone();
			let item = record[&key].clone();
			out.insert("$unknown".into(), json!([key, item]));
		}
	} else if has_type {
		for (key, item) in record {
			if !out.contains_key(&key) {
				out.insert(key, item);
			}
		}
	}
	Ok(Value::Object(out))
}

/// Node's `Buffer` base64 decoder: lenient about non-canonical trailing bits.
const BASE64_DECODER: GeneralPurpose = GeneralPurpose::new(
	&base64::alphabet::STANDARD,
	GeneralPurposeConfig::new()
		.with_decode_allow_trailing_bits(true)
		.with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// `fromBase64(input)` of `@smithy/util-base64` (Node).
fn from_base64(input: &str) -> PiResult<Vec<u8>> {
	if !(input.encode_utf16().count() * 3).is_multiple_of(4) {
		return Err(PiError::type_error("Incorrect padding on base64 string."));
	}
	static BASE64: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[A-Za-z0-9+/]*={0,2}$").expect("valid regex"));
	if !BASE64.is_match(input) {
		return Err(PiError::type_error("Invalid base64 string."));
	}
	Ok(BASE64_DECODER.decode(input).unwrap_or_default())
}

fn to_base64(bytes: &[u8]) -> String {
	base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ---------------------------------------------------------------------------------------------------------------
// Shared config files and settings (`@smithy/core/config`)
// ---------------------------------------------------------------------------------------------------------------

/// Parsed INI data: section name to its keys, in file order.
type IniData = IndexMap<String, IndexMap<String, String>>;

/// `IniSectionType` values.
const INI_SECTION_TYPES: [&str; 3] = ["profile", "sso-session", "services"];

/// `parseIni(iniData)`.
fn parse_ini(ini_data: &str) -> PiResult<IniData> {
	static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|\s)[;#]").expect("valid regex"));
	// JS uses the backreference `(["'])?...\2`; the Rust regex has none, so the quotes are compared below.
	static PREFIX_KEY: LazyLock<Regex> = LazyLock::new(|| {
		Regex::new(r#"^([A-Za-z0-9_-]+)\s(["']?)([A-Za-z0-9_\-@+.%:/]+)(["']?)$"#).expect("valid regex")
	});
	let mut map = IniData::new();
	let mut current_section: Option<String> = None;
	let mut current_sub_section: Option<String> = None;
	for ini_line in ini_data.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)) {
		let uncommented = COMMENT
			.find(ini_line)
			.map_or(ini_line, |found| &ini_line[..found.start()]);
		let trimmed_line = js_trim(uncommented);
		if trimmed_line.len() >= 2 && trimmed_line.starts_with('[') && trimmed_line.ends_with(']') {
			current_section = None;
			current_sub_section = None;
			let section_name = &trimmed_line[1..trimmed_line.len() - 1];
			match PREFIX_KEY
				.captures(section_name)
				.filter(|captures| captures[2] == captures[4])
			{
				Some(captures) => {
					if INI_SECTION_TYPES.contains(&&captures[1]) {
						current_section = Some(format!("{}.{}", &captures[1], &captures[3]));
					}
				}
				None => current_section = Some(section_name.to_string()),
			}
			if section_name == "__proto__" || section_name == "profile __proto__" {
				return Err(PiError::new(format!("Found invalid profile name \"{section_name}\"")));
			}
			continue;
		}
		let Some(section) = &current_section else {
			continue;
		};
		let Some(index_of_equals_sign) = trimmed_line.find('=').filter(|index| *index != 0) else {
			continue;
		};
		let name = js_trim(&trimmed_line[..index_of_equals_sign]);
		let value = js_trim(&trimmed_line[index_of_equals_sign + 1..]);
		if value.is_empty() {
			current_sub_section = Some(name.to_string());
			continue;
		}
		if current_sub_section.is_some() && ini_line.trim_start() == ini_line {
			current_sub_section = None;
		}
		let key = match &current_sub_section {
			Some(sub_section) => format!("{sub_section}.{name}"),
			None => name.to_string(),
		};
		map.entry(section.clone()).or_default().insert(key, value.to_string());
	}
	Ok(map)
}

/// `getConfigData(data)`: the config file's `[default]` and `[profile x]` sections by profile name, and its other
/// typed sections (`sso-session.x`) by their full key.
fn get_config_data(data: IniData) -> IniData {
	let mut config = IniData::new();
	if let Some(default) = data.get("default") {
		config.insert("default".into(), default.clone());
	}
	for (key, value) in data {
		let Some((prefix, name)) = key.split_once('.') else {
			continue;
		};
		if !INI_SECTION_TYPES.contains(&prefix) {
			continue;
		}
		let updated_key = if prefix == "profile" {
			name.to_string()
		} else {
			key.clone()
		};
		config.insert(updated_key, value);
	}
	config
}

/// `getHomeDir()`.
fn get_home_dir() -> String {
	if let Some(home) = pi_host::env("HOME").filter(|home| !home.is_empty()) {
		return home;
	}
	if let Some(profile) = pi_host::env("USERPROFILE").filter(|profile| !profile.is_empty()) {
		return profile;
	}
	if let Some(path) = pi_host::env("HOMEPATH").filter(|path| !path.is_empty()) {
		let drive = pi_host::env("HOMEDRIVE").unwrap_or_else(|| format!("C:{}", std::path::MAIN_SEPARATOR));
		return format!("{drive}{path}");
	}
	pi_host::home_dir().unwrap_or_default()
}

/// The shared config and credentials files, read once per request.
struct SharedConfigFiles {
	config_file: IniData,
	credentials_file: IniData,
	/// `getProfileName(init)`: `init.profile || AWS_PROFILE || "default"`.
	profile: String,
}

/// Where a setting of the shared config files is read from first.
#[derive(Clone, Copy, PartialEq)]
enum PreferredFile {
	Config,
	Credentials,
}

impl SharedConfigFiles {
	/// `loadSharedConfigFiles(init)`; a file that is missing or fails to parse reads as empty.
	async fn load(profile: Option<&str>) -> Self {
		let home_dir = get_home_dir();
		let resolve = |path: String| match path.strip_prefix("~/") {
			Some(rest) => std::path::Path::new(&home_dir)
				.join(rest)
				.to_string_lossy()
				.into_owned(),
			None => path,
		};
		let default_path = |file: &str| {
			std::path::Path::new(&home_dir)
				.join(".aws")
				.join(file)
				.to_string_lossy()
				.into_owned()
		};
		let config_path = pi_host::env("AWS_CONFIG_FILE")
			.filter(|path| !path.is_empty())
			.unwrap_or_else(|| default_path("config"));
		let credentials_path = pi_host::env("AWS_SHARED_CREDENTIALS_FILE")
			.filter(|path| !path.is_empty())
			.unwrap_or_else(|| default_path("credentials"));
		let config_file = read_ini(&resolve(config_path))
			.await
			.map(get_config_data)
			.unwrap_or_default();
		let credentials_file = read_ini(&resolve(credentials_path)).await.unwrap_or_default();
		let profile = profile
			.filter(|profile| !profile.is_empty())
			.map(str::to_string)
			.or_else(|| pi_host::env("AWS_PROFILE").filter(|profile| !profile.is_empty()))
			.unwrap_or_else(|| "default".into());
		Self {
			config_file,
			credentials_file,
			profile,
		}
	}

	/// The merged profile `fromSharedConfigFiles` hands to a config selector.
	fn profile_settings(&self, preferred: PreferredFile) -> IndexMap<String, String> {
		let from_credentials = self.credentials_file.get(&self.profile).cloned().unwrap_or_default();
		let from_config = self.config_file.get(&self.profile).cloned().unwrap_or_default();
		let (mut merged, overrides) = match preferred {
			PreferredFile::Config => (from_credentials, from_config),
			PreferredFile::Credentials => (from_config, from_credentials),
		};
		merged.extend(overrides);
		merged
	}

	/// `loadConfig` of a string setting: the environment variable when it is set (even empty), else the profile key.
	fn setting(&self, env_key: &str, config_keys: &[&str], preferred: PreferredFile) -> Option<String> {
		if let Some(value) = pi_host::env(env_key) {
			return Some(value);
		}
		let profile = self.profile_settings(preferred);
		config_keys.iter().find_map(|key| profile.get(*key).cloned())
	}

	/// A boolean setting (`loadConfig` with `booleanSelector`): the env value, then the profile value, then `false`. A
	/// value other than `"true"` or `"false"` fails its source, and `chain` moves on to the next one.
	fn boolean_setting(&self, env_key: &str, config_key: &str) -> bool {
		let parse = |value: Option<&str>| match value {
			Some("true") => Some(true),
			Some("false") => Some(false),
			_ => None,
		};
		parse(pi_host::env(env_key).as_deref())
			.or_else(|| {
				parse(
					self.profile_settings(PreferredFile::Config)
						.get(config_key)
						.map(String::as_str),
				)
			})
			.unwrap_or(false)
	}

	/// `NODE_MAX_ATTEMPT_CONFIG_OPTIONS`: `AWS_MAX_ATTEMPTS`, then `max_attempts`, then 3. An empty value is unset, and
	/// one that `parseInt` reads as `NaN` fails its source, so the next one is tried.
	fn max_attempts(&self) -> f64 {
		let parse = |value: Option<&str>| {
			value
				.filter(|value| !value.is_empty())
				.map(js_parse_int)
				.filter(|parsed| !parsed.is_nan())
		};
		parse(pi_host::env("AWS_MAX_ATTEMPTS").as_deref())
			.or_else(|| {
				parse(
					self.profile_settings(PreferredFile::Config)
						.get("max_attempts")
						.map(String::as_str),
				)
			})
			.unwrap_or(3.0)
	}
}

/// `readFile(path).then(parseIni)`, `None` when the file is missing or does not parse.
async fn read_ini(path: &str) -> Option<IniData> {
	let text = pi_host::read_file(path).await.ok()?;
	parse_ini(&text).ok()
}

/// `parseInt(text)` without a radix.
fn js_parse_int(text: &str) -> f64 {
	let text = js_trim(text);
	let (sign, rest) = match text.as_bytes().first() {
		Some(b'-') => (-1.0, &text[1..]),
		Some(b'+') => (1.0, &text[1..]),
		_ => (1.0, text),
	};
	let (radix, digits) = match rest.get(..2) {
		Some("0x" | "0X") => (16, &rest[2..]),
		_ => (10, rest),
	};
	let end = digits.find(|ch: char| !ch.is_digit(radix)).unwrap_or(digits.len());
	if end == 0 {
		return f64::NAN;
	}
	let value = digits[..end].chars().fold(0.0, |total, ch| {
		total * f64::from(radix) + f64::from(ch.to_digit(radix).expect("a digit of the radix"))
	});
	sign * value
}

/// `getArrayForCommaSeparatedString(str)`.
fn comma_separated(text: &str) -> Vec<String> {
	if text.is_empty() {
		return Vec::new();
	}
	text.split(',').map(|item| js_trim(item).to_string()).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Region and endpoint (`resolveRegionConfig`, the Bedrock Runtime endpoint rule set, `partition`)
// ---------------------------------------------------------------------------------------------------------------

fn is_fips_region(region: &str) -> bool {
	region.starts_with("fips-") || region.ends_with("-fips")
}

/// `getRealRegion(region)`.
fn get_real_region(region: &str) -> String {
	if !is_fips_region(region) {
		return region.to_string();
	}
	if region == "fips-aws-global" || region == "aws-fips" {
		return "us-east-1".into();
	}
	static FIPS: LazyLock<Regex> = LazyLock::new(|| Regex::new("fips-(dkr-|prod-)?|-fips").expect("valid regex"));
	FIPS.replace(region, "").into_owned()
}

/// `isValidHostLabel(value)`: `/^(?!.*-$)(?!-)[a-zA-Z0-9-]{1,63}$/`.
fn is_valid_host_label(value: &str) -> bool {
	(1..=63).contains(&value.len())
		&& value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
		&& !value.starts_with('-')
		&& !value.ends_with('-')
}

/// A partition's endpoint outputs (`partitions.json`); every partition supports FIPS and dual-stack.
struct Partition {
	region_regex: &'static str,
	global_region: &'static str,
	dns_suffix: &'static str,
	dual_stack_dns_suffix: &'static str,
}

const PARTITIONS: [Partition; 8] = [
	Partition {
		region_regex: r"^(us|eu|ap|sa|ca|me|af|il|mx)\-\w+\-\d+$",
		global_region: "aws-global",
		dns_suffix: "amazonaws.com",
		dual_stack_dns_suffix: "api.aws",
	},
	Partition {
		region_regex: r"^cn\-\w+\-\d+$",
		global_region: "aws-cn-global",
		dns_suffix: "amazonaws.com.cn",
		dual_stack_dns_suffix: "api.amazonwebservices.com.cn",
	},
	Partition {
		region_regex: r"^eusc\-(de)\-\w+\-\d+$",
		global_region: "",
		dns_suffix: "amazonaws.eu",
		dual_stack_dns_suffix: "api.amazonwebservices.eu",
	},
	Partition {
		region_regex: r"^us\-iso\-\w+\-\d+$",
		global_region: "aws-iso-global",
		dns_suffix: "c2s.ic.gov",
		dual_stack_dns_suffix: "api.aws.ic.gov",
	},
	Partition {
		region_regex: r"^us\-isob\-\w+\-\d+$",
		global_region: "aws-iso-b-global",
		dns_suffix: "sc2s.sgov.gov",
		dual_stack_dns_suffix: "api.aws.scloud",
	},
	Partition {
		region_regex: r"^eu\-isoe\-\w+\-\d+$",
		global_region: "aws-iso-e-global",
		dns_suffix: "cloud.adc-e.uk",
		dual_stack_dns_suffix: "api.cloud-aws.adc-e.uk",
	},
	Partition {
		region_regex: r"^us\-isof\-\w+\-\d+$",
		global_region: "aws-iso-f-global",
		dns_suffix: "csp.hci.ic.gov",
		dual_stack_dns_suffix: "api.aws.hci.ic.gov",
	},
	Partition {
		region_regex: r"^us\-gov\-\w+\-\d+$",
		global_region: "aws-us-gov-global",
		dns_suffix: "amazonaws.com",
		dual_stack_dns_suffix: "api.aws",
	},
];

/// `partition(region)`. Every region a partition lists matches its regex except its global region, so the region
/// lists reduce to those names. JS `\w` is ASCII, hence `(?-u)`.
fn partition(region: &str) -> &'static Partition {
	static REGION_REGEXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
		PARTITIONS
			.iter()
			.map(|partition| Regex::new(&format!("(?-u){}", partition.region_regex)).expect("valid regex"))
			.collect()
	});
	PARTITIONS
		.iter()
		.find(|partition| !partition.global_region.is_empty() && partition.global_region == region)
		.or_else(|| {
			PARTITIONS
				.iter()
				.zip(REGION_REGEXES.iter())
				.find_map(|(partition, regex)| regex.is_match(region).then_some(partition))
		})
		.unwrap_or(&PARTITIONS[0])
}

/// The endpoint the rule set resolves: `endpoint` when the caller set one, else the partition's regional endpoint.
fn resolve_endpoint(endpoint: Option<&str>, region: &str, use_fips: bool, use_dual_stack: bool) -> PiResult<Url> {
	if let Some(endpoint) = endpoint {
		if use_fips {
			return Err(PiError::new(
				"Invalid Configuration: FIPS and custom endpoint are not supported",
			));
		}
		if use_dual_stack {
			return Err(PiError::new(
				"Invalid Configuration: Dualstack and custom endpoint are not supported",
			));
		}
		// `parseUrl` keeps protocol, host, port and path; the rule set parses the rebuilt string again.
		let parsed = Url::parse(endpoint).map_err(|_| PiError::type_error("Invalid URL"))?;
		let port = parsed.port().map(|port| format!(":{port}")).unwrap_or_default();
		let rebuilt = format!(
			"{}://{}{port}{}",
			parsed.scheme(),
			parsed.host_str().unwrap_or(""),
			parsed.path()
		);
		return Url::parse(&rebuilt).map_err(|_| PiError::type_error("Invalid URL"));
	}
	let partition = partition(region);
	let (host, suffix) = match (use_fips, use_dual_stack) {
		(true, true) => ("bedrock-runtime-fips", partition.dual_stack_dns_suffix),
		(true, false) => ("bedrock-runtime-fips", partition.dns_suffix),
		(false, true) => ("bedrock-runtime", partition.dual_stack_dns_suffix),
		(false, false) => ("bedrock-runtime", partition.dns_suffix),
	};
	Url::parse(&format!("https://{host}.{region}.{suffix}")).map_err(|_| PiError::type_error("Invalid URL"))
}

// ---------------------------------------------------------------------------------------------------------------
// Identities (`defaultProvider` of `@aws-sdk/credential-provider-node`, the bearer token providers)
// ---------------------------------------------------------------------------------------------------------------

/// `AwsCredentialIdentity`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AwsCredentialIdentity {
	pub access_key_id: String,
	pub secret_access_key: String,
	pub session_token: Option<String>,
}

/// The identity of the selected auth scheme, and the user agent feature its `$source` sets.
enum Identity {
	SigV4(AwsCredentialIdentity, Option<&'static str>),
	Bearer(String, Option<&'static str>),
}

/// The two auth schemes of Bedrock Runtime, in the order its auth scheme provider lists them.
const AUTH_SCHEMES: [&str; 2] = ["sigv4", "httpBearerAuth"];

/// `defaultProvider(init)()`: environment variables (unless a profile is set), then the profile's static keys.
fn default_credentials(
	files: &SharedConfigFiles,
	profile_set: bool,
) -> PiResult<(AwsCredentialIdentity, &'static str)> {
	if !profile_set {
		let access_key_id = pi_host::env("AWS_ACCESS_KEY_ID").filter(|value| !value.is_empty());
		let secret_access_key = pi_host::env("AWS_SECRET_ACCESS_KEY").filter(|value| !value.is_empty());
		if let (Some(access_key_id), Some(secret_access_key)) = (access_key_id, secret_access_key) {
			let session_token = pi_host::env("AWS_SESSION_TOKEN").filter(|value| !value.is_empty());
			return Ok((
				AwsCredentialIdentity {
					access_key_id,
					secret_access_key,
					session_token,
				},
				"g",
			));
		}
	}
	// `parseKnownFiles`: the credentials file's keys override the config file's.
	let mut data = files.config_file.get(&files.profile).cloned().unwrap_or_default();
	data.extend(files.credentials_file.get(&files.profile).cloned().unwrap_or_default());
	let has = |key: &str| data.contains_key(key);
	let is_assume_role = has("role_arn") && (has("source_profile") != has("credential_source"));
	if !is_assume_role
		&& let (Some(access_key_id), Some(secret_access_key)) =
			(data.get("aws_access_key_id"), data.get("aws_secret_access_key"))
	{
		let identity = AwsCredentialIdentity {
			access_key_id: access_key_id.clone(),
			secret_access_key: secret_access_key.clone(),
			session_token: data.get("aws_session_token").cloned(),
		};
		return Ok((identity, "n"));
	}
	let unsupported = if is_assume_role {
		Some("assume role")
	} else if has("web_identity_token_file") && has("role_arn") {
		Some("web identity")
	} else if has("credential_process") {
		Some("credential process")
	} else if [
		"sso_start_url",
		"sso_account_id",
		"sso_session",
		"sso_region",
		"sso_role_name",
	]
	.iter()
	.any(|key| has(key))
	{
		Some("SSO")
	} else if has("login_session") {
		Some("login")
	} else {
		None
	};
	let message = match unsupported {
		Some(kind) => format!(
			"Could not resolve credentials using profile: [{}] in configuration/credentials file(s): {kind} credentials are not supported by the Rust port.",
			files.profile
		),
		None => "Could not load credentials from any providers".into(),
	};
	Err(PiError::named("CredentialsProviderError", message))
}

// ---------------------------------------------------------------------------------------------------------------
// User agent (`createDefaultUserAgentProvider`, `userAgentMiddleware`)
// ---------------------------------------------------------------------------------------------------------------

/// `escapeUserAgent([name, version])`.
fn escape_user_agent(name: &str, version: Option<&str>) -> String {
	static NAME_ESCAPE: LazyLock<Regex> =
		LazyLock::new(|| Regex::new(r"[^!$%&'*+\-.^_`|~A-Za-z0-9]").expect("valid regex"));
	static VALUE_ESCAPE: LazyLock<Regex> =
		LazyLock::new(|| Regex::new(r"[^!$%&'*+\-.^_`|~A-Za-z0-9#]").expect("valid regex"));
	let name = name
		.split('/')
		.map(|part| NAME_ESCAPE.replace_all(part, "-"))
		.collect::<Vec<_>>()
		.join("/");
	let version = version.map(|version| VALUE_ESCAPE.replace_all(version, "-").into_owned());
	let (prefix, ua_name) = match name.split_once('/') {
		Some((prefix, rest)) => (prefix.to_string(), rest.to_string()),
		None => (String::new(), name.clone()),
	};
	let ua_name = if prefix == "api" {
		ua_name.to_lowercase()
	} else {
		ua_name
	};
	let mut out = String::new();
	let items = [Some(prefix), Some(ua_name), version]
		.into_iter()
		.flatten()
		.filter(|item| !item.is_empty());
	for (index, item) in items.enumerate() {
		out = match index {
			0 => item,
			1 => format!("{out}/{item}"),
			_ => format!("{out}#{item}"),
		};
	}
	out
}

/// `os.platform()` names.
fn platform() -> &'static str {
	match std::env::consts::OS {
		"macos" => "darwin",
		"windows" => "win32",
		other => other,
	}
}

/// The `x-amz-user-agent` and `user-agent` values.
fn user_agent_headers(
	app_id: Option<&str>,
	features: &[&str],
	custom_user_agent: Option<&[(String, Option<String>)]>,
) -> (String, String) {
	let mut sections: Vec<(String, Option<String>)> = vec![
		("aws-sdk-js".into(), Some(SDK_VERSION.into())),
		("ua".into(), Some("2.1".into())),
		(format!("os/{}", platform()), None),
		("lang/rust".into(), None),
		("md/rust".into(), Some(env!("CARGO_PKG_RUST_VERSION").into())),
		("api/Bedrock Runtime".into(), Some(SDK_VERSION.into())),
	];
	if let Some(execution_env) = pi_host::env("AWS_EXECUTION_ENV").filter(|value| !value.is_empty()) {
		sections.push((format!("exec-env/{execution_env}"), None));
	}
	if let Some(app_id) = app_id {
		sections.push((format!("app/{app_id}"), None));
	}
	let mut default_user_agent: Vec<String> = sections
		.iter()
		.map(|(name, version)| escape_user_agent(name, version.as_deref()))
		.collect();
	let mut encoded_features = String::new();
	for feature in features {
		if encoded_features.len() + feature.len() + 1 > 1024 {
			break;
		}
		if !encoded_features.is_empty() {
			encoded_features.push(',');
		}
		encoded_features.push_str(feature);
	}
	default_user_agent.push(format!("m/{encoded_features}"));
	if let Some(app_id) = app_id {
		default_user_agent.push(escape_user_agent("app", Some(app_id)));
	}
	let custom: Vec<String> = custom_user_agent
		.unwrap_or_default()
		.iter()
		.map(|(name, version)| escape_user_agent(name, version.as_deref()))
		.collect();
	let user_agent = default_user_agent
		.iter()
		.chain(&custom)
		.cloned()
		.collect::<Vec<_>>()
		.join(" ");
	let x_amz_user_agent = default_user_agent
		.iter()
		.filter(|section| section.starts_with("aws-sdk-"))
		.chain(&custom)
		.cloned()
		.collect::<Vec<_>>()
		.join(" ");
	(x_amz_user_agent, user_agent)
}

// ---------------------------------------------------------------------------------------------------------------
// Event stream (`getChunkedStream`, `splitMessage`, `getMessageUnmarshaller`, `EventStreamSerde`)
// ---------------------------------------------------------------------------------------------------------------

/// CRC-32 (IEEE), as `Crc32` of `@smithy/core/checksum` computes it.
fn crc32(bytes: &[u8]) -> u32 {
	static TABLE: LazyLock<[u32; 256]> = LazyLock::new(|| {
		let mut table = [0u32; 256];
		for (index, entry) in (0u32..).zip(table.iter_mut()) {
			*entry = (0..8).fold(index, |value, _| {
				if value & 1 == 1 {
					0xEDB8_8320 ^ (value >> 1)
				} else {
					value >> 1
				}
			});
		}
		table
	});
	!bytes
		.iter()
		.fold(!0u32, |crc, byte| TABLE[usize::from((crc as u8) ^ byte)] ^ (crc >> 8))
}

/// `splitMessage(message)`: the length and checksum checks of one framed message.
fn split_message(message: &[u8]) -> PiResult<()> {
	if message.len() < 16 {
		return Err(PiError::new(
			"Provided message too short to accommodate event stream message overhead",
		));
	}
	let read_u32 = |offset: usize| u32::from_be_bytes(message[offset..offset + 4].try_into().expect("four bytes"));
	if read_u32(0) as usize != message.len() {
		return Err(PiError::new(
			"Reported message length does not match received message length",
		));
	}
	let expected_prelude_checksum = read_u32(8);
	let prelude_checksum = crc32(&message[..8]);
	if expected_prelude_checksum != prelude_checksum {
		return Err(PiError::new(format!(
			"The prelude checksum specified in the message ({expected_prelude_checksum}) does not match the calculated CRC32 checksum ({prelude_checksum})"
		)));
	}
	let expected_message_checksum = read_u32(message.len() - 4);
	let message_checksum = crc32(&message[..message.len() - 4]);
	if expected_message_checksum != message_checksum {
		return Err(PiError::new(format!(
			"The message checksum ({message_checksum}) did not match the expected value of {expected_message_checksum}"
		)));
	}
	Ok(())
}

/// Frames a response body into event stream messages, like `getChunkedStream`.
struct MessageReader {
	body: Body,
	buffer: Vec<u8>,
	signal: Option<AbortSignal>,
}

impl MessageReader {
	/// The next message, `None` at the end of the body. An abort ends the body like the default HTTP/2 handler, which
	/// closes the stream without an error.
	async fn next_message(&mut self) -> PiResult<Option<Message>> {
		loop {
			if self.buffer.len() >= 4 {
				let size = u32::from_be_bytes(self.buffer[..4].try_into().expect("four bytes")) as usize;
				if size < 4 {
					return Err(PiError::named(
						"RangeError",
						"Offset is outside the bounds of the DataView",
					));
				}
				if self.buffer.len() >= size {
					let bytes: Vec<u8> = self.buffer.drain(..size).collect();
					split_message(&bytes)?;
					let message = read_message_from(&bytes[..]).map_err(|error| PiError::new(error.to_string()))?;
					return Ok(Some(message));
				}
			}
			match self.body.next().await {
				Some(Ok(chunk)) => self.buffer.extend_from_slice(&chunk),
				Some(Err(_)) if self.signal.as_ref().is_some_and(AbortSignal::aborted) => return Ok(None),
				Some(Err(error)) => return Err(error),
				// A length prefix without its message is dropped; a started message is truncated.
				None if self.buffer.len() < 4 => return Ok(None),
				None => return Err(PiError::new("Truncated event message received.")),
			}
		}
	}

	/// The next known event of the stream as `{ [member]: value }`; unknown event types are skipped.
	async fn next_event(&mut self) -> PiResult<Option<Value>> {
		while let Some(message) = self.next_message().await? {
			if let Some(event) = unmarshal_message(&message)? {
				return Ok(Some(event));
			}
		}
		Ok(None)
	}
}

/// `message.headers[name].value` of a string header; reading a missing header is the JS `TypeError`.
fn message_header(message: &Message, name: &str) -> PiResult<String> {
	message
		.headers()
		.iter()
		.find(|header| header.name().as_str() == name)
		.and_then(|header| header.value().as_string().ok())
		.map(|value| value.as_str().to_string())
		.ok_or_else(|| PiError::type_error("Cannot read properties of undefined (reading 'value')"))
}

/// `getMessageUnmarshaller(deserializer)(message)`.
fn unmarshal_message(message: &Message) -> PiResult<Option<Value>> {
	let message_type = message_header(message, ":message-type").map_err(|_| {
		PiError::type_error(
			"Cannot destructure property 'value' of 'message.headers[\":message-type\"]' as it is undefined.",
		)
	})?;
	let payload = message.payload();
	match message_type.as_str() {
		"error" => {
			let error_message = message_header(message, ":error-message")?;
			let code = message_header(message, ":error-code")?;
			let error_message = if error_message.is_empty() {
				"UnknownError".into()
			} else {
				error_message
			};
			Err(PiError::named(code, error_message))
		}
		"exception" => {
			let code = message_header(message, ":exception-type")?;
			match read_event_member(&code, payload)? {
				Some((shape_name, value)) => Err(service_exception(shape_name, &value)),
				None => Err(PiError::named(code, String::from_utf8_lossy(payload))),
			}
		}
		"event" => {
			let event_type = message_header(message, ":event-type")?;
			Ok(read_event_member(&event_type, payload)?
				.map(|(_, value)| Value::Object(Map::from_iter([(event_type, value)]))))
		}
		_ => Err(PiError::new(format!(
			"Unrecognizable event type: {}",
			message_header(message, ":event-type")?
		))),
	}
}

/// `EventStreamSerde.readEventMember` for a member of `ConverseStreamOutput`: its shape name and value, `None` for a
/// member this SDK version does not know. Error members become the exception's own properties.
fn read_event_member(member: &str, payload: &[u8]) -> PiResult<Option<(&'static str, Value)>> {
	let Some(&(_, shape_name)) = shape("ConverseStreamOutput")
		.members
		.iter()
		.find(|(name, _)| *name == member)
	else {
		return Ok(None);
	};
	let data = if payload.is_empty() {
		Value::Object(Map::new())
	} else {
		read_json(ty(shape_name), payload)?
	};
	let Some(fault) = error_fault(shape_name) else {
		return Ok(Some((shape_name, data)));
	};
	let data = match data {
		Value::Object(data) => data,
		_ => Map::new(),
	};
	let message = data
		.get("message")
		.or_else(|| data.get("Message"))
		.cloned()
		.unwrap_or_else(|| "Unknown".into());
	let mut exception = Map::new();
	exception.insert("$fault".into(), fault.into());
	exception.insert("message".into(), message);
	exception.extend(data);
	Ok(Some((shape_name, Value::Object(exception))))
}

/// The `error` trait of an error shape.
fn error_fault(shape_name: &str) -> Option<&'static str> {
	ERROR_SHAPES
		.iter()
		.find(|(name, _)| *name == shape_name)
		.map(|(_, fault)| *fault)
}

/// A modeled exception (an instance of a `BedrockRuntimeServiceException` subclass) from its properties.
fn service_exception(name: &str, properties: &Value) -> PiError {
	let message = properties.get("message").map(js_string).unwrap_or_default();
	PiError::named(name, message)
		.with_data(json!({ "$fault": properties.get("$fault").cloned().unwrap_or(Value::Null) }))
}

/// The error a stream item of an exception member (`{ throttlingException: {...} }`) stands for, as `throw
/// item.throttlingException` throws it.
pub fn stream_item_exception(member: &str, properties: &Value) -> Option<PiError> {
	let &(_, shape_name) = shape("ConverseStreamOutput")
		.members
		.iter()
		.find(|(name, _)| *name == member)?;
	error_fault(shape_name)?;
	Some(service_exception(shape_name, properties))
}

/// `error instanceof BedrockRuntimeServiceException`: the SDK's service exceptions are the errors with a `$fault`.
pub fn is_service_exception(error: &PiError) -> bool {
	error.data.as_ref().is_some_and(|data| data.get("$fault").is_some())
}

// ---------------------------------------------------------------------------------------------------------------
// Errors (`AwsRestJsonProtocol.handleError`, `schemaDeserializationMiddleware`)
// ---------------------------------------------------------------------------------------------------------------

/// `$metadata` (`ResponseMetadata`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResponseMetadata {
	pub http_status_code: Option<u16>,
	pub request_id: Option<String>,
	pub extended_request_id: Option<String>,
	pub cf_id: Option<String>,
	pub attempts: Option<f64>,
	pub total_retry_delay: Option<f64>,
}

impl ResponseMetadata {
	/// `deserializeMetadata(output)`.
	fn from_response(status: u16, headers: &IndexMap<String, String>) -> Self {
		let header = |name: &str| headers.get(name).cloned();
		Self {
			http_status_code: Some(status),
			request_id: header("x-amzn-requestid")
				.or_else(|| header("x-amzn-request-id"))
				.or_else(|| header("x-amz-request-id")),
			extended_request_id: header("x-amz-id-2"),
			cf_id: header("x-amz-cf-id"),
			..Self::default()
		}
	}

	fn to_json(&self) -> Map<String, Value> {
		let mut out = Map::new();
		let mut put = |key: &str, value: Option<Value>| {
			if let Some(value) = value {
				out.insert(key.into(), value);
			}
		};
		put("httpStatusCode", self.http_status_code.map(Value::from));
		put("requestId", self.request_id.clone().map(Value::from));
		put("extendedRequestId", self.extended_request_id.clone().map(Value::from));
		put("cfId", self.cf_id.clone().map(Value::from));
		put("attempts", self.attempts.map(Value::from));
		put("totalRetryDelay", self.total_retry_delay.map(Value::from));
		out
	}
}

/// `error.$metadata` of a thrown error, as JSON.
fn error_metadata(error: &mut PiError) -> &mut Map<String, Value> {
	let data = error.data.get_or_insert_with(|| Value::Object(Map::new()));
	if !data.is_object() {
		*data = Value::Object(Map::new());
	}
	let data = data.as_object_mut().expect("data was made an object");
	let metadata = data.entry("$metadata").or_insert_with(|| Value::Object(Map::new()));
	if !metadata.is_object() {
		*metadata = Value::Object(Map::new());
	}
	metadata.as_object_mut().expect("$metadata was made an object")
}

/// `value || undefined` as a string, for the `message || Message || "UnknownError"` fallbacks.
fn truthy_string(value: Option<&Value>) -> Option<String> {
	match value? {
		Value::Null | Value::Bool(false) => None,
		Value::String(text) if text.is_empty() => None,
		Value::Number(number) if number.as_f64().is_none_or(|number| number == 0.0) => None,
		value => Some(js_string(value)),
	}
}

/// `loadRestJsonErrorCode(output, data)`.
fn load_rest_json_error_code(headers: &IndexMap<String, String>, data: &Map<String, Value>) -> Option<String> {
	let sanitize = |raw: String| {
		let mut clean = raw;
		if let Some((first, _)) = clean.split_once(',') {
			clean = first.to_string();
		}
		if let Some((first, _)) = clean.split_once(':') {
			clean = first.to_string();
		}
		if clean.contains('#') {
			clean = clean.split('#').nth(1).unwrap_or_default().to_string();
		}
		clean
	};
	if let Some((_, value)) = headers
		.iter()
		.find(|(name, _)| name.eq_ignore_ascii_case("x-amzn-errortype"))
	{
		return Some(sanitize(value.clone()));
	}
	if let Some((_, value)) = data.iter().find(|(key, _)| key.to_lowercase() == "code") {
		return Some(sanitize(js_string(value)));
	}
	data.get("__type").map(|value| sanitize(js_string(value)))
}

/// The exception `handleError` throws for a non-2xx response whose body parsed as JSON.
fn http_exception(status: u16, headers: &IndexMap<String, String>, data: &Value) -> PiError {
	let empty = Map::new();
	let data = data.as_object().unwrap_or(&empty);
	let error_name = load_rest_json_error_code(headers, data).unwrap_or_else(|| "Unknown".into());
	let fault = error_fault(&error_name).unwrap_or(if status < 500 { "client" } else { "server" });
	let message = truthy_string(data.get("message"))
		.or_else(|| truthy_string(data.get("Message")))
		.unwrap_or_else(|| "UnknownError".into());
	PiError::named(error_name, message).with_data(json!({
		"$fault": fault,
		"$metadata": Value::Object(ResponseMetadata::from_response(status, headers).to_json()),
	}))
}

/// The `catch` of `schemaDeserializationMiddleware`: an error without `$metadata` gets the hint and the response's
/// metadata.
fn decorate_deserialization_error(mut error: PiError, status: u16, headers: &IndexMap<String, String>) -> PiError {
	if is_service_exception(&error) {
		return error;
	}
	error.message.push_str("\n  ");
	error.message.push_str(DESERIALIZATION_HINT);
	let find_header = |pattern: &str| {
		let pattern = Regex::new(pattern).expect("valid regex");
		headers
			.iter()
			.find(|(name, _)| pattern.is_match(name))
			.map(|(_, value)| value.clone())
	};
	let metadata = ResponseMetadata {
		http_status_code: Some(status),
		request_id: find_header(r"^x-[A-Za-z0-9_-]+-request-?id$"),
		extended_request_id: find_header(r"^x-[A-Za-z0-9_-]+-id-2$"),
		cf_id: find_header(r"^x-[A-Za-z0-9_-]+-cf-id$"),
		..ResponseMetadata::default()
	};
	*error_metadata(&mut error) = metadata.to_json();
	error
}

/// The `AbortError` the HTTP handler rejects with.
fn request_aborted() -> PiError {
	PiError::named("AbortError", "Request aborted")
}

// ---------------------------------------------------------------------------------------------------------------
// Client (`BedrockRuntimeClient.send(new ConverseStreamCommand(input))`)
// ---------------------------------------------------------------------------------------------------------------

/// The `BedrockRuntimeClientConfig` fields pi-ai sets.
#[derive(Clone, Debug, Default)]
pub struct BedrockRuntimeClientConfig {
	pub region: Option<String>,
	pub profile: Option<String>,
	pub endpoint: Option<String>,
	pub credentials: Option<AwsCredentialIdentity>,
	/// `token: { token }`.
	pub token: Option<String>,
	/// Auth scheme names without their namespace (`httpBearerAuth`, `sigv4`).
	pub auth_scheme_preference: Option<Vec<String>>,
	/// `customUserAgent` as `[name, version?]` pairs (a string is one pair without a version).
	pub custom_user_agent: Option<Vec<(String, Option<String>)>>,
}

/// A `build`-step middleware: edits the request headers after the SDK's own build middleware.
pub type BuildMiddleware = Rc<dyn Fn(&mut IndexMap<String, String>)>;

/// A `deserialize`-step middleware below the SDK deserializer: sees every raw HTTP response, error responses included.
pub type DeserializeMiddleware = Rc<dyn Fn(ProviderResponse) -> LocalBoxFuture<'static, PiResult<()>>>;

/// `ConverseStreamCommandOutput`: `$metadata`, and `stream` with its first event already read.
pub struct ConverseStreamOutput {
	pub metadata: ResponseMetadata,
	pub stream: LocalBoxStream<'static, PiResult<Value>>,
}

/// `BedrockRuntimeClient` with the `ConverseStream` operation and a middleware stack reduced to the two hooks
/// pi-ai adds.
pub struct BedrockRuntimeClient {
	config: BedrockRuntimeClientConfig,
	build_middleware: Vec<BuildMiddleware>,
	deserialize_middleware: Vec<DeserializeMiddleware>,
	/// `config.systemClockOffset`, corrected from response `Date` headers.
	system_clock_offset: Cell<f64>,
	/// `signingProperties._preRequestSystemClockOffset` of the last SigV4 signing.
	pre_request_offset: Cell<f64>,
	/// `signingProperties._requestSentAt` of the last SigV4 signing.
	request_sent_at: Cell<f64>,
}

/// A failed attempt, with the response headers when the error came from deserializing a response (`$response`).
struct AttemptError {
	error: PiError,
	response_headers: Option<IndexMap<String, String>>,
}

impl From<PiError> for AttemptError {
	fn from(error: PiError) -> Self {
		Self {
			error,
			response_headers: None,
		}
	}
}

#[derive(Clone, Copy, PartialEq)]
enum RetryErrorType {
	Throttling,
	Transient,
	Other,
}

/// `THROTTLING_ERROR_CODES`.
const THROTTLING_ERROR_CODES: [&str; 14] = [
	"BandwidthLimitExceeded",
	"EC2ThrottledException",
	"LimitExceededException",
	"PriorRequestNotComplete",
	"ProvisionedThroughputExceededException",
	"RequestLimitExceeded",
	"RequestThrottled",
	"RequestThrottledException",
	"SlowDown",
	"ThrottledException",
	"Throttling",
	"ThrottlingException",
	"TooManyRequestsException",
	"TransactionInProgressException",
];

impl BedrockRuntimeClient {
	/// `new BedrockRuntimeClient(config)`.
	pub fn new(config: BedrockRuntimeClientConfig) -> Self {
		Self {
			config,
			build_middleware: Vec::new(),
			deserialize_middleware: Vec::new(),
			system_clock_offset: Cell::new(0.0),
			pre_request_offset: Cell::new(0.0),
			request_sent_at: Cell::new(0.0),
		}
	}

	/// `middlewareStack.add(middleware, { step: "build", priority: "low" })`.
	pub fn add_build_middleware(&mut self, middleware: BuildMiddleware) {
		self.build_middleware.push(middleware);
	}

	/// `middlewareStack.add(middleware, { step: "deserialize" })`.
	pub fn add_deserialize_middleware(&mut self, middleware: DeserializeMiddleware) {
		self.deserialize_middleware.push(middleware);
	}

	/// `client.send(new ConverseStreamCommand(input), { abortSignal })`.
	pub async fn converse_stream(&self, input: &Value, signal: Option<&AbortSignal>) -> PiResult<ConverseStreamOutput> {
		let config = &self.config;
		let files = SharedConfigFiles::load(config.profile.as_deref()).await;
		let provided_region = match &config.region {
			Some(region) => region.clone(),
			None => files
				.setting("AWS_REGION", &["region"], PreferredFile::Credentials)
				.ok_or_else(|| PiError::new("Region is missing"))?,
		};
		let region = get_real_region(&provided_region);
		if !is_valid_host_label(&region) && region != "*" {
			return Err(PiError::new(format!(
				"Region not accepted: region=\"{region}\" is not a valid hostname component."
			)));
		}
		let identity = self.resolve_identity(&files)?;
		let use_fips =
			is_fips_region(&provided_region) || files.boolean_setting("AWS_USE_FIPS_ENDPOINT", "use_fips_endpoint");
		let use_dual_stack = files.boolean_setting("AWS_USE_DUALSTACK_ENDPOINT", "use_dualstack_endpoint");
		let endpoint = resolve_endpoint(config.endpoint.as_deref(), &region, use_fips, use_dual_stack)?;
		let serialized = serialize_converse_stream(input, endpoint.path())?;

		let app_id = files
			.setting(
				"AWS_SDK_UA_APP_ID",
				&["sdk_ua_app_id", "sdk-ua-app-id"],
				PreferredFile::Config,
			)
			.filter(|app_id| !app_id.is_empty());
		let mut features = Vec::new();
		if config.endpoint.is_some() {
			features.push("N");
		}
		features.push("E");
		if let Identity::SigV4(_, Some(feature)) | Identity::Bearer(_, Some(feature)) = &identity {
			features.push(feature);
		}
		let (x_amz_user_agent, user_agent) =
			user_agent_headers(app_id.as_deref(), &features, config.custom_user_agent.as_deref());
		let host = match endpoint.port() {
			Some(port) => format!("{}:{port}", endpoint.host_str().unwrap_or_default()),
			None => endpoint.host_str().unwrap_or_default().to_string(),
		};
		let mut headers = IndexMap::new();
		headers.insert("content-type".to_string(), "application/json".to_string());
		headers.insert("content-length".to_string(), serialized.body.len().to_string());
		headers.insert("x-amz-user-agent".to_string(), x_amz_user_agent);
		headers.insert("user-agent".to_string(), user_agent);
		headers.insert("host".to_string(), host.clone());
		// `recursionDetectionMiddleware`, which the client adds before pi's build middleware.
		if let (Some(_), Some(trace_id)) = (
			pi_host::env("AWS_LAMBDA_FUNCTION_NAME").filter(|name| !name.is_empty()),
			pi_host::env("_X_AMZN_TRACE_ID").filter(|trace_id| !trace_id.is_empty()),
		) {
			headers.insert("X-Amzn-Trace-Id".to_string(), trace_id);
		}
		for middleware in &self.build_middleware {
			middleware(&mut headers);
		}
		// The headers are a plain object that the signer and Node's `setHeader` both read case-insensitively: a later
		// name replaces an earlier one with another case, in the earlier one's place.
		let mut merged: Vec<(String, String)> = Vec::new();
		for (name, value) in headers {
			match merged
				.iter_mut()
				.find(|(existing, _)| existing.eq_ignore_ascii_case(&name))
			{
				Some(entry) => *entry = (name, value),
				None => merged.push((name, value)),
			}
		}
		let mut headers: IndexMap<String, String> = merged.into_iter().collect();
		let url = format!("{}://{host}{}", endpoint.scheme(), serialized.path);
		let body = Bytes::from(serialized.body);

		let max_attempts = files.max_attempts();
		let new_retries = pi_host::env("SMITHY_NEW_RETRIES_2026").as_deref() == Some("true");
		headers.insert("amz-sdk-invocation-id".to_string(), uuid_v4());
		let mut retry_count = 0.0;
		let mut total_retry_delay = 0.0;
		loop {
			headers.insert(
				"amz-sdk-request".to_string(),
				format!("attempt={}; max={max_attempts}", retry_count + 1.0),
			);
			let failure = match self.attempt(&headers, &url, &body, &identity, &region, signal).await {
				Ok(mut output) => {
					output.metadata.attempts = Some(retry_count + 1.0);
					output.metadata.total_retry_delay = Some(total_retry_delay);
					return Ok(output);
				}
				Err(failure) => failure,
			};
			let mut error = failure.error;
			let error_type = retry_error_type(&error);
			if error_type == RetryErrorType::Other || retry_count + 1.0 >= max_attempts {
				let metadata = error_metadata(&mut error);
				metadata.insert("attempts".into(), Value::from(retry_count + 1.0));
				metadata.insert("totalRetryDelay".into(), Value::from(total_retry_delay));
				return Err(error);
			}
			let base = match (error_type, new_retries) {
				(RetryErrorType::Throttling, false) => 500.0,
				(RetryErrorType::Throttling, true) => 1000.0,
				(_, false) => 100.0,
				(_, true) => 50.0,
			};
			let mut delay = (rand::random::<f64>() * f64::min(base * 2f64.powf(retry_count), 20000.0)).floor();
			if let Some(hint) = failure.response_headers.as_ref().and_then(retry_after_hint) {
				delay = f64::max(delay, f64::min(hint - pi_host::now_ms(), delay + 5000.0));
			}
			let _ = pi_host::sleep(delay, None).await;
			total_retry_delay += delay;
			retry_count += 1.0;
		}
	}

	/// The identity of the first auth scheme in `authSchemePreference` order (`httpAuthSchemeMiddleware`).
	fn resolve_identity(&self, files: &SharedConfigFiles) -> PiResult<Identity> {
		let config = &self.config;
		let preference = match &config.auth_scheme_preference {
			Some(preference) => preference.clone(),
			None if pi_host::env("AWS_BEARER_TOKEN_BEDROCK").is_some() => vec!["httpBearerAuth".to_string()],
			None => files
				.setting(
					"AWS_AUTH_SCHEME_PREFERENCE",
					&["auth_scheme_preference"],
					PreferredFile::Config,
				)
				.map(|setting| comma_separated(&setting))
				.unwrap_or_default(),
		};
		let scheme = preference
			.iter()
			.find_map(|name| AUTH_SCHEMES.iter().find(|scheme| **scheme == name.as_str()))
			.copied()
			.unwrap_or(AUTH_SCHEMES[0]);
		if scheme == "httpBearerAuth" {
			return match (&config.token, pi_host::env("AWS_BEARER_TOKEN_BEDROCK")) {
				(Some(token), _) => Ok(Identity::Bearer(token.clone(), None)),
				(None, Some(token)) => Ok(Identity::Bearer(token, Some("3"))),
				(None, None) => Err(PiError::named(
					"TokenProviderError",
					"Could not load token from any providers",
				)),
			};
		}
		if let Some(credentials) = &config.credentials {
			return Ok(Identity::SigV4(credentials.clone(), Some("e")));
		}
		let profile_set = config.profile.as_deref().is_some_and(|profile| !profile.is_empty())
			|| pi_host::env("AWS_PROFILE").is_some_and(|profile| !profile.is_empty());
		let (credentials, feature) = default_credentials(files, profile_set)?;
		Ok(Identity::SigV4(credentials, Some(feature)))
	}

	/// One attempt: sign, send, run the deserialize middleware, and deserialize the response up to its first event.
	async fn attempt(
		&self,
		headers: &IndexMap<String, String>,
		url: &str,
		body: &Bytes,
		identity: &Identity,
		region: &str,
		signal: Option<&AbortSignal>,
	) -> Result<ConverseStreamOutput, AttemptError> {
		let mut request = HttpRequest::new("POST", url);
		request.headers = self.sign(headers, url, body, identity, region)?;
		// The default HTTP/2 handler sends `host` as `:authority`, which the host's fetch derives from the URL.
		request.headers.shift_remove("host");
		request.body = Some(body.clone());
		request.signal = signal.cloned();
		if signal.is_some_and(AbortSignal::aborted) {
			return Err(request_aborted().into());
		}
		let response = pi_host::fetch(request, None).await.map_err(|error| {
			if signal.is_some_and(AbortSignal::aborted) {
				request_aborted()
			} else {
				error
			}
		})?;
		let status = response.status;
		let response_headers = response.headers.clone();
		for middleware in &self.deserialize_middleware {
			middleware(ProviderResponse {
				status,
				headers: response_headers.clone(),
			})
			.await?;
		}
		let result = self.deserialize(response, signal).await;
		let sigv4 = matches!(identity, Identity::SigV4(..));
		let skew_corrected = sigv4 && self.correct_clock_skew(&response_headers);
		result.map_err(|mut error| {
			if skew_corrected && error.data.as_ref().is_some_and(|data| data.get("$metadata").is_some()) {
				error_metadata(&mut error).insert("clockSkewCorrected".into(), Value::Bool(true));
			}
			AttemptError {
				error,
				response_headers: Some(response_headers),
			}
		})
	}

	/// `schemaDeserializationMiddleware` with `deserializeResponse` of the event stream output.
	async fn deserialize(
		&self,
		response: HttpResponse,
		signal: Option<&AbortSignal>,
	) -> PiResult<ConverseStreamOutput> {
		let status = response.status;
		let headers = response.headers.clone();
		let decorate = |error: PiError| decorate_deserialization_error(error, status, &headers);
		if !(200..300).contains(&status) {
			let bytes = response.bytes().await.map_err(decorate)?;
			let data = read_json(Ty::Document, &bytes).map_err(decorate)?;
			return Err(http_exception(status, &headers, &data));
		}
		let mut reader = MessageReader {
			body: response.body,
			buffer: Vec::new(),
			signal: signal.cloned(),
		};
		let first = reader.next_event().await.map_err(decorate)?;
		let stream = match first {
			None => futures::stream::empty().boxed_local(),
			Some(first) => futures::stream::unfold(Some((reader, Some(first))), |state| async move {
				let (mut reader, pending) = state?;
				if let Some(event) = pending {
					return Some((Ok(event), Some((reader, None))));
				}
				match reader.next_event().await {
					Ok(Some(event)) => Some((Ok(event), Some((reader, None)))),
					Ok(None) => None,
					Err(error) => Some((Err(error), None)),
				}
			})
			.boxed_local(),
		};
		Ok(ConverseStreamOutput {
			metadata: ResponseMetadata::from_response(status, &headers),
			stream,
		})
	}

	/// `AwsSdkSigV4Signer`'s `successHandler` and `errorHandler`: a non-empty `Date` header sets `systemClockOffset`
	/// to `getUpdatedSystemClockOffset`. Returns whether an error would be marked `clockSkewCorrected`: the new offset
	/// is at least 240 seconds and differs from the offset the request was signed with.
	fn correct_clock_skew(&self, headers: &IndexMap<String, String>) -> bool {
		let Some(date) = headers.get("date").filter(|date| !date.is_empty()) else {
			return false;
		};
		let offset = self.updated_system_clock_offset(date, headers.contains_key("age"));
		self.system_clock_offset.set(offset);
		offset.abs() >= 240000.0 && offset != self.pre_request_offset.get()
	}

	/// `getUpdatedSystemClockOffset(clockTime, currentSystemClockOffset, timeRequestSent, ageHeader)`: the server time
	/// minus the midpoint of sending and receiving, unless the response has an `Age` header or took over 15 minutes.
	fn updated_system_clock_offset(&self, clock_time: &str, has_age_header: bool) -> f64 {
		let current = self.system_clock_offset.get();
		if has_age_header {
			return current;
		}
		let server_time = date_parse(clock_time);
		let time_response_received = pi_host::now_ms();
		let time_request_sent = self.request_sent_at.get();
		if time_response_received - time_request_sent > 900000.0 {
			return current;
		}
		server_time - (time_request_sent + time_response_received) / 2.0
	}

	/// `HttpBearerAuthSigner.sign` or `AwsSdkSigV4Signer.sign` of the request headers.
	fn sign(
		&self,
		headers: &IndexMap<String, String>,
		url: &str,
		body: &[u8],
		identity: &Identity,
		region: &str,
	) -> PiResult<IndexMap<String, String>> {
		let credentials = match identity {
			Identity::Bearer(token, _) => {
				let mut signed = headers.clone();
				signed.insert("Authorization".into(), format!("Bearer {token}"));
				return Ok(signed);
			}
			Identity::SigV4(credentials, _) => credentials,
		};
		let mut signed = headers.clone();
		signed.retain(|name, _| !matches!(name.to_lowercase().as_str(), "authorization" | "x-amz-date" | "date"));
		// `getCanonicalHeaders` signs `value.trim().replace(/\s+/g, " ")`; aws-sigv4 collapses only spaces.
		let signable: Vec<(&str, String)> = signed
			.iter()
			.filter(|(name, _)| {
				let name = name.to_lowercase();
				!UNSIGNABLE_HEADERS.contains(&name.as_str()) && !name.starts_with("proxy-") && !name.starts_with("sec-")
			})
			.map(|(name, value)| {
				let words: Vec<&str> = value.split(is_js_whitespace).filter(|word| !word.is_empty()).collect();
				(name.as_str(), words.join(" "))
			})
			.collect();
		let signing_error =
			|error: &dyn std::fmt::Display| PiError::new(format!("Failed to sign the request: {error}"));
		let identity = Credentials::new(
			credentials.access_key_id.clone(),
			credentials.secret_access_key.clone(),
			credentials.session_token.clone(),
			None,
			"pi",
		)
		.into();
		let mut settings = SigningSettings::default();
		settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
		settings.excluded_headers = Some(Vec::new());
		let now = pi_host::now_ms();
		self.pre_request_offset.set(self.system_clock_offset.get());
		self.request_sent_at.set(now);
		// `getSkewCorrectedDate` makes an Invalid Date from a NaN offset, whose `toISOString` throws while signing.
		let signing_ms = (now + self.system_clock_offset.get()).trunc();
		if signing_ms.is_nan() || signing_ms.abs() > 8.64e15 {
			return Err(PiError::named("RangeError", "Invalid time value"));
		}
		let signing_time = if signing_ms >= 0.0 {
			UNIX_EPOCH + Duration::from_millis(signing_ms as u64)
		} else {
			UNIX_EPOCH - Duration::from_millis(-signing_ms as u64)
		};
		let params = v4::SigningParams::builder()
			.identity(&identity)
			.region(region)
			.name(SIGNING_NAME)
			.time(signing_time)
			.settings(settings)
			.build()
			.map_err(|error| signing_error(&error))?
			.into();
		let request = SignableRequest::new(
			"POST",
			url,
			signable.iter().map(|(name, value)| (*name, value.as_str())),
			SignableBody::Bytes(body),
		)
		.map_err(|error| signing_error(&error))?;
		let (instructions, _) = sign(request, &params)
			.map_err(|error| signing_error(&error))?
			.into_parts();
		let (signing_headers, _) = instructions.into_parts();
		for name in [
			"x-amz-date",
			"x-amz-security-token",
			"x-amz-content-sha256",
			"authorization",
		] {
			if let Some(header) = signing_headers.iter().find(|header| header.name() == name) {
				signed.insert(name.to_string(), header.value().to_string());
			}
		}
		Ok(signed)
	}
}

/// JavaScript `\s`: white space and line terminators, which include U+FEFF but not U+0085.
fn is_js_whitespace(ch: char) -> bool {
	(ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}'
}

/// `ALWAYS_UNSIGNABLE_HEADERS` of `@smithy/signature-v4`.
const UNSIGNABLE_HEADERS: [&str; 15] = [
	"authorization",
	"cache-control",
	"connection",
	"expect",
	"from",
	"keep-alive",
	"max-forwards",
	"pragma",
	"referer",
	"te",
	"trailer",
	"transfer-encoding",
	"upgrade",
	"user-agent",
	"x-amzn-trace-id",
];

/// `getRetryErrorType(error)`.
/// `TRANSIENT_ERROR_CODES`.
const TRANSIENT_ERROR_CODES: [&str; 3] = ["TimeoutError", "RequestTimeout", "RequestTimeoutException"];

/// `NODEJS_TIMEOUT_ERROR_CODES` and `NODEJS_NETWORK_ERROR_CODES`.
const NODEJS_ERROR_CODES: [&str; 8] = [
	"ECONNRESET",
	"ECONNREFUSED",
	"EPIPE",
	"ETIMEDOUT",
	"EHOSTUNREACH",
	"ENETUNREACH",
	"ENOTFOUND",
	"EAI_AGAIN",
];

fn retry_error_type(error: &PiError) -> RetryErrorType {
	let status = error
		.data
		.as_ref()
		.and_then(|data| data.get("$metadata"))
		.and_then(|metadata| metadata.get("httpStatusCode"))
		.and_then(Value::as_f64);
	if status == Some(429.0) || THROTTLING_ERROR_CODES.contains(&error.name.as_str()) {
		return RetryErrorType::Throttling;
	}
	let clock_skew_corrected = error
		.data
		.as_ref()
		.and_then(|data| data.get("$metadata"))
		.and_then(|metadata| metadata.get("clockSkewCorrected"))
		.is_some_and(|corrected| corrected == &Value::Bool(true));
	let transient = (is_service_exception(error) && error.name == "ModelNotReadyException")
		|| clock_skew_corrected
		|| (error.name == "InvalidSignatureException" && error.message.contains("Signature expired"))
		|| TRANSIENT_ERROR_CODES.contains(&error.name.as_str())
		|| is_host_transport_failure(error)
		|| has_transient_code(error.data.as_ref(), 0)
		|| status.is_some_and(|status| [500.0, 502.0, 503.0, 504.0].contains(&status));
	if transient {
		RetryErrorType::Transient
	} else {
		RetryErrorType::Other
	}
}

/// `parseRetryAfterHeader(response)` as an epoch time in milliseconds.
/// The host's `TypeError: fetch failed` for a request that got no response. Node's HTTP/2 handler fails such a
/// request with a socket error whose `code` the SDK reads as transient; the host only gives the cause as text, so
/// every failure to get a response counts as one.
fn is_host_transport_failure(error: &PiError) -> bool {
	error.name == "TypeError"
		&& error.message == "fetch failed"
		&& error.data.as_ref().is_none_or(|data| data.get("$metadata").is_none())
}

/// `isTransientError`'s `error.code` and `error.name` checks down the `cause` chain, ten levels deep like the SDK.
fn has_transient_code(data: Option<&Value>, depth: usize) -> bool {
	let Some(data) = data else {
		return false;
	};
	let code = data.get("code").and_then(Value::as_str).unwrap_or("");
	let cause = data.get("cause");
	let cause_name = cause
		.and_then(|cause| cause.get("name"))
		.and_then(Value::as_str)
		.unwrap_or("");
	NODEJS_ERROR_CODES.contains(&code)
		|| (depth <= 10 && (TRANSIENT_ERROR_CODES.contains(&cause_name) || has_transient_code(cause, depth + 1)))
}

fn retry_after_hint(headers: &IndexMap<String, String>) -> Option<f64> {
	static SECONDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^((\d+)|(\d+\.\d+))$").expect("valid regex"));
	static GMT_SECONDS: LazyLock<Regex> =
		LazyLock::new(|| Regex::new(r" GMT, ((\d+)|(\d+\.\d+))$").expect("valid regex"));
	for (name, value) in headers {
		match name.to_lowercase().as_str() {
			"retry-after" => {
				let now = pi_host::now_ms();
				let seconds = if value.ends_with("GMT") {
					(date_parse(value) - now) / 1000.0
				} else if let Some(captures) = GMT_SECONDS.captures(value) {
					string_to_number(&captures[1])
				} else if SECONDS.is_match(value) {
					string_to_number(value)
				} else if date_parse(value) >= now {
					(date_parse(value) - now) / 1000.0
				} else {
					f64::NAN
				};
				return (!seconds.is_nan()).then(|| (now + seconds * 1000.0).trunc());
			}
			"x-amz-retry-after" => {
				let milliseconds = string_to_number(value);
				return (!milliseconds.is_nan()).then(|| (pi_host::now_ms() + milliseconds).trunc());
			}
			_ => {}
		}
	}
	None
}

/// `v4()`: a random UUID.
fn uuid_v4() -> String {
	let mut bytes: [u8; 16] = rand::random();
	bytes[6] = (bytes[6] & 0x0f) | 0x40;
	bytes[8] = (bytes[8] & 0x3f) | 0x80;
	let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
	format!(
		"{}-{}-{}-{}-{}",
		&hex[..8],
		&hex[8..12],
		&hex[12..16],
		&hex[16..20],
		&hex[20..]
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	const BODY: &str = r#"{"messages":[{"role":"user","content":[{"text":"hello"},{"cachePoint":{"type":"default"}}]}],"system":[{"text":"Be brief."},{"cachePoint":{"type":"default"}}],"inferenceConfig":{"maxTokens":128000,"temperature":0.5}}"#;

	#[test]
	fn serializes_converse_stream_requests_in_schema_order() {
		let input = json!({
			"modelId": "us.anthropic.claude-opus-4-8",
			"messages": [{ "role": "user", "content": [{ "text": "hello" }, { "cachePoint": { "type": "default" } }] }],
			"system": [{ "text": "Be brief." }, { "cachePoint": { "type": "default" } }],
			"inferenceConfig": { "temperature": 0.5, "maxTokens": 128000 },
			"toolConfig": null,
		});
		let serialized = serialize_converse_stream(&input, "/").expect("serializes");
		assert_eq!(serialized.path, "/model/us.anthropic.claude-opus-4-8/converse-stream");
		assert_eq!(String::from_utf8(serialized.body).expect("utf-8"), BODY);
	}

	#[test]
	fn signs_like_the_sdk() {
		pi_host::testing::run(|host| async move {
			host.advance(1794052800000.0 - pi_host::now_ms());
			let client = BedrockRuntimeClient::new(BedrockRuntimeClientConfig::default());
			let signed = sign_fixture(&client, "v");
			let names: Vec<&str> = signed.keys().map(String::as_str).collect();
			assert_eq!(
				names,
				[
					"content-type",
					"content-length",
					"x-amz-user-agent",
					"user-agent",
					"host",
					"x-custom",
					"amz-sdk-invocation-id",
					"amz-sdk-request",
					"x-amz-date",
					"x-amz-content-sha256",
					"authorization"
				]
			);
			assert_eq!(signed["x-amz-date"], "20261107T120000Z");
			assert_eq!(
				signed["x-amz-content-sha256"],
				"2ceba15d1c5fc3132e71ba2fe80537e22d509e7c446f91468e3ea925e64528ba"
			);
			assert_eq!(
				signed["authorization"],
				"AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261107/us-west-2/bedrock/aws4_request, SignedHeaders=amz-sdk-invocation-id;amz-sdk-request;content-length;content-type;host;x-amz-content-sha256;x-amz-date;x-amz-user-agent;x-custom, Signature=af5a01547f6bff7386280d4c10ddfd3a9d2713b6d2854c7df6af8f298e95467e"
			);
		});
	}

	/// `client.sign` of the request headers of the `basic-sigv4` oracle case, with `x-custom` set to `custom`.
	fn sign_fixture(client: &BedrockRuntimeClient, custom: &str) -> IndexMap<String, String> {
		let headers: IndexMap<String, String> = [
			("content-type", "application/json"),
			("content-length", "216"),
			("x-amz-user-agent", "aws-sdk-js/3.1126.0 pi/1.5.0"),
			(
				"user-agent",
				"aws-sdk-js/3.1126.0 ua/2.1 os/linux lang/rust api/bedrock-runtime#3.1126.0 m/N,E,e pi/1.5.0",
			),
			("host", "127.0.0.1:45123"),
			("x-custom", custom),
			("amz-sdk-invocation-id", "637f9490-e814-484e-8000-13b957ad6b5c"),
			("amz-sdk-request", "attempt=1; max=3"),
		]
		.into_iter()
		.map(|(name, value)| (name.to_string(), value.to_string()))
		.collect();
		let identity = Identity::SigV4(
			AwsCredentialIdentity {
				access_key_id: "AKIDEXAMPLE".into(),
				secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
				session_token: None,
			},
			Some("e"),
		);
		let url = "http://127.0.0.1:45123/model/us.anthropic.claude-opus-4-8/converse-stream";
		client
			.sign(&headers, url, BODY.as_bytes(), &identity, "us-west-2")
			.expect("signs")
	}

	/// Recorded with `@smithy/signature-v4`, whose canonical header value is `value.trim().replace(/\s+/g, " ")`.
	#[test]
	fn signs_header_values_with_collapsed_whitespace_and_sends_them_unchanged() {
		pi_host::testing::run(|host| async move {
			host.advance(1794052800000.0 - pi_host::now_ms());
			let client = BedrockRuntimeClient::new(BedrockRuntimeClientConfig::default());
			let signed = sign_fixture(&client, " a\tb \u{a0} c\n");
			assert_eq!(signed["x-custom"], " a\tb \u{a0} c\n");
			assert!(
				signed["authorization"]
					.ends_with("Signature=8a31fea94c6b1feb5473e52599fe3292543a932e8ea86dc64d72286506ab0062"),
				"{}",
				signed["authorization"]
			);
		});
	}
}
