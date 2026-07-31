# Compile-Time Audit (boxed-trait)

An inventory of every compile-speed lever identified in async-graphql, assuming
the `boxed-trait` feature is enabled. Each finding documents the cause, the
evidence, and the fix. Findings are ordered by expected impact within each
section.

Two of the fixes (D1, D2) are implemented in the working tree and validated;
together they cut downstream schema-crate builds by ~36%.

## Methodology

Benchmark leaf crate: a generated 2,112-line schema — 60 `SimpleObject`s
(8 fields each), 40 `#[Object]`s (5 resolvers each, with arguments), 15 enums,
15 input objects, 5 unions, 3 interfaces, one `Query` root. Dev profile with
`debug = 0`, `CARGO_INCREMENTAL=0`, rustc 1.97 nightly, 32 cores. Generator:
`gen_schema.py` (session scratchpad).

Tools: `cargo llvm-lines`, `-Zmacro-stats`, `-Ztime-passes`, `cargo build
--timings`.

### Baseline (before fixes)

| Metric (leaf schema crate) | Value |
|---|---|
| `cargo check` | 7.89 s |
| `cargo build` (dev) | 8.97 s |
| LLVM IR lines | 943,309 |
| Monomorphized copies | 37,200 |
| Macro-expanded output | ~52 K lines / ~3.0 MB from 2.1 K source lines |

`-Ztime-passes` for the leaf crate (6.3 s total in rustc):

| Pass | Time |
|---|---|
| MIR borrow checking | 2.21 s |
| generate_crate_metadata | 2.14 s |
| monomorphization collector | 1.96 s |
| type checking | 0.75 s |
| LLVM passes + codegen to IR | ~1.07 s |
| macro expansion | 0.41 s |

The key observation: **the frontend dominates** (`check` is 88% of `build`).
Costs scale with (a) expanded token volume — borrowck, metadata, typecheck —
and (b) the monomorphization graph — collector, codegen, metadata. Every fix
below attacks one or both.

### After fixes D1 + D2 (in working tree)

| Metric | Baseline | After | Δ |
|---|---|---|---|
| `cargo check` | 7.89 s | 5.34 s | −32% |
| `cargo build` (dev) | 8.97 s | 5.76 s | −36% |
| LLVM IR lines | 943,309 | 500,068 | −47% |
| Monomorphized copies | 37,200 | 14,767 | −60% |

Full test suite passes with both patches (`cargo test --features boxed-trait --tests`).

---

## Part D — Downstream schema-crate costs

These are the multiplier costs: paid per GraphQL type / field in *every user
crate*, on *every* rebuild. They dominate real-world experience.

### D1. `resolve_list` monomorphizes the whole join machinery per element type — **fixed in tree**

**Cause.** `resolve_list<T>` (`src/resolver_utils/list.rs`) collected unboxed
per-item futures into a `Vec` and passed them to
`futures_util::future::try_join_all`. `try_join_all` drags `FuturesOrdered`,
`FuturesUnordered`, `ReadyToRunQueue`, `OrderWrapper`, `TryMaybeDone`, iterator
adapters, and drop glue with it — all monomorphized *per element type*, per
extensions/non-extensions branch. The deeply nested generic future types are
also expensive for the frontend (type folding in borrowck/metadata), which is
why `cargo check` improved too.

**Evidence.** In the baseline llvm-lines aggregation, the
`try_join_all`/`FuturesOrdered`/`FuturesUnordered` cluster accounted for
~240 K lines (~25% of the crate) across 80 copies of each component (40 list
types × 2 branches). Boxing alone: 943 K → 525 K lines (−44%).

**Fix (applied).** Under `boxed-trait`, push `BoxFuture<'_, ServerResult<Value>>`
instead of bare futures. `try_join_all::<Vec<BoxFuture>>` then instantiates
exactly once. One allocation per list item is added — consistent with the
boxed-trait trade-off, and lists already allocate per item elsewhere.

**Remaining.** The outer `resolve_list` body (two loops + `ResolveInfo`
plumbing) is still ~420 lines/copy per element type (~32 K lines in the
bench). Split it further: a per-`T` thin loop that only boxes
`OutputType::resolve(&item, ...)` futures, handing `Vec<BoxFuture>` plus
`type_name`/`qualified_type_name` strings to a non-generic driver that owns
the extensions/`ResolveInfo` logic. Est. additional −25 K lines.

### D2. `Registry::create_type<F>` duplicated per registration closure — **fixed in tree**

**Cause.** `create_type` (`src/registry/mod.rs`) was generic over
`F: FnMut(&mut Registry) -> MetaType`. Every derived type passes a unique
closure, so the whole body (name-conflict panic paths, `format!` machinery,
fake-type insertion) was monomorphized once per GraphQL type.

**Evidence.** ~20 K LLVM lines across 100 copies (~200 lines/copy) in the
baseline bench.

**Fix (applied).** Take `f: &mut dyn FnMut(&mut Registry) -> MetaType`. The
public generic wrappers (`create_output_type` etc.) auto-coerce; no caller
changes. Validated: −25 K lines on top of D1.

### D3. `#[async_trait]` re-expansion of every generated impl

**Cause.** Under `boxed-trait`, every derive attaches
`#[async_graphql::async_trait::async_trait]` to each generated
`ContainerType`/`OutputType`/`ComplexObject` impl (`gen_boxed_trait`,
`derive/src/utils.rs:379`). `async-trait` is itself a proc macro: it re-parses
the entire (already large) impl, rewrites each async fn into
`fn -> Pin<Box<dyn Future>>` with an inner async block, and re-emits
everything with extra lifetime bounds and `Send` assertions. The library's own
`DynContainer` blanket impl then wraps those boxed futures in *another*
`#[async_trait]` method — a second box per call (see D5).

**Evidence.** `-Zmacro-stats` on the bench crate: `#[async_trait]` fired 233
times, emitting 25,002 lines / 1.49 MB — half of all expanded output. Every
one of those bytes is parsed, name-resolved, type-checked, borrow-checked, and
serialized into crate metadata (the three passes that dominate the profile).

**Fix.** Stop using the `async-trait` proc macro in generated code. Under
`boxed-trait`, define the traits with explicit boxed signatures —

```rust
fn resolve_field<'a>(&'a self, ctx: &'a Context<'a>)
    -> BoxFuture<'a, ServerResult<Option<Value>>>;
```

— exactly as `SubscriptionType::create_field_stream` already does by hand
(`src/subscription.rs:30`), and have the derives emit
`Box::pin(async move { ... })` bodies directly. The derive knows every
signature statically; there is nothing async-trait computes that quote can't
emit directly. Benefits: kills the second proc-macro pass over ~700 KB of
tokens, roughly halves expanded token volume for the impls, removes the
`async-trait` dependency from generated code, and unlocks D4/D5.

**Expected gain.** The largest single remaining lever. Frontend passes scale
near-linearly with expanded volume; expect on the order of 20–30% off leaf
`cargo check` after D1/D2, plus the D4/D5 wins it enables.

### D4. Adapter impls (`&T`, `Box<T>`, `Arc<T>`, `Option<T>`, `Result<T, E>`) each build a new boxed state machine per hop

**Cause.** With `#[async_trait]` sugar, a forwarding impl like
`OutputType for Box<T>` (`src/base.rs:170`) compiles to a fresh async closure
+ `Box::pin` wrapping `T::resolve`'s already-boxed future. A common return
type like `Option<Box<Simple>>` pays the chain `Option<Box<T>> → Box<T> → T`,
each hop a separate per-type monomorphized state machine and a runtime
allocation.

**Evidence.** Bench (after D1/D2): `Option<Box<SimpleN>>::resolve` 8.1 K,
`&Option<Box<SimpleN>>::resolve` 5.9 K, `Box<SimpleN>::resolve` 5.9 K,
`Option<SimpleN>::resolve` 6.0 K, `&SimpleN::resolve` 3.9 K lines — ~30 K
lines of pure forwarding.

**Fix.** Requires D3. With explicit `-> BoxFuture` signatures, forwarding
impls become allocation-free one-liners that return the inner future
unchanged: `fn resolve(...) -> BoxFuture<...> { T::resolve(&**self, ctx, field) }`.
No new state machine, no monomorphized closure, no extra allocation.
(`Option<T>` genuinely branches, so it keeps a small body; the pointer
adapters become trivial.)

### D5. `DynContainer`/`DynSubscription` blanket shims double-box

**Cause.** The `boxed-trait` dyn-erasure shim (added in f865d3ee) implements
`DynContainer for T: ContainerType` via `#[async_trait]`, so
`DynContainer::resolve_field` allocates a new boxed future that awaits the
boxed future `ContainerType::resolve_field` already returns. Same for
`find_entity`.

**Evidence.** Per-type shim copies in the bench (after D1/D2):
`SimpleN as DynContainer::{resolve_field, find_entity}` ≈ 11.5 K lines,
`ComplexN` ≈ 7.7 K lines — ~19 K total, ~96 lines/copy, plus one wasted
allocation per resolved field at runtime.

**Fix.** Requires D3. Implement `DynContainer` manually and forward the boxed
future directly:

```rust
impl<T: ContainerType> DynContainer for T {
    fn resolve_field<'a>(&'a self, ctx: &'a Context<'a>)
        -> BoxFuture<'a, ServerResult<Option<Value>>> {
        ContainerType::resolve_field(self, ctx)
    }
    ...
}
```

Zero per-type async machinery; the shim reduces to vtable plumbing.

### D6. `resolve_field_async` monomorphized per resolver method

**Cause.** `resolve_field_async<T, E, F>`
(`src/resolver_utils/object.rs:13`) is generic over each resolver's unique
future type, so its whole body (error mapping, `with_selection_set`,
`OutputType::resolve` dispatch) is instantiated once per `#[Object]` field.
`#[inline(never)]` already keeps it out of `resolve_field` (stack-overflow
guard), but the copies remain.

**Evidence.** 200 copies ≈ 33 K lines (~167 lines/copy) in the bench.

**Fix.** Under `boxed-trait`, add an object-safe value-erasure trait (the
output analogue of `DynContainer`, e.g. `DynOutput: resolve(&self, ...) ->
BoxFuture + type_name(&self)`), and a non-generic
`resolve_field_async_dyn(ctx, fut: BoxFuture<'a, Result<Box<dyn DynOutput + 'a>, Error>>)`
driver. Generated code boxes the user's future and maps the value into
`Box<dyn DynOutput>`. Per-field cost drops to a thin boxing closure; the
driver compiles once in the library. Est. −25 K lines plus matching frontend
volume.

### D7. `create_type_info` emits imperative metadata-building code per type

**Cause.** Every derive expands schema registration into straight-line builder
code: for each field/argument a `MetaField::new(ToString::to_string(...))`
block plus one `field.x = ...;` statement per set attribute
(`derive/src/simple_object.rs:306`, `object.rs:644`, etc.). It runs once per
process at schema build, but is compiled on every build: ~425–566 LLVM
lines/type in the bench and a large share of `#[Object]`'s 17.4 KB and
`SimpleObject`'s 9.1 KB average expansion.

**Evidence.** After D1/D2 the `create_type_info` closures are the second
largest cluster: SimpleN 25.5 K + ComplexN 22.6 K + InputN 4.4 K ≈ 52 K lines.

**Fix.** Make registration data-driven. Derives emit `static` declarative
tables (`&'static str` names/descriptions, flags, fn pointers only where
values are computed, e.g. `create_type_info` of the field's type and default
values), and a single library function interprets the table into `MetaField`s.
Token volume per field collapses from ~30 lines of builder calls to one table
row; the interpreter compiles once. This also shrinks `generate_crate_metadata`
(2.1 s baseline), which serializes all this code for downstream crates.

### D8. Every `#[Object]` emits a `find_entity` override even with no entities

**Cause.** `derive/src/object.rs` unconditionally generates the
`find_entity` method (calling `find_entity_params` with an empty match body)
even though `ContainerType::find_entity` already has a default returning
`Ok(None)` (`src/resolver_utils/container.rs:77`). Under `boxed-trait`,
async-trait turns each into another boxed state machine.

**Evidence.** `ComplexN as ContainerType::find_entity` ≈ 3.7 K lines across
40 copies (~92 lines each) of dead code in the bench, plus its `DynContainer`
shim twin (counted in D5).

**Fix.** Only emit the override when `find_entities` is non-empty. One-line
condition in the derive; pure win for non-federation users.

### D9. `SimpleObject` generates a public async getter per field, used or not

**Cause.** For every field, the derive emits
`pub async fn field(&self, ctx: &Context<'_>) -> Result<&T>`
(`derive/src/simple_object.rs:360`). They exist so `#[derive(Interface)]`
delegation can call `obj.field(ctx).await` — but they're generated for every
SimpleObject, interface member or not. They never reach codegen when unused
(LLVM shows zero copies — DCE works), but they are parsed, type-checked,
borrow-checked, and serialized into crate metadata as public items; with 8
fields × 60 types that's 480 async fns of pure frontend overhead.

**Cause of shape.** They are `async` (and take `ctx`) only for signature
compatibility with `#[Object]` resolvers in interface dispatch; their bodies
never await.

**Fix options.** (a) Make getter emission opt-in/opt-out
(`#[graphql(getters)]` or emit only when the object is referenced by an
interface — not knowable locally, so an attribute is the practical route);
(b) keep them but emit non-async fns and have the Interface derive detect...
it can't. So: attribute opt-out, documented for large schemas — or accept as
the cost of interface support. Frontend-only win; worth having on 1000-type
schemas.

### D10. Subscription fields inline ~70 lines of stream plumbing each

**Cause.** `derive/src/subscription.rs:341-425` expands, per subscription
field, the full "wrap stream, clone envs, build `ResolveInfo`, run extensions,
assemble `Response`" machinery — three nested closures and two async blocks —
directly into `create_field_stream`.

**Fix.** Hoist it into a library helper generic only over the stream's item
type (or dyn-erased under `boxed-trait` via `DynOutput` from D6):
`resolve_subscription_stream<S: Stream>(ctx, field, stream, type_name) ->
impl Stream<Item = Response>`. The generated arm shrinks to argument parsing +
guard + one call. Subscription roots are usually small, so this is a lower
priority than D1–D7, but it's the same pattern.

### D11. `get_param_value<Q>` mixes non-generic work into a per-type instantiation

**Cause.** `ContextBase::get_param_value<Q: InputType>`
(`src/context.rs:625`) does argument lookup + variable resolution
(non-generic) before the only genuinely generic step, `InputType::parse`.
Instantiated per argument type per context kind.

**Fix.** Split: non-generic `fn lookup_param(&self, args, name) ->
ServerResult<(Pos, Option<Value>)>` plus a thin generic tail. Small win
(~100 lines/copy), trivial change.

---

## Part L — Library and cold-build costs

Paid once per clean build / CI run. Cold tree build: **10.3 s wall / 31.7 s
CPU** (32 cores). Biggest units: async-graphql 4.40 s, async-graphql-derive
1.92 s, syn 1.40 s, askama_parser 1.20 s, regex-automata 1.11 s, regex-syntax
1.07 s, futures-util 1.06 s, askama_derive 0.87 s, async-graphql-parser
0.77 s, darling_core 0.74 s, rustix 0.73 s.

### L1. `regex` is a hard dependency used by exactly one validator

**Cause.** `src/validators/regex.rs` — the `regex` input validator. Costs the
full `regex` + `regex-automata` + `regex-syntax` + `aho-corasick` stack
(~3.3 s CPU) on every cold build for everyone.

**Fix.** Either swap to `regex-lite` (drop-in for validator-sized patterns,
compiles in a fraction of the time) or feature-gate the validator
(`validators-regex`, on by default if ecosystem breakage is a concern, off if
v8 can take the break — it's already an rc).

### L2. `async-graphql-derive` depends on `async-graphql-parser` — and never uses it

**Cause.** Leftover dependency in `derive/Cargo.toml`. The only "references"
are `#crate_name::parser::types::...` token paths resolved in the *target*
crate. Because leaf-crate macro expansion can't start until the proc-macro dll
links, this puts `pest` + `async-graphql-value` + `serde_json` on the
blocking prefix of every downstream cold build.

**Evidence.** Verified: the derive crate compiles cleanly with the dependency
removed.

**Fix.** Delete the line.

### L3. GraphiQL/askama in default features

**Cause.** `default = ["dynamic-schema", "tempfile", "graphiql"]`. `graphiql`
pulls `askama` + `askama_derive` + `askama_parser` + `winnow` (~2.6 s CPU) to
render what is a mostly static HTML page with a handful of substitutions.

**Fix.** Replace askama with a plain `include_str!` template + small manual
placeholder substitution (the template inputs are simple scalars), or demote
`graphiql` from default features. Same consideration applies to `tempfile`
(pulls `rustix`, 0.73 s) and `dynamic-schema` (~6 K lines of the 34 K-line
crate) — defaults are convenient, but each is dead weight for production
servers that don't use them; documenting `default-features = false` setups is
the cheap version.

### L4. `syn` `extra-traits` + `strum` in the derive crate

**Cause.** `extra-traits` enables `Debug`/`Eq`/`Hash` impls for every syn AST
node — used here only for `#[derive(Debug)]` on internal args structs.
`strum::Display` is used for exactly two enums
(`derive/src/args.rs:1045,1070`), pulling `strum_macros` (0.47 s) into the
blocking proc-macro prefix.

**Fix.** Drop the `Debug` derives (or hand-write the two or three needed for
error paths) and the `extra-traits` feature; hand-write the two
SCREAMING_SNAKE_CASE `Display` impls. Shaves both syn's own compile and two
crates off the critical path.

### L5. `multer`/`encoding_rs` are hard dependencies

**Cause.** Multipart upload support (`src/http/multipart.rs`) is
unconditional; `multer` brings `encoding_rs` (0.56 s) and friends. Not every
server accepts uploads.

**Fix.** Feature-gate multipart (`multipart` in defaults to stay compatible).
Users disabling it drop ~1 s CPU of deps.

### L6. `pest`-based query parser

**Cause.** `async-graphql-parser` uses `pest` (0.47 s + grammar codegen), and
sits on the critical path before both the main crate and (until L2 lands) the
derive crate.

**Fix.** Long-term: a hand-written recursive-descent lexer/parser (GraphQL's
grammar is small and stable; this is also a runtime win). This is the biggest
change in Part L for the smallest per-build return — last priority.

---

## Part U — User-side mitigations (documentation candidates)

No library changes; worth documenting in the book's performance page:

- **`debug = 0`** (or `debug = "line-tables-only"`) in the dev profile — schema
  crates generate enormous debuginfo for async state machines.
- **Split large schemas across crates.** Per-type generated code is
  independent; 5 × 200-type crates compile in parallel and rebuild
  incrementally far better than 1 × 1000. `-Zshare-generics` (on by default in
  dev) lets leaf crates reuse the library's instantiations.
- **`cargo check` iteration** stays ~30% cheaper than `build` even after fixes.
- **Avoid deep `Box<A<Box<B<...>>>` type chains** in schema types: 60-level
  nesting hits `E0275` recursion-limit overflow in auto-trait solving (observed
  while constructing the benchmark).

---

## Suggested order of attack

| # | Item | Effort | Downstream gain (bench) | Status |
|---|---|---|---|---|
| 1 | D1 `resolve_list` boxing | S | −44% LLVM lines, −3.2 s build | ✅ in tree |
| 2 | D2 `create_type` dyn closure | S | −25 K LLVM lines | ✅ in tree |
| 3 | L2 drop parser dep from derive | S | cold-build prefix | verified |
| 4 | D8 skip empty `find_entity` | S | ~7 K lines + shims | — |
| 5 | L1 regex-lite / feature | S | −3.3 s CPU cold | — |
| 6 | L4 syn/strum diet | S | cold prefix | — |
| 7 | D3 de-async-trait generated impls | L | ~50% of expanded tokens; enables 8–9 | — |
| 8 | D4 forwarding adapters (needs D3) | M | ~30 K lines + runtime allocs | — |
| 9 | D5 `DynContainer` manual impl (needs D3) | S | ~19 K lines + 1 alloc/field | — |
| 10 | D6 `resolve_field_async` erasure | M | ~25 K lines | — |
| 11 | D7 table-driven `create_type_info` | L | ~50 K lines + metadata pass | — |
| 12 | D10 subscription helper | M | per-subscription-field | — |
| 14 | D9 getter opt-out, D11 param split | S | frontend on huge schemas | — |
| 15 | L6 replace pest | L | 0.5–1 s cold | — |

S/M/L = small/medium/large implementation effort.
