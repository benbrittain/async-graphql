use jiff::{Timestamp, Zoned};

use crate::{InputValueError, InputValueResult, Scalar, ScalarType, Value};

/// Implement the Zoned scalar (replaces DateTime<FixedOffset>)
///
/// The input/output is a string in RFC3339 format.
#[Scalar(
    internal,
    name = "DateTime",
    specified_by_url = "https://datatracker.ietf.org/doc/html/rfc3339"
)]
impl ScalarType for Zoned {
    fn parse(value: Value) -> InputValueResult<Self> {
        match &value {
            Value::String(s) => Ok(s.parse::<Zoned>()?),
            _ => Err(InputValueError::expected_type(value)),
        }
    }
    fn to_value(&self) -> Value {
        Value::String(self.to_string())
    }
}

/// Implement the Timestamp scalar (replaces DateTime<Utc>)
///
/// The input/output is a string in RFC3339 format.
#[Scalar(
    internal,
    name = "DateTime",
    specified_by_url = "https://datatracker.ietf.org/doc/html/rfc3339"
)]
impl ScalarType for Timestamp {
    fn parse(value: Value) -> InputValueResult<Self> {
        match &value {
            Value::String(s) => Ok(s.parse::<Timestamp>()?),
            _ => Err(InputValueError::expected_type(value)),
        }
    }
    fn to_value(&self) -> Value {
        Value::String(self.to_string())
    }
}
