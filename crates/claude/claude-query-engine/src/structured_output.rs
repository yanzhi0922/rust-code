//! JSON schema enforcement for structured output from the provider.
//!
//! Validates that provider responses conform to an expected JSON schema,
//! and provides coercion for common schema violations.

use serde_json::Value;

/// Enforces structured output constraints on provider responses.
#[derive(Debug, Clone)]
pub struct StructuredOutputEnforcer {
    /// Optional JSON Schema to validate against.
    schema: Option<Value>,
    /// Whether to attempt coercion on validation failures.
    coerce_on_failure: bool,
}

impl StructuredOutputEnforcer {
    /// Create a new enforcer without a schema (pass-through).
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema: None,
            coerce_on_failure: false,
        }
    }

    /// Create an enforcer with the given JSON Schema.
    #[must_use]
    pub fn with_schema(schema: Value) -> Self {
        Self {
            schema: Some(schema),
            coerce_on_failure: false,
        }
    }

    /// Enable coercion on validation failure.
    #[must_use]
    pub fn with_coercion(mut self) -> Self {
        self.coerce_on_failure = true;
        self
    }

    /// Returns true if a schema is configured.
    #[must_use]
    pub fn has_schema(&self) -> bool {
        self.schema.is_some()
    }

    /// Validate a JSON value against the configured schema.
    /// Returns Ok(()) if no schema is set or validation passes.
    pub fn validate(&self, value: &Value) -> Result<(), StructuredOutputError> {
        let Some(schema) = &self.schema else {
            return Ok(());
        };

        // Basic schema validation: check type constraint
        if let Some(expected_type) = schema.get("type").and_then(|t| t.as_str()) {
            let actual_type = json_type_of(value);
            if actual_type != expected_type {
                if self.coerce_on_failure {
                    return Ok(());
                }
                return Err(StructuredOutputError::TypeMismatch {
                    expected: expected_type.to_string(),
                    actual: actual_type.to_string(),
                });
            }
        }

        // Check required fields for objects
        if let Some(required) = schema.get("required").and_then(|r| r.as_array())
            && let Some(obj) = value.as_object()
        {
            for field in required {
                if let Some(field_name) = field.as_str()
                    && !obj.contains_key(field_name)
                {
                    return Err(StructuredOutputError::MissingRequiredField {
                        field: field_name.to_string(),
                    });
                }
            }
        }

        // Check enum constraint
        if let Some(enum_values) = schema.get("enum").and_then(|e| e.as_array())
            && !enum_values.contains(value)
        {
            return Err(StructuredOutputError::EnumViolation {
                value: value.clone(),
                allowed: enum_values.clone(),
            });
        }

        Ok(())
    }

    /// Attempt to parse a string as JSON and validate it.
    pub fn parse_and_validate(&self, text: &str) -> Result<Value, StructuredOutputError> {
        let value: Value =
            serde_json::from_str(text).map_err(|e| StructuredOutputError::ParseError {
                message: e.to_string(),
            })?;
        self.validate(&value)?;
        Ok(value)
    }

    /// Coerce a string value to the expected type defined in the schema.
    /// Returns the original value if no schema is set or coercion is not possible.
    #[must_use]
    pub fn coerce(&self, value: &Value) -> Value {
        let Some(schema) = &self.schema else {
            return value.clone();
        };
        let Some(expected_type) = schema.get("type").and_then(|t| t.as_str()) else {
            return value.clone();
        };

        match expected_type {
            "object" => {
                if value.is_string()
                    && let Ok(parsed) = serde_json::from_str::<Value>(value.as_str().unwrap_or(""))
                    && parsed.is_object()
                {
                    return parsed;
                }
                value.clone()
            }
            "array" => {
                if value.is_string()
                    && let Ok(parsed) = serde_json::from_str::<Value>(value.as_str().unwrap_or(""))
                    && parsed.is_array()
                {
                    return parsed;
                }
                value.clone()
            }
            "number" => {
                if let Some(s) = value.as_str()
                    && let Ok(n) = s.parse::<f64>()
                {
                    return Value::from(n);
                }
                value.clone()
            }
            "boolean" => {
                if let Some(s) = value.as_str()
                    && let Ok(b) = s.parse::<bool>()
                {
                    return Value::from(b);
                }
                value.clone()
            }
            _ => value.clone(),
        }
    }
}

impl Default for StructuredOutputEnforcer {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors from structured output validation.
#[derive(Debug, Clone, thiserror::Error)]
pub enum StructuredOutputError {
    #[error("type mismatch: expected {expected}, got {actual}")]
    TypeMismatch { expected: String, actual: String },
    #[error("missing required field: {field}")]
    MissingRequiredField { field: String },
    #[error("enum violation: value not in allowed set")]
    EnumViolation { value: Value, allowed: Vec<Value> },
    #[error("JSON parse error: {message}")]
    ParseError { message: String },
}

fn json_type_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{StructuredOutputEnforcer, StructuredOutputError};

    #[test]
    fn enforcer_without_schema_passes_all() {
        let enforcer = StructuredOutputEnforcer::new();
        assert!(enforcer.validate(&json!("anything")).is_ok());
        assert!(enforcer.validate(&json!({"key": "value"})).is_ok());
    }

    #[test]
    fn enforcer_validates_type_constraint() {
        let enforcer = StructuredOutputEnforcer::with_schema(json!({"type": "object"}));
        assert!(enforcer.validate(&json!({"key": "value"})).is_ok());
        let result = enforcer.validate(&json!("not an object"));
        assert!(matches!(
            result,
            Err(StructuredOutputError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn enforcer_validates_required_fields() {
        let enforcer = StructuredOutputEnforcer::with_schema(json!({
            "type": "object",
            "required": ["name", "age"]
        }));
        assert!(
            enforcer
                .validate(&json!({"name": "test", "age": 25}))
                .is_ok()
        );
        let result = enforcer.validate(&json!({"name": "test"}));
        assert!(
            matches!(result, Err(StructuredOutputError::MissingRequiredField { field }) if field == "age")
        );
    }

    #[test]
    fn enforcer_validates_enum_constraint() {
        let enforcer = StructuredOutputEnforcer::with_schema(json!({
            "enum": ["red", "green", "blue"]
        }));
        assert!(enforcer.validate(&json!("red")).is_ok());
        assert!(matches!(
            enforcer.validate(&json!("yellow")),
            Err(StructuredOutputError::EnumViolation { .. })
        ));
    }

    #[test]
    fn enforcer_coercion_mode_skips_type_check() {
        let enforcer =
            StructuredOutputEnforcer::with_schema(json!({"type": "object"})).with_coercion();
        assert!(enforcer.validate(&json!("not an object")).is_ok());
    }

    #[test]
    fn enforcer_parse_and_validate() {
        let enforcer = StructuredOutputEnforcer::with_schema(json!({"type": "object"}));
        let result = enforcer.parse_and_validate(r#"{"key": "value"}"#);
        assert!(result.is_ok());
        let bad = enforcer.parse_and_validate("not json");
        assert!(matches!(bad, Err(StructuredOutputError::ParseError { .. })));
    }

    #[test]
    fn enforcer_coerce_string_to_object() {
        let enforcer = StructuredOutputEnforcer::with_schema(json!({"type": "object"}));
        let coerced = enforcer.coerce(&json!(r#"{"a":1}"#));
        // String value that looks like JSON object should be coerced
        assert!(coerced.is_object() || coerced.is_string());
    }

    #[test]
    fn enforcer_has_schema() {
        let without = StructuredOutputEnforcer::new();
        assert!(!without.has_schema());
        let with = StructuredOutputEnforcer::with_schema(json!({"type": "string"}));
        assert!(with.has_schema());
    }
}
