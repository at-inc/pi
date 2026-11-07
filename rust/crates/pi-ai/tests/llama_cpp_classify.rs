//! Port of `packages/ai/test/llama-cpp-classify.test.ts`.
//!
//! The fake `llama-server` is a `FetchFunction` that records each request. Each test runs on its own thread, so the
//! thread-local label token cache starts empty; the fresh server URL per model keeps the TS reason for it visible.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::api::llama_cpp_classify::{
	answer_from_probabilities, classify, label_probabilities, llama_server_root, peak_confidence, render_question,
};
use pi_ai::types::{
	BaseModel, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierQuestion, ClassifierStopReason,
	ModelCost, ModelInputModality, ModelType, OnPayload, OnResponse, ProviderRequestOptions,
};
use pi_host::testing::run;
use pi_host::{AbortController, FetchFunction, HttpRequest, HttpResponse};
use serde_json::{Value, json};

thread_local! {
	static SERVER_COUNT: Cell<u32> = const { Cell::new(0) };
}

/// A fresh server URL per test: label tokens are cached per server and model.
fn model() -> ClassifierModel {
	let count = SERVER_COUNT.with(|count| {
		count.set(count.get() + 1);
		count.get()
	});
	ClassifierModel {
		base: BaseModel {
			id: "qwen".into(),
			name: "qwen".into(),
			api: "llama-cpp-classify".into(),
			provider: "llama.cpp".into(),
			base_url: format!("http://llama-{count}.test:8080/v1"),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		r#type: ModelType::Classifier,
		context_window: 32768.0,
		compat: None,
		extra: Default::default(),
	}
}

#[derive(Clone)]
struct RecordedRequest {
	url: String,
	body: Value,
	request: HttpRequest,
}

type NextFn = Rc<dyn Fn(&str, f64) -> Vec<(&'static str, f64)>>;
type TemplateFn = Rc<dyn Fn(&[Value]) -> String>;
type TokenizeFn = Rc<dyn Fn(&str) -> Vec<u32>>;

#[derive(Default, Clone)]
struct FakeServerOptions {
	/// Log-probabilities of the next token by token text, in rank order.
	next: Option<NextFn>,
	template: Option<TemplateFn>,
	tokenize: Option<TokenizeFn>,
}

/// Token IDs: one per character, the character code.
fn char_tokens(content: &str) -> Vec<u32> {
	content.chars().map(|char| char as u32).collect()
}

fn json_response(body: Value) -> HttpResponse {
	HttpResponse::from_bytes(
		200,
		IndexMap::from([("content-type".to_string(), "application/json".to_string())]),
		pi_host::js::json_stringify(&body),
	)
}

struct FakeServer {
	fetch: FetchFunction,
	requests: Rc<RefCell<Vec<RecordedRequest>>>,
}

fn fake_server(options: FakeServerOptions) -> FakeServer {
	let requests: Rc<RefCell<Vec<RecordedRequest>>> = Rc::new(RefCell::new(Vec::new()));
	let recorded = requests.clone();
	let fetch: FetchFunction = Rc::new(move |request: HttpRequest| {
		let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
		recorded.borrow_mut().push(RecordedRequest {
			url: request.url.clone(),
			body: body.clone(),
			request: request.clone(),
		});
		let path = url::Url::parse(&request.url).unwrap().path().to_string();
		let response = match path.as_str() {
			"/tokenize" => {
				let content = body["content"].as_str().unwrap().to_string();
				let tokens = options
					.tokenize
					.as_ref()
					.map_or_else(|| char_tokens(&content), |tokenize| tokenize(&content));
				json_response(json!({ "tokens": tokens }))
			}
			"/apply-template" => {
				let messages = body["messages"].as_array().unwrap().clone();
				let prompt = options.template.as_ref().map_or_else(
					|| {
						let rendered: String = messages
							.iter()
							.map(|message| {
								format!(
									"<|{}|>\n{}\n",
									message["role"].as_str().unwrap(),
									message["content"].as_str().unwrap()
								)
							})
							.collect();
						format!("{rendered}<|assistant|>\n")
					},
					|template| template(&messages),
				);
				json_response(json!({ "prompt": prompt }))
			}
			"/completion" => {
				let next = options.next.as_ref().map_or_else(
					|| vec![("A", -0.1), ("B", -2.5)],
					|next| next(body["prompt"].as_str().unwrap(), body["n_probs"].as_f64().unwrap()),
				);
				let top_logprobs: Vec<Value> = next
					.into_iter()
					.map(|(token, logprob)| {
						json!({ "id": token.chars().next().unwrap() as u32, "token": token, "bytes": [], "logprob": logprob })
					})
					.collect();
				json_response(json!({
					"content": "A",
					"completion_probabilities": [{ "id": 65, "token": "A", "top_logprobs": top_logprobs }],
				}))
			}
			_ => HttpResponse::from_bytes(404, IndexMap::new(), "not found"),
		};
		Box::pin(async move { Ok(response) })
	});
	FakeServer { fetch, requests }
}

/// Completion log-probabilities for bool (Yes/No) and letter labels; the fake tokenizer maps a label to its first
/// character.
fn answer_by_prompt(prompt: &str, _depth: f64) -> Vec<(&'static str, f64)> {
	if prompt.contains("Answer Yes or No.") {
		return vec![("Y", -0.05), ("N", -3.0)];
	}
	if prompt.contains("Answer with one level number.") {
		return vec![("2", -0.2), ("1", -1.8), ("0", -4.0)];
	}
	vec![("B", -0.3), ("A", -1.5), ("C", -3.0)]
}

fn context() -> ClassifierContext {
	serde_json::from_value(json!({
		"state": { "message": "Help! My payouts have been failing for 3 days." },
		"questions": {
			"team": {
				"type": "choice",
				"instructions": "Which team should handle this?",
				"criteria": { "billing": "Payments and refunds", "technical": "Bugs and outages", "sales": "" },
			},
			"urgent": {
				"type": "bool",
				"instructions": "Does this convey urgency?",
				"criteria": { "true": "The user needs help soon", "false": "No time pressure" },
			},
			"severity": { "type": "score", "instructions": "How severe is this?", "criteria": ["low", "medium", "high"] },
		},
	}))
	.unwrap()
}

fn questions_context(questions: Value) -> ClassifierContext {
	serde_json::from_value(json!({ "state": {}, "questions": questions })).unwrap()
}

fn pick() -> Value {
	json!({ "pick": { "type": "choice", "instructions": "Pick one", "criteria": { "a": "", "b": "" } } })
}

fn pick_short() -> Value {
	json!({ "pick": { "type": "choice", "instructions": "Pick", "criteria": { "a": "", "b": "" } } })
}

/// Maps multi-character labels to single tokens, as a real vocabulary would.
fn word_tokens(content: &str) -> Vec<u32> {
	let mut tokens = Vec::new();
	let mut parts: Vec<&str> = Vec::new();
	let mut rest = content;
	while let Some(index) = rest.find('\n') {
		parts.push(&rest[..index]);
		parts.push("\n");
		rest = &rest[index + 1..];
	}
	parts.push(rest);
	for part in parts {
		match part {
			"" => {}
			"Yes" => tokens.push(89),
			"No" => tokens.push(78),
			part => tokens.extend(char_tokens(part)),
		}
	}
	tokens
}

fn options(fetch: FetchFunction) -> ClassifierOptions {
	ClassifierOptions {
		base: ProviderRequestOptions {
			fetch: Some(fetch),
			..ProviderRequestOptions::default()
		},
		temperature: None,
	}
}

fn to_json(value: impl serde::Serialize) -> Value {
	serde_json::to_value(value).unwrap()
}

fn completion_depths(requests: &[RecordedRequest]) -> Vec<f64> {
	requests
		.iter()
		.filter(|request| request.url.ends_with("/completion"))
		.map(|request| request.body["n_probs"].as_f64().unwrap())
		.collect()
}

mod llama_cpp_classifier {
	use super::*;

	#[test]
	fn answers_choice_bool_and_score_questions_from_label_log_probabilities() {
		run(|_| async {
			let server = fake_server(FakeServerOptions {
				next: Some(Rc::new(answer_by_prompt)),
				tokenize: Some(Rc::new(word_tokens)),
				..FakeServerOptions::default()
			});
			let classifier_model = model();
			let mut request_options = options(server.fetch.clone());
			request_options.api_key = Some("local".into());

			let result = classify(&classifier_model, &context(), Some(request_options))
				.await
				.unwrap();

			assert_eq!(result.error_message, None);
			assert_eq!(result.stop_reason, ClassifierStopReason::Stop);
			let choice = label_probabilities(&[-1.5, -0.3, -3.0], 1.0);
			assert_eq!(
				to_json(&result.answers["team"]),
				json!({
					"type": "choice",
					"choice": "technical",
					"probabilities": { "billing": choice[0], "technical": choice[1], "sales": choice[2] },
					"confidence": peak_confidence(&choice),
				})
			);
			assert_eq!(
				to_json(&result.answers["urgent"]),
				json!({ "type": "bool", "probability": label_probabilities(&[-0.05, -3.0], 1.0)[0] })
			);
			let levels = label_probabilities(&[-4.0, -1.8, -0.2], 1.0);
			assert_eq!(
				to_json(&result.answers["severity"]),
				json!({ "type": "score", "score": levels[1] + 2.0 * levels[2], "confidence": peak_confidence(&levels) })
			);

			let root = classifier_model.base_url.strip_suffix("/v1").unwrap().to_string();
			let requests = server.requests.borrow();
			for request in requests.iter() {
				assert!(request.url.starts_with(&format!("{root}/")));
				assert_eq!(request.body["model"], "qwen");
				assert_eq!(request.request.header("authorization"), Some("Bearer local"));
			}
			let completion = requests
				.iter()
				.find(|request| request.url.ends_with("/completion"))
				.unwrap();
			assert_eq!(completion.body["n_predict"], 1);
			assert_eq!(completion.body["n_probs"].as_f64(), Some(256.0));
			assert_eq!(completion.body["post_sampling_probs"], false);
			assert_eq!(completion.body["cache_prompt"], true);
			let template = requests
				.iter()
				.find(|request| request.url.ends_with("/apply-template"))
				.unwrap();
			assert_eq!(
				template.body["chat_template_kwargs"],
				json!({ "enable_thinking": false })
			);
		});
	}

	#[test]
	fn repeats_the_state_around_all_questions_and_ends_with_this_questions_labels() {
		let rendered = render_question(&context(), "team").unwrap();
		let state = "State:\n{\n \"message\": \"Help! My payouts have been failing for 3 days.\"\n}";
		assert_eq!(rendered.labels, ["A", "B", "C"]);
		assert_eq!(rendered.keys, ["billing", "technical", "sales"]);
		assert_eq!(
			rendered.content,
			[
				state,
				"",
				"Task: answer each of the following questions about the state.",
				"",
				"Question: Which team should handle this?",
				"",
				"Options:",
				"- billing: Payments and refunds",
				"- technical: Bugs and outages",
				"- sales",
				"",
				"Question: Does this convey urgency?",
				"",
				"Yes means: The user needs help soon",
				"No means: No time pressure",
				"",
				"Question: How severe is this?",
				"",
				"Levels:",
				"0. low",
				"1. medium",
				"2. high",
				"",
				state,
				"",
				"Question: Which team should handle this?",
				"",
				"Options:",
				"A. billing: Payments and refunds",
				"B. technical: Bugs and outages",
				"C. sales",
				"",
				"Answer with one letter.",
			]
			.join("\n")
		);
	}

	#[test]
	fn shares_everything_before_the_final_question_across_the_questions_of_a_request() {
		let prefix = |id: &str| {
			let content = render_question(&context(), id).unwrap().content;
			content[..content.rfind("Question:").unwrap()].to_string()
		};
		assert_eq!(prefix("urgent"), prefix("team"));
		assert_eq!(prefix("severity"), prefix("team"));
		assert!(
			render_question(&context(), "urgent")
				.unwrap()
				.content
				.ends_with("No means: No time pressure\n\nAnswer Yes or No.")
		);
		assert!(
			render_question(&context(), "severity")
				.unwrap()
				.content
				.ends_with("2. high\n\nAnswer with one level number.")
		);
	}

	#[test]
	fn divides_label_log_probabilities_by_the_temperature() {
		run(|_| async {
			let server = fake_server(FakeServerOptions {
				next: Some(Rc::new(|_, _| vec![("A", -0.1), ("B", -2.5)])),
				..FakeServerOptions::default()
			});
			let mut request_options = options(server.fetch.clone());
			request_options.temperature = Some(2.0);

			let result = classify(&model(), &questions_context(pick()), Some(request_options))
				.await
				.unwrap();

			let expected = label_probabilities(&[-0.1 / 2.0, -2.5 / 2.0], 1.0);
			assert_eq!(
				to_json(&result.answers["pick"])["probabilities"],
				json!({ "a": expected[0], "b": expected[1] })
			);
			assert_eq!(label_probabilities(&[-0.1, -2.5], 2.0), expected);
		});
	}

	#[test]
	fn rejects_non_positive_temperatures_before_sending_requests() {
		run(|_| async {
			let server = fake_server(FakeServerOptions::default());
			let mut request_options = options(server.fetch.clone());
			request_options.temperature = Some(0.0);
			let result = classify(&model(), &context(), Some(request_options)).await.unwrap();

			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert!(
				result
					.error_message
					.unwrap()
					.contains("Temperature must be a positive number, got 0")
			);
			assert!(server.requests.borrow().is_empty());
		});
	}

	#[test]
	fn retries_deeper_readouts_when_a_label_is_missing_and_fails_without_inventing_zeros() {
		run(|_| async {
			let deep = fake_server(FakeServerOptions {
				next: Some(Rc::new(|_, depth| {
					if depth < 4096.0 {
						vec![("A", -0.1)]
					} else {
						vec![("A", -0.1), ("B", -9.0)]
					}
				})),
				..FakeServerOptions::default()
			});
			let recovered = classify(&model(), &questions_context(pick()), Some(options(deep.fetch.clone())))
				.await
				.unwrap();
			assert_eq!(recovered.stop_reason, ClassifierStopReason::Stop);
			assert_eq!(completion_depths(&deep.requests.borrow()), [256.0, 4096.0]);

			let never = fake_server(FakeServerOptions {
				next: Some(Rc::new(|_, _| vec![("A", -0.1)])),
				..FakeServerOptions::default()
			});
			let failed = classify(&model(), &questions_context(pick()), Some(options(never.fetch.clone())))
				.await
				.unwrap();
			assert_eq!(failed.stop_reason, ClassifierStopReason::Error);
			assert!(failed.answers.is_empty());
			assert!(
				failed
					.error_message
					.unwrap()
					.contains("did not rank labels B for pick within the top 32768 tokens")
			);
			assert_eq!(completion_depths(&never.requests.borrow()), [256.0, 4096.0, 32768.0]);
		});
	}

	#[test]
	fn closes_a_reasoning_block_the_template_leaves_open() {
		run(|_| async {
			let server = fake_server(FakeServerOptions {
				template: Some(Rc::new(|_| "<|assistant|>\n<think>".into())),
				..FakeServerOptions::default()
			});
			classify(
				&model(),
				&questions_context(pick_short()),
				Some(options(server.fetch.clone())),
			)
			.await
			.unwrap();

			let requests = server.requests.borrow();
			let completion = requests
				.iter()
				.find(|request| request.url.ends_with("/completion"))
				.unwrap();
			assert_eq!(completion.body["prompt"], "<|assistant|>\n<think></think>");
		});
	}

	#[test]
	fn reads_labels_in_reply_position_and_rejects_labels_that_are_not_one_token() {
		run(|_| async {
			// A tokenizer that merges a newline with a following letter falls back to the label alone.
			let merging = fake_server(FakeServerOptions {
				tokenize: Some(Rc::new(|content| {
					if content.starts_with('\n') && content.chars().count() > 1 {
						vec![1000]
					} else {
						char_tokens(content)
					}
				})),
				..FakeServerOptions::default()
			});
			let merged = classify(
				&model(),
				&questions_context(pick_short()),
				Some(options(merging.fetch.clone())),
			)
			.await
			.unwrap();
			assert_eq!(merged.stop_reason, ClassifierStopReason::Stop);

			// The default fake tokenizer splits "Yes" into three tokens.
			let split = fake_server(FakeServerOptions::default());
			let result = classify(
				&model(),
				&questions_context(
					json!({ "ok": { "type": "bool", "instructions": "OK?", "criteria": { "true": "", "false": "" } } }),
				),
				Some(options(split.fetch.clone())),
			)
			.await
			.unwrap();
			assert_eq!(result.stop_reason, ClassifierStopReason::Error);
			assert!(
				result
					.error_message
					.unwrap()
					.contains("Label \"Yes\" is not a single token for qwen")
			);
		});
	}

	#[test]
	fn caches_label_tokens_per_server_and_model() {
		run(|_| async {
			let classifier_model = model();
			let server = fake_server(FakeServerOptions::default());
			let tokenizations = |server: &FakeServer| {
				server
					.requests
					.borrow()
					.iter()
					.filter(|request| request.url.ends_with("/tokenize"))
					.count()
			};
			classify(
				&classifier_model,
				&questions_context(pick_short()),
				Some(options(server.fetch.clone())),
			)
			.await
			.unwrap();
			let first_tokenizations = tokenizations(&server);
			classify(
				&classifier_model,
				&questions_context(pick_short()),
				Some(options(server.fetch.clone())),
			)
			.await
			.unwrap();

			assert!(first_tokenizations > 0);
			assert_eq!(tokenizations(&server), first_tokenizations);
		});
	}

	#[test]
	fn validates_option_counts_before_sending_requests() {
		run(|_| async {
			let server = fake_server(FakeServerOptions::default());
			let criteria: serde_json::Map<String, Value> =
				(0..63).map(|index| (format!("option{index}"), json!(""))).collect();
			let too_many = classify(
				&model(),
				&questions_context(
					json!({ "pick": { "type": "choice", "instructions": "Pick", "criteria": criteria } }),
				),
				Some(options(server.fetch.clone())),
			)
			.await
			.unwrap();
			let too_few = classify(
				&model(),
				&questions_context(
					json!({ "rate": { "type": "score", "instructions": "Rate", "criteria": ["only"] } }),
				),
				Some(options(server.fetch.clone())),
			)
			.await
			.unwrap();

			assert!(
				too_many
					.error_message
					.unwrap()
					.contains("A choice question needs 2 to 62 options, got 63")
			);
			assert!(
				too_few
					.error_message
					.unwrap()
					.contains("A score question needs 2 to 10 levels, got 1")
			);
			assert!(server.requests.borrow().is_empty());
		});
	}

	#[test]
	fn passes_completion_payloads_and_responses_through_the_request_hooks() {
		run(|_| async {
			let server = fake_server(FakeServerOptions::default());
			let payloads: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
			let statuses: Rc<RefCell<Vec<u16>>> = Rc::new(RefCell::new(Vec::new()));
			let seen_payloads = payloads.clone();
			let seen_statuses = statuses.clone();
			let on_payload: OnPayload<ClassifierModel> = Rc::new(move |payload: Value, _: &ClassifierModel| {
				seen_payloads.borrow_mut().push(payload.clone());
				let mut payload = payload;
				payload["id_slot"] = json!(1);
				Box::pin(async move { Ok(Some(payload)) })
			});
			let on_response: OnResponse<ClassifierModel> = Rc::new(move |response, _: &ClassifierModel| {
				seen_statuses.borrow_mut().push(response.status);
				Box::pin(async { Ok(()) })
			});
			let mut request_options = options(server.fetch.clone());
			request_options.on_payload = Some(on_payload);
			request_options.on_response = Some(on_response);

			classify(&model(), &questions_context(pick_short()), Some(request_options))
				.await
				.unwrap();

			let payloads = payloads.borrow();
			assert_eq!(payloads.len(), 1);
			assert_eq!(payloads[0]["n_predict"], 1);
			assert_eq!(*statuses.borrow(), [200]);
			let requests = server.requests.borrow();
			let completion = requests
				.iter()
				.find(|request| request.url.ends_with("/completion"))
				.unwrap();
			assert_eq!(completion.body["id_slot"], 1);
		});
	}

	#[test]
	fn reports_server_errors_and_cancellation() {
		run(|_| async {
			let failing_fetch: FetchFunction = Rc::new(|_| {
				Box::pin(async {
					Ok(HttpResponse::from_bytes(
						400,
						IndexMap::new(),
						r#"{"error":{"message":"context overflow"}}"#,
					))
				})
			});
			let mut failing_options = options(failing_fetch);
			failing_options.max_retries = Some(0.0);
			let failing = classify(&model(), &context(), Some(failing_options)).await.unwrap();
			assert_eq!(failing.stop_reason, ClassifierStopReason::Error);
			let message = failing.error_message.unwrap();
			assert!(message.contains("llama.cpp error (400)"), "{message}");
			assert!(message.contains("context overflow"), "{message}");

			let controller = AbortController::new();
			controller.abort();
			let aborting_fetch: FetchFunction = Rc::new(|request: HttpRequest| {
				Box::pin(async move {
					if let Some(signal) = &request.signal {
						signal.throw_if_aborted()?;
					}
					Ok(json_response(json!({})))
				})
			});
			let mut aborted_options = options(aborting_fetch);
			aborted_options.signal = Some(controller.signal());
			let aborted = classify(&model(), &context(), Some(aborted_options)).await.unwrap();
			assert_eq!(aborted.stop_reason, ClassifierStopReason::Aborted);
		});
	}

	#[test]
	fn rejects_models_for_other_classifier_apis() {
		run(|_| async {
			let server = fake_server(FakeServerOptions::default());
			let mut other = model();
			other.api = "typesafe-system-one".into();
			let result = classify(&other, &context(), Some(options(server.fetch.clone())))
				.await
				.unwrap();

			assert!(
				result
					.error_message
					.unwrap()
					.contains("Unsupported classifier API: typesafe-system-one")
			);
			assert!(server.requests.borrow().is_empty());
		});
	}

	#[test]
	fn derives_the_server_root_from_openai_compatible_base_urls() {
		assert_eq!(llama_server_root("http://127.0.0.1:8080/v1/"), "http://127.0.0.1:8080");
		assert_eq!(
			llama_server_root("https://example.com/prefix/v1"),
			"https://example.com/prefix"
		);
		assert_eq!(llama_server_root("http://127.0.0.1:8080"), "http://127.0.0.1:8080");
	}

	#[test]
	fn computes_typesafes_confidence_and_expected_scores() {
		assert!((peak_confidence(&[0.89, 0.06, 0.05]) - 0.835).abs() < 0.005);
		assert_eq!(peak_confidence(&[0.5, 0.5]), 0.0);
		assert_eq!(peak_confidence(&[1.0, 0.0, 0.0]), 1.0);
		let question: ClassifierQuestion =
			serde_json::from_value(json!({ "type": "score", "instructions": "", "criteria": ["a", "b", "c"] }))
				.unwrap();
		assert_eq!(
			to_json(answer_from_probabilities(
				&question,
				&["0".into(), "1".into(), "2".into()],
				&[0.2, 0.3, 0.5]
			)),
			json!({ "type": "score", "score": 1.3, "confidence": peak_confidence(&[0.2, 0.3, 0.5]) })
		);
	}
}
