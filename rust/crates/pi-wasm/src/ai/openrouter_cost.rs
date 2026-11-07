//! Exports of `utils/openrouter-cost`.

use js_sys::Reflect;
use pi_ai::Usage;
use pi_ai::utils::openrouter_cost;
use serde_json::Value;
use wasm_bindgen::prelude::*;

use crate::json::from_js_text;
use crate::options::throw;

/// `isOpenRouterModel(model)`: reads `provider` and `baseUrl`.
#[wasm_bindgen(js_name = aiIsOpenRouterModel)]
pub fn is_open_router_model(model: JsValue) -> Result<bool, JsValue> {
	let field = |name: &str| -> Result<String, JsValue> {
		Ok(Reflect::get(&model, &name.into())?.as_string().unwrap_or_default())
	};
	Ok(openrouter_cost::is_open_router_target(
		&field("provider")?,
		&field("baseUrl")?,
	))
}

/// `applyOpenRouterReportedCost(usage, reported)`: sets `usage.cost.total` and `usage.cost.totalFromProvider` on
/// the caller's object, as the TS does.
#[wasm_bindgen(js_name = aiApplyOpenRouterReportedCost)]
pub fn apply_open_router_reported_cost(usage: JsValue, reported: JsValue) -> Result<(), JsValue> {
	let mut parsed: Usage = throw(from_js_text(&usage))?;
	let reported: Value = throw(from_js_text(&reported))?;
	let before = parsed.cost.clone();
	openrouter_cost::apply_open_router_reported_cost(&mut parsed, &reported);
	if parsed.cost == before {
		return Ok(());
	}
	let cost = Reflect::get(&usage, &"cost".into())?;
	Reflect::set(&cost, &"total".into(), &parsed.cost.total.into())?;
	Reflect::set(
		&cost,
		&"totalFromProvider".into(),
		&parsed.cost.total_from_provider.unwrap_or(false).into(),
	)?;
	Ok(())
}
