//! Helpers shared by the ported vitest suites.
#![allow(dead_code)]

use std::rc::Rc;

use pi_host::testing::TestHost;

/// Builds `SpanAttributes` from `key => value` pairs; `key => None` is an explicit `undefined`.
#[macro_export]
macro_rules! attrs {
	() => { pi_telemetry::SpanAttributes::new() };
	($($key:expr => $value:expr),+ $(,)?) => {{
		let mut attributes = pi_telemetry::SpanAttributes::new();
		$(attributes.insert($key.to_string(), $crate::common::IntoAttribute::into_attribute($value));)+
		attributes
	}};
}

/// Converts test literals into `Option<AttributeValue>`.
pub trait IntoAttribute {
	fn into_attribute(self) -> Option<pi_telemetry::AttributeValue>;
}

impl<T: Into<pi_telemetry::AttributeValue>> IntoAttribute for T {
	fn into_attribute(self) -> Option<pi_telemetry::AttributeValue> {
		Some(self.into())
	}
}

/// The `undefined` attribute value.
pub struct Undefined;

impl IntoAttribute for Undefined {
	fn into_attribute(self) -> Option<pi_telemetry::AttributeValue> {
		None
	}
}

/// `vi.spyOn(Date, "now").mockReturnValue(ms)`: moves the shared virtual clock to `ms`.
pub fn set_now(host: &Rc<TestHost>, ms: f64) {
	host.advance(ms - pi_host::now_ms());
}

/// `SpanOptions` with only a name.
pub fn options(name: &str) -> pi_telemetry::SpanOptions {
	pi_telemetry::SpanOptions {
		name: name.to_string(),
		..Default::default()
	}
}

/// Object identity (`toBe`) between two `Rc` handles of possibly different types.
pub fn same_object<A: ?Sized, B: ?Sized>(left: &Rc<A>, right: &Rc<B>) -> bool {
	std::ptr::addr_eq(Rc::as_ptr(left), Rc::as_ptr(right))
}
