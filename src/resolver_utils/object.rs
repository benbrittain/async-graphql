use std::future::Future;

use indexmap::IndexMap;

use crate::{Context, Error, Name, OutputType, ServerError, ServerResult, Value};
#[cfg(feature = "boxed-trait")]
use crate::{ContextSelectionSet, Positioned, parser::types::Field};

/// Object-safe view of [`OutputType`], used only under the `boxed-trait`
/// feature.
///
/// [`OutputType`] cannot be made into a trait object because of its non-`self`
/// `type_name`/`create_type_info` methods. This shim exposes just `resolve`,
/// so the field-resolution drivers below can take `dyn DynOutput` and be
/// compiled once instead of being monomorphized per resolver return type.
#[cfg(feature = "boxed-trait")]
#[doc(hidden)]
pub trait DynOutput: Send + Sync {
    fn resolve<'a>(
        &'a self,
        ctx: &'a ContextSelectionSet<'a>,
        field: &'a Positioned<Field>,
    ) -> futures_util::future::BoxFuture<'a, ServerResult<Value>>;
}

/// Forwards the already-boxed future from `OutputType::resolve` as-is.
#[cfg(feature = "boxed-trait")]
impl<T: OutputType> DynOutput for T {
    fn resolve<'a>(
        &'a self,
        ctx: &'a ContextSelectionSet<'a>,
        field: &'a Positioned<Field>,
    ) -> futures_util::future::BoxFuture<'a, ServerResult<Value>> {
        OutputType::resolve(self, ctx, field)
    }
}

/// Helper used by proc-macro-generated object resolvers to reduce emitted code.
///
/// `boxed-trait` variant: the resolver future and its value are type-erased,
/// so this driver is compiled once instead of once per resolver method.
#[doc(hidden)]
#[cfg(feature = "boxed-trait")]
// NOTE: this is important to prevent resolve_field methods from growing too large,
// which can lead to stack overflows.
#[inline(never)]
pub async fn resolve_field_async<'a>(
    ctx: &'a Context<'a>,
    fut: futures_util::future::BoxFuture<'a, Result<Box<dyn DynOutput + 'a>, Error>>,
) -> ServerResult<Option<Value>> {
    let obj = fut.await.map_err(|err| {
        let err = err.into_server_error(ctx.item.pos);
        ctx.set_error_path(err)
    })?;

    let ctx_obj = ctx.with_selection_set(&ctx.item.node.selection_set);
    obj.resolve(&ctx_obj, ctx.item).await.map(Option::Some)
}

/// Helper used by proc-macro-generated object resolvers to reduce emitted code.
#[doc(hidden)]
#[cfg(not(feature = "boxed-trait"))]
#[allow(clippy::manual_async_fn)]
// NOTE: this is important to prevent resolve_field methods from growing too large,
// which can lead to stack overflows.
#[inline(never)]
pub fn resolve_field_async<'a, T, E, F>(
    ctx: &'a Context<'a>,
    fut: F,
) -> impl Future<Output = ServerResult<Option<Value>>> + Send + 'a
where
    T: OutputType + Send,
    E: Into<Error> + Send + Sync,
    F: Future<Output = Result<T, E>> + Send + 'a,
{
    async move {
        let obj: T = fut.await.map_err(|err| {
            let err = ::std::convert::Into::<Error>::into(err).into_server_error(ctx.item.pos);
            ctx.set_error_path(err)
        })?;

        let ctx_obj = ctx.with_selection_set(&ctx.item.node.selection_set);
        OutputType::resolve(&obj, &ctx_obj, ctx.item)
            .await
            .map(Option::Some)
    }
}

/// Helper used by proc-macro-generated object resolvers to parse entity params.
#[doc(hidden)]
pub fn find_entity_params<'a>(
    ctx: &'a Context<'a>,
    params: &'a Value,
) -> ServerResult<Option<(&'a IndexMap<Name, Value>, &'a String)>> {
    let params = match params {
        Value::Object(params) => params,
        _ => return Ok(None),
    };
    let typename = if let Some(Value::String(typename)) = params.get("__typename") {
        typename
    } else {
        return Err(ServerError::new(
            r#""__typename" must be an existing string."#,
            Some(ctx.item.pos),
        ));
    };
    Ok(Some((params, typename)))
}

/// Resolve a SimpleObject field value using the current selection set.
///
/// This is a small helper used by derive codegen to keep emitted resolver code
/// small. `boxed-trait` variant: the value is type-erased at the call site
/// (`&T` coerces to `&dyn DynOutput`), so this compiles once.
#[doc(hidden)]
#[cfg(feature = "boxed-trait")]
// NOTE: this is important to prevent resolve_field methods from growing too large,
// which can lead to stack overflows.
#[inline(never)]
pub async fn resolve_simple_field_value(
    ctx: &Context<'_>,
    value: &dyn DynOutput,
) -> ServerResult<Option<Value>> {
    value
        .resolve(
            &ctx.with_selection_set(&ctx.item.node.selection_set),
            ctx.item,
        )
        .await
        .map(Option::Some)
        .map_err(|err| ctx.set_error_path(err))
}

/// Resolve a SimpleObject field value using the current selection set.
///
/// This is a small helper used by derive codegen to keep emitted resolver code
/// small.
#[doc(hidden)]
#[cfg(not(feature = "boxed-trait"))]
// NOTE: this is important to prevent resolve_field methods from growing too large,
// which can lead to stack overflows.
#[inline(never)]
pub async fn resolve_simple_field_value<T: OutputType + ?Sized>(
    ctx: &Context<'_>,
    value: &T,
) -> ServerResult<Option<Value>> {
    OutputType::resolve(
        value,
        &ctx.with_selection_set(&ctx.item.node.selection_set),
        ctx.item,
    )
    .await
    .map(Option::Some)
    .map_err(|err| ctx.set_error_path(err))
}
