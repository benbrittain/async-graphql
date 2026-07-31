use std::borrow::Cow;

use crate::{
    ContextSelectionSet, InputType, InputValueError, InputValueResult, OutputType, Positioned,
    ServerResult, Value, parser::types::Field, registry,
};

impl<T: InputType> InputType for Option<T> {
    type RawValueType = T::RawValueType;

    fn type_name() -> Cow<'static, str> {
        T::type_name()
    }

    fn qualified_type_name() -> String {
        T::type_name().to_string()
    }

    fn create_type_info(registry: &mut registry::Registry) -> String {
        T::create_type_info(registry);
        T::type_name().to_string()
    }

    fn parse(value: Option<Value>) -> InputValueResult<Self> {
        match value.unwrap_or_default() {
            Value::Null => Ok(None),
            value => Ok(Some(
                T::parse(Some(value)).map_err(InputValueError::propagate)?,
            )),
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Some(value) => value.to_value(),
            None => Value::Null,
        }
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        match self {
            Some(value) => value.as_raw_value(),
            None => None,
        }
    }
}

impl<T: OutputType + Sync> OutputType for Option<T> {
    fn type_name() -> Cow<'static, str> {
        T::type_name()
    }

    fn qualified_type_name() -> String {
        T::type_name().to_string()
    }

    fn create_type_info(registry: &mut registry::Registry) -> String {
        T::create_type_info(registry);
        T::type_name().to_string()
    }

    #[cfg(feature = "boxed-trait")]
    fn resolve<'life0, 'life1, 'life2, 'life3, 'async_trait>(
        &'life0 self,
        ctx: &'life1 ContextSelectionSet<'life2>,
        field: &'life3 Positioned<Field>,
    ) -> ::std::pin::Pin<
        Box<dyn ::std::future::Future<Output = ServerResult<Value>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        'life3: 'async_trait,
        Self: 'async_trait,
    {
        use futures_util::FutureExt;

        match self {
            // Forward the inner boxed future through a lightweight `map`
            // instead of building a second boxed state machine per type.
            Some(inner) => Box::pin(OutputType::resolve(inner, ctx, field).map(
                move |res| match res {
                    Ok(value) => Ok(value),
                    Err(err) => {
                        ctx.add_error(err);
                        Ok(Value::Null)
                    }
                },
            )),
            None => Box::pin(futures_util::future::ready(Ok(Value::Null))),
        }
    }

    #[cfg(not(feature = "boxed-trait"))]
    async fn resolve(
        &self,
        ctx: &ContextSelectionSet<'_>,
        field: &Positioned<Field>,
    ) -> ServerResult<Value> {
        if let Some(inner) = self {
            match OutputType::resolve(inner, ctx, field).await {
                Ok(value) => Ok(value),
                Err(err) => {
                    ctx.add_error(err);
                    Ok(Value::Null)
                }
            }
        } else {
            Ok(Value::Null)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::InputType;

    #[test]
    fn test_optional_type() {
        assert_eq!(Option::<i32>::type_name(), "Int");
        assert_eq!(Option::<i32>::qualified_type_name(), "Int");
        assert_eq!(&Option::<i32>::type_name(), "Int");
        assert_eq!(&Option::<i32>::qualified_type_name(), "Int");
    }
}
