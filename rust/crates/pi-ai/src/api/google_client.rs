//! Rust-only replacement for the parts of `@google/genai` (v2.21.0) that pi-ai uses.
//! PORT-STATUS: pending

/// `FunctionCallingConfigMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FunctionCallingConfigMode {
	ModeUnspecified,
	Auto,
	Any,
	None,
	Validated,
}

impl FunctionCallingConfigMode {
	/// The wire value.
	pub fn as_str(self) -> &'static str {
		match self {
			FunctionCallingConfigMode::ModeUnspecified => "MODE_UNSPECIFIED",
			FunctionCallingConfigMode::Auto => "AUTO",
			FunctionCallingConfigMode::Any => "ANY",
			FunctionCallingConfigMode::None => "NONE",
			FunctionCallingConfigMode::Validated => "VALIDATED",
		}
	}
}

/// `ThinkingLevel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThinkingLevel {
	ThinkingLevelUnspecified,
	Minimal,
	Low,
	Medium,
	High,
}

impl ThinkingLevel {
	/// The wire value.
	pub fn as_str(self) -> &'static str {
		match self {
			ThinkingLevel::ThinkingLevelUnspecified => "THINKING_LEVEL_UNSPECIFIED",
			ThinkingLevel::Minimal => "MINIMAL",
			ThinkingLevel::Low => "LOW",
			ThinkingLevel::Medium => "MEDIUM",
			ThinkingLevel::High => "HIGH",
		}
	}
}
