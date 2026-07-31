use crate::{
    ContextSelectionSet, OutputType, Positioned, ServerResult, Value, extensions::ResolveInfo,
    parser::types::Field,
};

/// Resolve an list by executing each of the items concurrently.
///
/// Under `boxed-trait`, only this thin shim is generic over the element type:
/// items are erased to `Box<dyn DynOutput>` and handed to the non-generic
/// driver below, so the resolution loops and the join machinery
/// (`try_join_all`, `FuturesOrdered`, ...) are compiled once.
#[cfg(feature = "boxed-trait")]
pub async fn resolve_list<'a, T: OutputType + 'a>(
    ctx: &ContextSelectionSet<'a>,
    field: &Positioned<Field>,
    iter: impl IntoIterator<Item = T>,
    len: Option<usize>,
) -> ServerResult<Value> {
    use crate::resolver_utils::DynOutput;

    let mut items: Vec<Box<dyn DynOutput + 'a>> = len.map(Vec::with_capacity).unwrap_or_default();
    items.extend(
        iter.into_iter()
            .map(|item| Box::new(item) as Box<dyn DynOutput + 'a>),
    );
    resolve_list_dyn(
        ctx,
        field,
        items,
        &Vec::<T>::type_name(),
        &T::qualified_type_name(),
    )
    .await
}

#[cfg(feature = "boxed-trait")]
async fn resolve_list_dyn<'a>(
    ctx: &ContextSelectionSet<'a>,
    field: &Positioned<Field>,
    items: Vec<Box<dyn crate::resolver_utils::DynOutput + 'a>>,
    parent_type: &str,
    return_type: &str,
) -> ServerResult<Value> {
    use futures_util::future::BoxFuture;

    let extensions = &ctx.query_env.extensions;
    if !extensions.is_empty() {
        let mut futures: Vec<BoxFuture<'_, ServerResult<Value>>> = Vec::with_capacity(items.len());
        for (idx, item) in items.into_iter().enumerate() {
            futures.push(Box::pin({
                let ctx = ctx.clone();
                async move {
                    let ctx_idx = ctx.with_index(idx);
                    let extensions = &ctx.query_env.extensions;

                    let resolve_info = ResolveInfo {
                        path_node: ctx_idx.path_node.as_ref().unwrap(),
                        parent_type,
                        return_type,
                        name: field.node.name.node.as_str(),
                        alias: field.node.alias.as_ref().map(|alias| alias.node.as_str()),
                        is_for_introspection: ctx_idx.is_for_introspection,
                        field: &field.node,
                    };
                    let resolve_fut = async {
                        item.resolve(&ctx_idx, field)
                            .await
                            .map(Option::Some)
                            .map_err(|err| ctx_idx.set_error_path(err))
                    };
                    futures_util::pin_mut!(resolve_fut);
                    extensions
                        .resolve(resolve_info, &mut resolve_fut)
                        .await
                        .map(|value| value.expect("You definitely encountered a bug!"))
                }
            }));
        }
        Ok(Value::List(
            futures_util::future::try_join_all(futures).await?,
        ))
    } else {
        let mut futures: Vec<BoxFuture<'_, ServerResult<Value>>> = Vec::with_capacity(items.len());
        for (idx, item) in items.into_iter().enumerate() {
            let ctx_idx = ctx.with_index(idx);
            futures.push(Box::pin(async move {
                item.resolve(&ctx_idx, field)
                    .await
                    .map_err(|err| ctx_idx.set_error_path(err))
            }));
        }
        Ok(Value::List(
            futures_util::future::try_join_all(futures).await?,
        ))
    }
}

/// Resolve an list by executing each of the items concurrently.
#[cfg(not(feature = "boxed-trait"))]
pub async fn resolve_list<'a, T: OutputType + 'a>(
    ctx: &ContextSelectionSet<'a>,
    field: &Positioned<Field>,
    iter: impl IntoIterator<Item = T>,
    len: Option<usize>,
) -> ServerResult<Value> {
    let extensions = &ctx.query_env.extensions;
    if !extensions.is_empty() {
        let mut futures = len.map(Vec::with_capacity).unwrap_or_default();
        for (idx, item) in iter.into_iter().enumerate() {
            futures.push({
                let ctx = ctx.clone();
                async move {
                    let ctx_idx = ctx.with_index(idx);
                    let extensions = &ctx.query_env.extensions;

                    let resolve_info = ResolveInfo {
                        path_node: ctx_idx.path_node.as_ref().unwrap(),
                        parent_type: &Vec::<T>::type_name(),
                        return_type: &T::qualified_type_name(),
                        name: field.node.name.node.as_str(),
                        alias: field.node.alias.as_ref().map(|alias| alias.node.as_str()),
                        is_for_introspection: ctx_idx.is_for_introspection,
                        field: &field.node,
                    };
                    let resolve_fut = async {
                        OutputType::resolve(&item, &ctx_idx, field)
                            .await
                            .map(Option::Some)
                            .map_err(|err| ctx_idx.set_error_path(err))
                    };
                    futures_util::pin_mut!(resolve_fut);
                    extensions
                        .resolve(resolve_info, &mut resolve_fut)
                        .await
                        .map(|value| value.expect("You definitely encountered a bug!"))
                }
            });
        }
        Ok(Value::List(
            futures_util::future::try_join_all(futures).await?,
        ))
    } else {
        let mut futures = len.map(Vec::with_capacity).unwrap_or_default();
        for (idx, item) in iter.into_iter().enumerate() {
            let ctx_idx = ctx.with_index(idx);
            futures.push(async move {
                OutputType::resolve(&item, &ctx_idx, field)
                    .await
                    .map_err(|err| ctx_idx.set_error_path(err))
            });
        }
        Ok(Value::List(
            futures_util::future::try_join_all(futures).await?,
        ))
    }
}
