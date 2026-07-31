#[cfg(test)]
mod no_getters {
    use async_graphql::*;

    #[derive(SimpleObject)]
    #[graphql(no_getters)]
    struct NoGetters {
        a: i32,
        b: String,
    }

    struct Query;

    #[Object]
    impl Query {
        async fn obj(&self) -> NoGetters {
            NoGetters {
                a: 7,
                b: "x".into(),
            }
        }
    }

    #[tokio::test]
    async fn resolves_without_getters() {
        let schema = Schema::new(Query, EmptyMutation, EmptySubscription);
        let resp = schema.execute("{ obj { a b } }").await;
        assert_eq!(resp.data, value!({ "obj": { "a": 7, "b": "x" } }));
    }
}
