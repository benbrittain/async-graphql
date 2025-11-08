use jiff::Span;

use crate::{InputValueError, InputValueResult, Scalar, ScalarType, Value};

/// Implement the Duration scalar
///
/// The input/output is a string in ISO8601 format.
#[Scalar(
    internal,
    name = "Duration",
    specified_by_url = "https://en.wikipedia.org/wiki/ISO_8601#Durations"
)]
impl ScalarType for Span {
    fn parse(value: Value) -> InputValueResult<Self> {
        match &value {
            Value::String(s) => Ok(s.parse::<Span>()?),
            _ => Err(InputValueError::expected_type(value)),
        }
    }

    fn to_value(&self) -> Value {
        Value::String(self.to_string())
    }
}
