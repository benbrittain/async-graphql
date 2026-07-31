#[cfg(feature = "boxed-trait")]
use std::sync::Arc;
use std::{borrow::Cow, pin::Pin};

use futures_util::stream::{Stream, StreamExt};

use crate::{
    Context, ContextSelectionSet, PathSegment, Response, ServerError, ServerResult,
    parser::types::Selection, registry, registry::Registry,
};
#[cfg(feature = "boxed-trait")]
use crate::{
    Data, Name, Positioned, QueryEnv, QueryPathNode, QueryPathSegment, Value,
    extensions::ResolveInfo, parser::types::Field, resolver_utils::DynOutput, schema::SchemaEnv,
};

/// A GraphQL subscription object
pub trait SubscriptionType: Send + Sync {
    /// Type the name.
    fn type_name() -> Cow<'static, str>;

    /// Qualified typename.
    fn qualified_type_name() -> String {
        format!("{}!", Self::type_name())
    }

    /// Create type information in the registry and return qualified typename.
    fn create_type_info(registry: &mut registry::Registry) -> String;

    /// This function returns true of type `EmptySubscription` only.
    #[doc(hidden)]
    fn is_empty() -> bool {
        false
    }

    #[doc(hidden)]
    fn create_field_stream<'a>(
        &'a self,
        ctx: &'a Context<'_>,
    ) -> Option<Pin<Box<dyn Stream<Item = Response> + Send + 'a>>>;
}

pub(crate) type BoxFieldStream<'a> = Pin<Box<dyn Stream<Item = Response> + 'a + Send>>;

/// Object-safe view of [`SubscriptionType`], used only under the `boxed-trait`
/// feature so the subscription stream collector can take `&dyn DynSubscription`
/// and be compiled once instead of monomorphized per subscription root.
///
/// [`SubscriptionType`] is not object-safe because of its non-`self`
/// `type_name`/`create_type_info` methods; this shim exposes just the instance
/// methods the collector needs.
#[cfg(feature = "boxed-trait")]
#[doc(hidden)]
pub trait DynSubscription: Send + Sync {
    fn create_field_stream<'a>(
        &'a self,
        ctx: &'a Context<'_>,
    ) -> Option<Pin<Box<dyn Stream<Item = Response> + Send + 'a>>>;

    /// The static GraphQL type name, carried as an instance method so it
    /// survives type erasure.
    fn type_name(&self) -> Cow<'static, str>;
}

#[cfg(feature = "boxed-trait")]
impl<T: SubscriptionType> DynSubscription for T {
    fn create_field_stream<'a>(
        &'a self,
        ctx: &'a Context<'_>,
    ) -> Option<Pin<Box<dyn Stream<Item = Response> + Send + 'a>>> {
        SubscriptionType::create_field_stream(self, ctx)
    }

    fn type_name(&self) -> Cow<'static, str> {
        <T as SubscriptionType>::type_name()
    }
}

#[cfg(not(feature = "boxed-trait"))]
pub(crate) fn collect_subscription_streams<'a, T: SubscriptionType + 'static>(
    ctx: &ContextSelectionSet<'a>,
    root: &'a T,
    streams: &mut Vec<BoxFieldStream<'a>>,
) -> ServerResult<()> {
    for selection in &ctx.item.node.items {
        if let Selection::Field(field) = &selection.node {
            streams.push(Box::pin({
                let ctx = ctx.clone();
                asynk_strim::stream_fn(move |mut yielder| async move {
                    let ctx = ctx.with_field(field);
                    let field_name = ctx.item.node.response_key().node.clone();
                    let stream = root.create_field_stream(&ctx);
                    if let Some(mut stream) = stream {
                        while let Some(resp) = stream.next().await {
                            yielder.yield_item(resp).await;
                        }
                    } else {
                        let err = ServerError::new(
                            format!(
                                r#"Cannot query field "{}" on type "{}"."#,
                                field_name,
                                T::type_name()
                            ),
                            Some(ctx.item.pos),
                        )
                        .with_path(vec![PathSegment::Field(field_name.to_string())]);
                        yielder.yield_item(Response::from_errors(vec![err])).await;
                    }
                })
            }))
        }
    }
    Ok(())
}

#[cfg(feature = "boxed-trait")]
pub(crate) fn collect_subscription_streams<'a>(
    ctx: &ContextSelectionSet<'a>,
    root: &'a dyn DynSubscription,
    streams: &mut Vec<BoxFieldStream<'a>>,
) -> ServerResult<()> {
    for selection in &ctx.item.node.items {
        if let Selection::Field(field) = &selection.node {
            streams.push(Box::pin({
                let ctx = ctx.clone();
                asynk_strim::stream_fn(move |mut yielder| async move {
                    let ctx = ctx.with_field(field);
                    let field_name = ctx.item.node.response_key().node.clone();
                    let stream = root.create_field_stream(&ctx);
                    if let Some(mut stream) = stream {
                        while let Some(resp) = stream.next().await {
                            yielder.yield_item(resp).await;
                        }
                    } else {
                        let err = ServerError::new(
                            format!(
                                r#"Cannot query field "{}" on type "{}"."#,
                                field_name,
                                root.type_name()
                            ),
                            Some(ctx.item.pos),
                        )
                        .with_path(vec![PathSegment::Field(field_name.to_string())]);
                        yielder.yield_item(Response::from_errors(vec![err])).await;
                    }
                })
            }))
        }
    }
    Ok(())
}

/// Wrap a subscription field's message stream so each message is resolved
/// into a [`Response`] through the extensions pipeline.
///
/// Used by proc-macro-generated `create_field_stream` under `boxed-trait`:
/// only this shim is generic over the stream; messages are erased to
/// `Box<dyn DynOutput>` and processed by the non-generic driver below, which
/// replaces ~70 lines of per-field expanded plumbing.
#[cfg(feature = "boxed-trait")]
#[doc(hidden)]
pub fn resolve_subscription_stream<'a, S>(
    schema_env: SchemaEnv,
    query_env: QueryEnv,
    field: Arc<Positioned<Field>>,
    field_name: Name,
    parent_type: String,
    return_type: String,
    stream: S,
) -> futures_util::stream::BoxStream<'a, ServerResult<Response>>
where
    S: Stream + Send + 'a,
    S::Item: crate::OutputType + 'a,
{
    resolve_subscription_stream_dyn(
        schema_env,
        query_env,
        field,
        field_name,
        parent_type,
        return_type,
        Box::pin(stream.map(|msg| Box::new(msg) as Box<dyn DynOutput + 'a>)),
    )
}

#[cfg(feature = "boxed-trait")]
fn resolve_subscription_stream_dyn<'a>(
    schema_env: SchemaEnv,
    query_env: QueryEnv,
    field: Arc<Positioned<Field>>,
    field_name: Name,
    parent_type: String,
    return_type: String,
    stream: Pin<Box<dyn Stream<Item = Box<dyn DynOutput + 'a>> + Send + 'a>>,
) -> futures_util::stream::BoxStream<'a, ServerResult<Response>> {
    Box::pin(stream.then(move |msg| {
        let schema_env = schema_env.clone();
        let query_env = query_env.clone();
        let field = field.clone();
        let field_name = field_name.clone();
        let parent_type = parent_type.clone();
        let return_type = return_type.clone();
        async move {
            let f = |execute_data: Option<Data>| {
                let schema_env = schema_env.clone();
                let query_env = query_env.clone();
                let field = field.clone();
                let field_name = field_name.clone();
                let parent_type = parent_type.clone();
                let return_type = return_type.clone();
                let msg = &msg;
                async move {
                    let ctx_selection_set = query_env.create_context(
                        &schema_env,
                        Some(QueryPathNode {
                            parent: None,
                            segment: QueryPathSegment::Name(&field_name),
                        }),
                        &field.node.selection_set,
                        execute_data.as_ref(),
                    );

                    let ri = ResolveInfo {
                        path_node: ctx_selection_set.path_node.as_ref().unwrap(),
                        parent_type: &parent_type,
                        return_type: &return_type,
                        name: field.node.name.node.as_str(),
                        alias: field.node.alias.as_ref().map(|alias| alias.node.as_str()),
                        is_for_introspection: false,
                        field: &field.node,
                    };
                    let resolve_fut =
                        async { msg.resolve(&ctx_selection_set, &field).await.map(Some) };
                    futures_util::pin_mut!(resolve_fut);
                    let mut resp = query_env
                        .extensions
                        .resolve(ri, &mut resolve_fut)
                        .await
                        .map(|value| {
                            let mut map = indexmap::IndexMap::new();
                            map.insert(field_name.clone(), value.unwrap_or_default());
                            Response::new(Value::Object(map))
                        })
                        .unwrap_or_else(|err| Response::from_errors(vec![err]));

                    resp.errors
                        .extend(std::mem::take(&mut *query_env.errors.lock().unwrap()));
                    resp
                }
            };
            Ok(query_env
                .extensions
                .execute(query_env.operation_name.as_deref(), f)
                .await)
        }
    }))
}

impl<T: SubscriptionType> SubscriptionType for &T {
    fn type_name() -> Cow<'static, str> {
        T::type_name()
    }

    fn create_type_info(registry: &mut Registry) -> String {
        T::create_type_info(registry)
    }

    fn create_field_stream<'a>(
        &'a self,
        ctx: &'a Context<'_>,
    ) -> Option<Pin<Box<dyn Stream<Item = Response> + Send + 'a>>> {
        T::create_field_stream(*self, ctx)
    }
}
