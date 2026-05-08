# Changelog

All notable changes to `deribit-mcp` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Trading-tool dispatch routing (v0.6-03):
  - `handle_place_order` and `handle_cancel_order` now branch on
    `ctx.config.order_transport` with an exhaustive match. With
    `--order-transport=http` the existing v0.4 path runs
    unchanged; with `--order-transport=fix` the calls go through
    the lazy `AdapterContext::ensure_fix()` session.
  - `place_order_via_fix` builds a
    `deribit_fix::model::request::NewOrderRequest` from the same
    `PlaceOrderInput`, drives `DeribitFixClient::send_order`,
    and synthesizes a minimal MCP `OrderResponse`-shaped JSON
    (`{"order": {...}, "trades": []}`) so the wire surface is
    identical across both transports. The synthesized payload
    carries a `"transport": "fix"` marker so the LLM can tell
    where the response came from.
  - `cancel_order` over FIX dispatches
    `DeribitFixClient::cancel_order(order_id)` and returns
    `{"order_id": <id>, "order_state": "cancelled", "transport":
    "fix"}` — `deribit-fix 0.3` resolves `cancel_order` to
    `()` so the adapter assembles the response shape.
  - `cancel_all_by_currency` and `cancel_all_by_instrument`
    intentionally always dispatch through HTTP. When the
    configured transport is `Fix` the adapter logs at WARN that
    the active FIX session was bypassed; `deribit-fix 0.3` does
    not expose a mass-cancel helper.
  - Schema snapshots are byte-identical between
    `--order-transport=http` and `--order-transport=fix` runs;
    the MCP tool surface is unchanged.
  - Tests:
    `build_fix_new_order_request_round_trips_buy_limit`,
    `build_fix_new_order_request_maps_stop_limit_with_trigger`,
    `build_fix_new_order_request_overflow_rejects_valid_until`,
    `synthesize_fix_order_response_matches_documented_shape`.

- FIX session lifecycle in the adapter (v0.6-02):
  - New `deribit-fix = "0.3"` dependency, gated behind a default-on
    `fix` Cargo feature. Disabling the feature drops the FIX wiring
    entirely so a constrained build still compiles.
  - `AdapterContext::fix: OnceCell<Arc<Mutex<DeribitFixClient>>>`
    — lazy. Constructed on first `ensure_fix()` call.
  - `AdapterContext::ensure_fix()` short-circuits with a
    structured `AdapterError::Validation { field:
    "order_transport" }` when the configuration selects `Http`,
    and with `field: "credentials"` when no client_id /
    client_secret are configured. Otherwise it builds the
    upstream `DeribitFixConfig` (host = `fix-test.deribit.com`
    on testnet / `fix.deribit.com` on mainnet, port `9881`,
    plain TCP), drives `DeribitFixClient::new` +
    `client.connect()` (FIX `Logon (A)` + heartbeat task), and
    returns the shared handle.
  - `AdapterContext::shutdown_fix()` is the SIGTERM-side hook —
    no-op when the session was never opened, otherwise issues a
    proper FIX `Logout (5)` via the upstream `disconnect`. Wired
    into `main()` after both the stdio and HTTP branches return.
  - Manual `Debug` impl for `AdapterContext` so the upstream
    `DeribitFixClient` (which does not derive `Debug`) doesn't
    leak into the bound; the FIX field is rendered as a redacted
    `<fix client>` placeholder.
  - New `From<deribit_fix::error::DeribitFixError> for
    AdapterError` mapping with an exhaustive match on the
    upstream enum:
    - `Authentication` → `AdapterError::Auth { reason }` (via
      the existing `classify_auth_failure_reason`).
    - `Connection` / `Io` → `UpstreamErrorKind::Fix { kind:
      Disconnected, message }`.
    - `Session` / `Protocol` / `MessageParsing` /
      `MessageConstruction` → `Fix { kind: SessionReject }`.
    - `Config` → `Fix { kind: Config }`.
    - `Timeout` / `Generic` / `Json` / `Http` → `Fix { kind:
      Other }`.
  - New `UpstreamErrorKind::Fix` variant + `FixErrorKind` enum
    (`Disconnected`, `SessionReject`, `Config`, `Other`) — both
    behind `cfg(feature = "fix")`.
  - Tests:
    `ensure_fix_when_transport_is_http_returns_validation`,
    `ensure_fix_without_credentials_returns_validation`,
    `shutdown_fix_when_never_opened_is_noop`.

- `--order-transport=http|fix` CLI flag + config plumbing
  (v0.6-01):
  - New `Config::order_transport: OrderTransport` field with
    `Http` (default — v0.1..v0.5 behaviour) and `Fix` variants.
    The startup guard uses an exhaustive `match` so adding a
    future variant fails to compile until the gating is
    revisited.
  - CLI: `--order-transport=http|fix`.
  - Env: `DERIBIT_ORDER_TRANSPORT=http|fix`.
  - Resolution: CLI > env > default `Http`.
  - Startup guard: `Config::load` refuses to build when
    `order_transport == Fix` and `allow_trading == false` so
    the adapter does not start in a state where the FIX session
    would never be reached. Surfaces as a structured `anyhow`
    error from `main()` exit-code 1.
  - The actual FIX session lifecycle and trading-tool dispatch
    routing land in v0.6-02 / v0.6-03.

- `position_review` MCP prompt (v0.5-04):
  - Drives the LLM through an end-of-day account / position
    review using the v0.2 Account tools (`get_account_summary`,
    `get_positions`, `get_open_orders_by_currency`, optionally
    `get_user_trades_by_currency`). Pins the output sections
    (headline, positions, open orders, flags, optional recent
    trades, caveats).
  - Arguments: `currency: String` (required, case-normalised),
    `include_history: bool` (defaults `false`).
  - Credentials-aware: when the adapter has no credentials
    (`AdapterContext::has_credentials() == false`), the prompt
    returns a structured `WARNING:`-prefixed body that explains
    the Account tools are not registered and instructs the LLM
    not to attempt any tool call. With credentials present, the
    body names the four Account tools and pins the output
    structure.
  - Prompt-descriptor schema snapshot
    (`tests/snapshots/schema__prompt_descriptors.snap`) updated;
    integration tests cover both branches:
    `prompts_get_position_review_with_credentials_lists_account_tools`,
    `prompts_get_position_review_without_credentials_emits_warning`.

- `funding_snapshot` MCP prompt (v0.5-03):
  - Curated User + Assistant message pair that walks the LLM
    through assembling a current + historical perpetual
    funding-rate snapshot. Names `list_instruments` (filtered
    to `kind: "future"`, `expired: false`) and
    `get_funding_rate_history` and pins the output sections
    (latest, mean / median / p10 / p90, sign breakdown,
    outliers, caveats).
  - Arguments: `currency` (`BTC` / `ETH`, case-normalised),
    `lookback_hours: u32` in `1..=720` (30 days).
  - Prompt-descriptor schema snapshot
    (`tests/snapshots/schema__prompt_descriptors.snap`) extended
    with the new descriptor; integration test
    `prompts_get_funding_snapshot_returns_well_formed_messages`
    asserts the body references `BTC-PERPETUAL` and
    `get_funding_rate_history`.

- `daily_options_summary` MCP prompt (v0.5-02):
  - Curated User + Assistant message pair that walks the LLM
    through "summarise BTC / ETH options expiring in the next N
    days". Composes the public Read tools `list_instruments`,
    `get_book_summary_by_currency`, `get_historical_volatility`
    — the prompt names them and constrains the output sections;
    it does not call them itself.
  - Arguments: `currency: String` (`BTC` / `ETH` only;
    case-normalised), `horizon_days: u8` (range 1..=31).
    Failures surface as
    `AdapterError::Validation { field, message }`.
  - New schema snapshot `tests/snapshots/schema__prompt_descriptors.snap`
    pins the `prompts/list` wire shape.
  - Integration tests:
    `prompts_get_daily_options_summary_returns_well_formed_messages`,
    `prompts_get_daily_options_summary_rejects_invalid_currency`.

- MCP `prompts` capability + `PromptRegistry` (v0.5-01):
  - New `src/prompts/mod.rs` with `PromptRegistry`,
    `PromptEntry`, and the `PromptHandlerFn` Fn-pointer type.
    Mirrors the `tools` registry pattern: build once at startup,
    frozen for the process lifetime, dispatch by name.
  - `DeribitMcpServer::server_info()` now advertises the
    `prompts` capability via
    `ServerCapabilities::builder().enable_prompts()`.
    `listChanged` is left unset (omitted from the wire JSON) —
    the registry is frozen.
  - `ServerHandler::list_prompts` returns the sorted entry list;
    `ServerHandler::get_prompt` dispatches via
    `PromptRegistry::get`. A registry miss (`name` not
    registered) returns `AdapterError::Validation { field:
    "name" }` which the rmcp boundary translates into a
    structured error.
  - `lib.rs` re-exports `PromptRegistry`, `PromptEntry`,
    `PromptHandlerFn`.
  - Unit + integration tests cover the empty-registry list and
    the unknown-prompt rejection paths. The three concrete
    prompts (`daily_options_summary`, `funding_snapshot`,
    `position_review`) ship in the v0.5-02 / v0.5-03 / v0.5-04
    issues.

- Trading integration coverage + live smoke (v0.4-05):
  - `From<HttpError> for AdapterError` now extracts the
    `"API error: <code> - <message>"` payload that
    `deribit-http` surfaces through `RequestFailed` and routes
    it to `UpstreamErrorKind::Api { code: Some(<i64>), message }`
    instead of an opaque `Http { message }`. The LLM now sees
    the structured Deribit error code (e.g. `11044
    not_open_order`) for failed trading calls.
  - New integration scenarios in `tests/integration.rs`:
    - `trading_tools_register_all_with_allow_trading` —
      asserts every Trading tool is registered when
      credentials and `--allow-trading` are configured.
    - `cancel_order_upstream_order_not_found_maps_to_api_code_11044`
      — asserts the new HttpError → Api { code } mapping.
    - `cancel_all_by_currency_zero_count_returns_zero` — pins
      the empty-result shape.
  - `tests/sandbox_smoke.rs` extension `live_testnet_trading_smoke`
    — opt-in via `DERIBIT_MCP_TRADING_SMOKE=1` (separate from
    the read-only `DERIBIT_MCP_SMOKE`). Places a deeply OTM
    post-only limit order on `BTC-PERPETUAL` against
    `test.deribit.com`, then cancels it. `--max-order-usd=100`
    safety net stays armed so a misconfigured run cannot place
    a meaningful order.

- `--max-order-usd` notional cap enforcement (v0.4-04):
  - `enforce_size_cap` runs in `place_order` after schema
    validation, before any private order-placement call. When
    the cap is unset the function is a no-op.
  - Notional formula:
    - Linear (`*_USDC*` / `*_USDT*`): `amount × price`. Market
      orders without a caller `price` fetch upstream
      `mark_price` via `/public/ticker`.
    - Option (final segment is `-C` / `-P` with a numeric
      strike): `amount × index_price`, where `index_price`
      comes from `/public/get_index_price?index_name=<base>_usd`.
      Pins the conservative upper bound of underlying USD
      exposure.
    - Inverse (everything else — BTC- / ETH-denominated futures
      and perpetuals): `amount` is already USD-notional per
      Deribit's contract-size convention, so the notional is
      `amount` directly.
  - Failures surface as
    `AdapterError::SizeCapExceeded { requested, cap }`.
  - New unit + integration tests:
    `linear_instrument_classification`,
    `option_instrument_classification`, `cap_unset_is_noop`,
    `inverse_notional_uses_amount`, `inverse_under_cap_passes`,
    `linear_notional_uses_price_times_amount`,
    `place_order_over_size_cap_rejected_before_network`,
    `place_order_linear_market_fetches_mark_price_for_cap`.

- `cancel_all_by_currency` and `cancel_all_by_instrument`
  Trading-class tools (v0.4-03):
  - `cancel_all_by_currency { currency }` →
    `deribit_http::cancel_all_by_currency`. Returns
    `{"cancelled": <count>}`.
  - `cancel_all_by_instrument { instrument_name }` →
    `deribit_http::cancel_all_by_instrument`. Returns the same
    shape.
  - Tool descriptions warn the LLM that the call is irreversible
    and cancels every matching open order.
  - The `kind` / `type` filter mentioned in the issue brief is
    not exposable yet — `deribit-http 0.7` does not surface
    those parameters on the per-currency / per-instrument
    endpoints (only on `cancel_all_by_kind_or_type`). Filtering
    will land alongside the upstream parameter expansion;
    documented as a deferred follow-up.
  - Trading schema snapshot updated; new integration tests
    `cancel_all_by_currency_dispatches_through_registry` and
    `cancel_all_by_instrument_dispatches_through_registry`
    cover the happy path against `mockito`.

- `edit_order` and `cancel_order` Trading-class tools (v0.4-02):
  - `edit_order` modifies `amount`, `price`, `post_only`,
    `reject_post_only`, `reduce_only`, `mmp`, `valid_until`,
    `trigger_price` of an existing order identified by
    `order_id`. All non-`order_id` fields are optional — `None`
    leaves the upstream value unchanged. Backed by
    `deribit_http::edit_order`. Validates non-empty `order_id`
    and finite-positive `amount` / `price` / `trigger_price`
    when supplied.
  - `cancel_order` cancels one open order by id. Backed by
    `deribit_http::cancel_order`. Rejects empty / whitespace
    `order_id` with `AdapterError::Validation`.
  - Both tools effect-class `Trading`; register only when
    credentials are configured AND `--allow-trading` is set.
  - Trading schema snapshot
    `tests/snapshots/schema__trading_tool_input_schemas.snap`
    extended with the new entries; integration tests cover the
    happy path against `mockito` + the validation-before-network
    failure mode for empty `order_id`. Upstream `OrderNotFound`
    surfaces verbatim through the existing
    `From<HttpError> for AdapterError` mapping (already covered
    by the v0.2 error-mapping unit tests).

- `place_order` Trading-class tool (v0.4-01):
  - Buy / sell with the full Deribit order parameter surface
    (`limit`, `market`, `stop_limit`, `stop_market`, `take_limit`,
    `take_market`, `market_limit`; optional `time_in_force`,
    `trigger`, `post_only`, `reject_post_only`, `reduce_only`,
    `mmp`, `valid_until`, `label`).
  - Adapter-side validation runs before the network call:
    `amount` finite & `> 0`, `price` / `trigger_price` finite &
    `> 0` when supplied, `price` required for limit / stop_limit
    / take_limit, `trigger_price` + `trigger` required for stop /
    take variants. Failures surface as
    `AdapterError::Validation { field, message }`.
  - Closed-set local enums (`Side`, `PlaceOrderType`,
    `PlaceTimeInForce`, `PlaceTrigger`) with `snake_case` JSON
    wire names — exhaustive match on dispatch / build, no
    wildcard arms.
  - `Side::Buy` → `deribit_http::buy_order`,
    `Side::Sell` → `deribit_http::sell_order`. Effect class
    `Trading`; registers only when `--allow-trading` is set
    *and* credentials are configured (ADR-0010).
  - Tool description tells the LLM the call places a real order
    on the configured Deribit endpoint.
  - New schema snapshot
    `tests/snapshots/schema__trading_tool_input_schemas.snap`
    pins the wire shape; integration tests in
    `tests/integration.rs` cover the round-trip against
    `mockito` and the validation-before-network path.

- Reconnect / resume integration tests (`tests/integration_live.rs`)
  — v0.3-07:
  - `MockWsServer::subscribe_count()` /
    `MockWsServer::unsubscribe_count()` — atomic counters so
    tests can assert the upstream observed exactly N
    `public/subscribe` / `public/unsubscribe` calls across the
    session.
  - `MockWsServer::shutdown()` — fires the cancellation token so
    the listener stops accepting new connections and active
    per-connection tasks send a close frame within a few ms;
    deterministic trigger for reconnect-style scenarios.
  - Four scenarios in `tests/integration_live.rs`:
    `one_client_one_subscribe` (scenario 1 — exactly one
    upstream subscribe per first read),
    `two_clients_each_send_their_own_subscribe` (scenario 2 — two
    distinct WS connections each subscribe once),
    `explicit_unsubscribe_increments_unsubscribe_count` (scenario
    3 — explicit unsubscribe surfaces on the mock),
    `mock_shutdown_terminates_client_stream` (scenario 4 — mock
    shutdown ends the client stream within 2 s, the trigger a
    real `WsSubscriptionProvider` would reconnect on).
  - LiveRegistry-level refcount-reuse / refcount-teardown
    semantics (scenarios 2 & 3 of the issue brief at the
    adapter layer) remain covered by the lib-level
    `second_subscribe_reuses_entry` /
    `dropping_last_handle_removes_entry` tests in
    `src/resources/live.rs`. Scenarios 5 & 6 — resume
    notifications across reconnect and `SubscriptionLost`
    surfacing — are deferred until the real
    `WsSubscriptionProvider` over `deribit-websocket` lands;
    the mock-side trigger from scenario 4 is the contract they
    will be written against.

- Mock WebSocket server (`tests/support/mock_ws.rs`) — v0.3-06:
  - `MockWsServer::start()` binds an ephemeral `127.0.0.1:0` port
    and accepts WS upgrades. `ws_url()` returns the
    `ws://…/ws/api/v2` URL clients connect to.
  - Per-test instance, no global state. Drop fires the
    cancellation token; the listener task aborts within a few
    ms.
  - Honours the subset of the Deribit JSON-RPC WS protocol the
    adapter cares about: `public/auth` → canned token,
    `public/subscribe` / `public/unsubscribe` → ack with
    channel list (tracked per connection), `public/set_heartbeat`
    / `public/test` → no-op `ok` ack. Anything else → `result:
    null`.
  - `push_frame(channel, data)` wraps the payload in the
    standard `subscription` envelope and relays it to every
    connected client that has subscribed to the channel.
  - New dev-dep: `tokio-tungstenite = "0.29"` (same major as
    `deribit-websocket` already pulls in, so the test binary
    does not add a second copy).
  - Coverage: `tests/integration_live.rs` exercises the mock
    end-to-end (auth handshake, scripted book frame relay,
    unsubscribe stops the relay).
  - `tests/support/README.md` documents the helper API.
- Subscription update notifications + throttle (v0.3-05):
  - New `NotificationSink` trait. The live registry calls
    `sink.notify(&uri)` post-throttle every time a subscribed
    URI produces a new snapshot. `LiveRegistry::set_notification_sink(...)`
    installs / detaches the sink at runtime; the MCP server impl
    is expected to wire this to rmcp's `Peer::notify_resource_updated`
    (`rmcp::service::server`) so connected clients receive
    `notifications/resources/updated` per the MCP 2025-06-18 spec.
  - `DEFAULT_NOTIFY_INTERVAL = 100 ms` (≈ 10 Hz). The
    coalescing throttle suppresses intermediate frames within
    the window; the next `resources/read` returns the latest
    snapshot. `LiveRegistry::set_notify_interval(d)` overrides
    the cadence, including `Duration::ZERO` to disable
    throttling entirely.
  - `SubscriptionEntry` carries a per-URI `last_notified`
    `Instant` so the throttle is per-resource (a busy book
    channel does not starve a quiet ticker channel).
- Live resource — `deribit://trades/{instrument}` (v0.3-04):
  - `TradeUpdate` DTO carrying `direction`, `price`, `amount`,
    `trade_id`, `timestamp`, plus optional `liquidation`,
    `tick_direction`, `mark_price`, `index_price`.
  - `TradeUpdate::batch_from_value(raw)` decodes one
    `trades.<instrument>.raw` channel frame (a JSON array of
    trade objects); elements that fail to decode are dropped
    individually rather than failing the whole call.
  - `LiveRegistry::SubscriptionEntry` gains a rolling `history:
    Mutex<VecDeque<Value>>` capped at `HISTORY_CAPACITY = 32`
    frames. The reader task pushes every received frame into both
    `latest` (book / ticker) and `history` (trades). New
    `SubscriptionHandle::history()` accessor.
  - `ResourceRegistry::read(Trades)` subscribes, awaits the first
    frame, then flattens the history into a chronological
    newest-first list of `TradeUpdate`s, capped at 32.
- Live resource — `deribit://ticker/{instrument}` (v0.3-03):
  - `TickerSnapshot` DTO carrying `mark_price`, `index_price`,
    `best_bid_price`, `best_ask_price`, `last_price`, `mark_iv`,
    `delta`, `gamma`, `vega`, `timestamp`. Optional fields stay
    `None` when the upstream omits them — perpetuals / futures
    do not carry the greeks.
  - `TickerSnapshot::from_value(instrument, raw)` decoder unwraps
    the inner `greeks` object (delta / gamma / vega) up to the
    top-level shape so the JSON the LLM sees is flat.
  - `ResourceRegistry::read(Ticker)` reuses the v0.3-02 live
    plumbing (subscribe → first-frame timeout → decode →
    return).
  - `Trades` placeholder reason adjusted to point at v0.3-04.
- Live resource — `deribit://book/{instrument}` (v0.3-02):
  - `BookSnapshot` DTO (`instrument`, `bids`, `asks`, `change_id`,
    `timestamp`) with permissive
    `BookSnapshot::from_value(instrument, raw)` decoder. Handles
    `[op, price, size]` (delta) and `[price, size]` (snapshot)
    upstream level shapes; missing optional fields default rather
    than failing the call.
  - `ResourceRegistry::with_subscription_provider(provider)` injects
    a `SubscriptionProvider` (real wiring lands when the binary
    startup hooks `deribit-websocket`; tests pass a stub).
  - `ResourceRegistry::read` for `Book` subscribes via the
    [`LiveRegistry`], waits up to `FIRST_FRAME_TIMEOUT` (5 s) for
    the first frame, decodes into a `BookSnapshot`, and returns
    `ResourceContent::Json`. Without a provider it returns
    `AdapterError::Internal { reason: "live subscription provider
    not configured" }`.
  - `channel_name_for(Book)` switched from the speculative
    `100ms` aggregation to the documented `book.<i>.raw` channel
    per the v0.3-02 spec.
  - Live-template descriptions updated to spell out the new
    behaviour.
- Live resource registry + lifecycle (`src/resources/live.rs`):
  - `LiveRegistry` keys per `ResourceUri`. First subscriber opens
    the upstream channel via the new `SubscriptionProvider`
    trait; subsequent subscribers reuse the cached entry
    (refcount++).
  - `SubscriptionHandle` decrements the refcount on `Drop`. When
    the count returns to zero the per-entry
    `tokio_util::sync::CancellationToken` is fired, the upstream
    reader task exits, and the entry is removed from the map.
  - `SubscriptionEntry` carries the upstream channel name, latest
    decoded snapshot (`tokio::sync::Mutex<Option<Value>>`), a
    per-entry `broadcast::Sender<()>` for update fan-out
    (capacity 64), and the cancel token.
  - Refcount uses checked arithmetic — `fetch_add(1)` is asserted
    against `u64::MAX` overflow; `fetch_sub` paths only tear down
    when `prev == 1` so an under-decrement could not silently
    succeed.
  - `SubscriptionProvider` trait abstracts the v0.3-02 / v0.3-04
    real `deribit-websocket` wiring so the registry has a stub-
    backed test surface.
  - `ResourceUri` now derives `Hash` to live in the per-channel
    map.
  - New deps: `futures-core` + `futures-util` (stream combinators
    for the reader task).
- Sandbox smoke test (`tests/sandbox_smoke.rs`):
  - Drives `get_server_time`, `get_ticker BTC-PERPETUAL`,
    `get_account_summary BTC`, `get_positions BTC` end-to-end
    against `test.deribit.com`.
  - `#[ignore]` by default; CI never runs it.
  - Skip-not-fail when `DERIBIT_MCP_SMOKE` is unset, or when
    `DERIBIT_CLIENT_ID` / `DERIBIT_CLIENT_SECRET` are missing —
    `--ignored` reruns on a developer laptop without secrets do
    not turn red.
  - Every upstream call wrapped in `tokio::time::timeout` (30 s)
    so the test never hangs on a stalled network.
  - No `eprintln!` of response bodies / credentials; assertion
    failure messages render only a `shape_of` summary (kind +
    array len / object key count) so a panicking diagnostic does
    not dump testnet account fields. The adapter's tracing
    redaction layer (v0.1-03) keeps secret material out of logs
    even at TRACE.
- OAuth wiring through `deribit-http` (`src/context.rs`,
  `src/error.rs`):
  - `AdapterContext::auth_state()` returns a typed
    [`AuthState::Anonymous`] / [`AuthState::Configured`] enum so
    the registry / dispatcher decide whether `Account` / `Trading`
    tools register at all.
  - `http_config_from` now forwards `client_id` / `client_secret`
    from our resolved `Config` into the upstream
    `HttpConfig.credentials`. Removes the dependency on dotenvy
    populating the process env before `DeribitHttpClient` reads
    its own defaults.
  - First private call triggers the upstream `AuthManager`'s
    OAuth client-credentials flow lazily, caches the token, and
    refreshes ~30 s before `expires_in` (handled inside
    `deribit-http`).
  - `AuthFailureReason` reshaped for v0.2-05 — the v0.1
    placeholder set is replaced with the closed set the spec
    requires:
    - `MissingCredentials` (env not set).
    - `Unauthorized` (HTTP 401 / upstream code `10004`).
    - `TokenExpiredAndRefreshFailed` (`13004` / `invalid_token`
      / `token has expired`).
    - `Suspended` (account suspended / KYC hold / regulatory; code
      `10005`).
    - `ScopeInsufficient { needed: String }` (`13009`; the payload
      names the scope the LLM should ask the operator to add).
    `From<HttpError>` for `AdapterError` now classifies an
    `AuthenticationFailed` message into one of the above via a
    code / phrase scan, never leaking the raw upstream body. Drops
    the `TokenRefreshFailed` and `Other` placeholder variants from
    v0.1 (pre-v1.0 breaking change).
  - `AuthFailureReason` no longer derives `Copy` —
    `ScopeInsufficient` carries a `String`. Existing `match` arms
    that took `AuthFailureReason` by value still work; downstream
    callers that depended on `Copy` borrow instead.
  - Integration tests
    (`first_private_call_triggers_oauth_against_mock`,
    `second_private_call_reuses_token`) drive the full OAuth +
    private-call round-trip against `mockito` and assert the auth
    endpoint is hit exactly once across multiple private calls.

- Account `Read` tool family — first cut (`src/tools/account.rs`):
  - `get_account_summary { currency, extended? }` — balance /
    equity / margin for one currency.
  - `get_positions { currency?, kind?, subaccount_id? }` — open
    positions, optionally filtered.
  - `get_subaccounts { with_portfolio? }` — subaccount list.
  - `get_transaction_log { currency, start_timestamp,
    end_timestamp, query?, count?, subaccount_id?, continuation? }`
    — historical transaction log over a window.
  - `get_deposits { currency, count?, offset? }` — recent deposits.
  - `get_withdrawals { currency, count?, offset? }` — recent
    withdrawals.
  - `get_open_orders_by_currency { currency, kind?, type? }` —
    open orders for a currency.
  - `get_open_orders_by_instrument { instrument_name, type? }` —
    open orders for an instrument.
  - `get_user_trades_by_currency { currency, kind?, start_id?,
    end_id?, count?, start_timestamp?, end_timestamp?, sorting?,
    historical?, subaccount_id? }` — user trades over an id /
    timestamp window. The handler converts the user-supplied
    strings into the upstream's `Currency` /
    `InstrumentKind` / `SortDirection` closed-set enums; an
    out-of-vocab value returns
    `AdapterError::Validation { field: "currency"|"kind"|"sorting" }`.
  - `get_user_trades_by_instrument { instrument_name, start_seq?,
    end_seq?, count?, include_old?, sorting? }` — user trades for
    an instrument over a sequence-number window.
  - All carry `ToolClass::Account`. The registry omits them
    entirely when credentials are absent (ADR-0003), and the
    bearer-token gate at dispatch time provides defence-in-depth.
  - Integration tests cover registry gating
    (`account_tools_register_only_with_credentials`),
    end-to-end dispatch through `mockito`
    (`account_summary_tool_dispatches_through_registry`), and the
    no-credentials → registry-miss path
    (`account_summary_without_credentials_is_validation_error`).
  - Schema snapshot extended with a separate
    `account_tool_input_schemas` snapshot for the new family.

- Project skeleton, dependency set, and module tree per
  `doc/ARCHITECTURE.md` §2:
  - `Cargo.toml` with the v0.1 dependency set: `rmcp`, `tokio`,
    `tracing`, `tracing-subscriber`, `serde`, `serde_json`, `schemars`,
    `thiserror`, `clap`, `dotenvy`, `anyhow` (binary only),
    `deribit-base`, `deribit-http`, `deribit-websocket`.
  - `src/lib.rs` — public module re-exports.
  - `src/main.rs` — binary entry point placeholder.
  - `src/{config,context,error,observability,server}.rs` — module
    placeholders for upcoming v0.1 work.
  - `src/tools/{mod,public,account,trading}.rs` — tool family
    placeholders with the [`ToolClass`] enum (ADR-0003).
  - `src/resources/{mod,static_,live}.rs` — resource family placeholders.
  - `src/prelude.rs` — curated re-exports.
- Developer tooling: `justfile` with `check` (fmt + clippy + test +
  doc) and the same targets as `Makefile`; `clippy.toml` and
  `rustfmt.toml` mirroring sibling-crate conventions.
- `.gitignore` extended to cover IDE / editor / OS noise and `.env*`
  files.
- Configuration loader (`src/config.rs`): CLI + env + `.env` resolution
  per `doc/DERIBIT-INTEGRATION.md` §2 (ADR-0004, ADR-0009, ADR-0011):
  - [`Config`]: Deribit endpoint, credentials, trading flags, transport,
    HTTP listen address, bearer token, log format.
  - `--testnet` / `--mainnet` for endpoint selection (testnet default).
  - `--client-id`, `--allow-trading`, `--max-order-usd`, `--transport`,
    `--listen`, `--log-format`, `--env-file` CLI flags.
  - Environment variable fallbacks: `DERIBIT_ENDPOINT`,
    `DERIBIT_CLIENT_ID`, `DERIBIT_CLIENT_SECRET`,
    `DERIBIT_ALLOW_TRADING`, `DERIBIT_MAX_ORDER_USD`,
    `DERIBIT_MCP_TRANSPORT`, `DERIBIT_HTTP_LISTEN`,
    `DERIBIT_HTTP_BEARER_TOKEN`, `DERIBIT_LOG_FORMAT`.
  - Secrets (`client_secret`, `http_bearer_token`) env/`.env` only
    (no CLI flags to prevent argv leakage).
- Observability (`src/observability.rs`): `tracing` setup with secret
  redaction per `rules/global_rules.md` (Logging & Observability):
  - Dual log format: text (stderr for stdio mode) / JSON (stdout for HTTP).
  - `RUST_LOG` env filter (INFO default).
  - Field-level redaction layer for `client_secret`, `access_token`,
    `refresh_token`, `http_bearer_token`.
  - Structured logging via `FmtSpan::NEW | FmtSpan::CLOSE` for span events.
  - [`init(config)`] initializes the global subscriber from Config.
- `AdapterContext` (`src/context.rs`): the shared value every handler
  holds an `Arc<…>` of. Owns the resolved `Config`, the eagerly
  constructed `deribit_http::DeribitHttpClient`, and a lazy
  `tokio::sync::OnceCell<deribit_websocket::DeribitWebSocketClient>`
  for v0.3 live resources.
  - `AdapterContext::new(Arc<Config>) -> Result<Self, AdapterError>`.
  - `has_credentials()` gate for the `Account` / `Trading` tool families.
  - `websocket()` async accessor with single-init semantics.
- `AdapterError` (`src/error.rs`): the only error type that crosses the
  MCP boundary. Closed-set, structured variants — `_` arms forbidden.
  - Variants: `Auth`, `RateLimited`, `Upstream`, `Validation`,
    `SizeCapExceeded`, `NotEnabled`, `Internal`.
  - `AuthFailureReason` and `UpstreamErrorKind` closed-set helpers.
  - `From` impls for `deribit_http::HttpError`,
    `deribit_websocket::error::WebSocketError`, and `serde_json::Error`
    so upstream errors map at the boundary.
  - serde-tagged (`{"kind": "..."}`) representation for stable wire
    JSON; round-trip identity verified per variant.
  - Convenience constructors (`validation`, `rate_limited`,
    `not_enabled`, `internal`) marked `#[cold] #[inline(never)]`.
- New direct dependency: `url = "2"` (shared with the upstream HTTP
  / WebSocket crates so endpoint round-tripping uses the same
  parser).
- `rmcp` Server scaffold (`src/server.rs`): `DeribitMcpServer`
  implements `rmcp::ServerHandler`. Pins the MCP protocol revision
  to `2025-06-18` (exposed as `MCP_PROTOCOL_VERSION`).
  - `initialize` advertises the documented capabilities: `tools` (no
    `listChanged`), `resources` with `subscribe: true`, and
    `logging`. `prompts` and `sampling` are deliberately not
    advertised in v0.1.
  - `tools/list`, `resources/list`, and
    `resources/templates/list` return the (v0.1-05 empty) registry
    snapshots; v0.1-06 / v0.1-07 fill the registries.
  - `ping`, `shutdown`, and the rest of `ServerHandler` use the
    `rmcp` defaults.
- Stub `ToolRegistry` (`src/tools/mod.rs`) and `ResourceRegistry`
  (`src/resources/mod.rs`) — empty placeholders the server holds an
  `Arc<…>` of so v0.1-06 / v0.1-07 can land independently.
- Tool registry, effect-class gating, and dispatch
  (`src/tools/mod.rs`):
  - `ToolEntry` (descriptor + class + handler) and `ToolHandlerFn`
    (boxed async handler) are the building blocks every per-family
    `register()` adds.
  - `ToolRegistry::build(&AdapterContext)` is the canonical builder:
    `Read` always, `Account` only when credentials are present,
    `Trading` only when credentials are present *and*
    `--allow-trading` is set (ADR-0010).
  - `ToolRegistry::call` performs a defence-in-depth class
    re-check before dispatching to the handler — even if a future
    code path inserts a `Trading` entry without the flag, dispatch
    refuses with `AdapterError::NotEnabled`.
  - Unknown tool name returns `AdapterError::Validation`.
  - `ToolClass::flag()` exposes the human-readable enabling
    requirement for the `NotEnabled` payload.
- Per-family registration hooks (`src/tools/{public,account,trading}.rs`)
  are empty in v0.1-06 — v0.1-10 / v0.1-11 / v0.2 / v0.4 fill them.
- Public `Read` tools — market data (`src/tools/public.rs`):
  - **v0.1-10** (per-instrument):
    - `get_ticker { instrument_name }` — latest ticker.
    - `get_instrument { instrument_name }` — static metadata.
    - `list_instruments { currency, kind?, expired? }` — instruments
      by currency, filterable.
    - `get_order_book { instrument_name, depth? }` — order book
      snapshot.
    - `get_index_price { index_name }` — current index price.
  - **v0.1-11** (summaries & meta):
    - `get_book_summary_by_currency { currency, kind? }` —
      best-bid/ask + 24 h stats for every instrument of a currency.
    - `get_book_summary_by_instrument { instrument_name }` — same for
      one instrument.
    - `get_currencies` — supported-currencies catalogue.
    - `get_server_time` — Deribit server time (epoch ms).
    - `get_status` — platform-wide status (locked currencies).
    - `get_last_trades { instrument_name, count?, include_old? }` —
      recent trades.
    - `get_tradingview_chart_data { instrument_name, start_timestamp,
      end_timestamp, resolution }` — OHLCV bars.
    - `get_funding_rate_history { instrument_name, start_timestamp,
      end_timestamp }` — funding-rate time series.
    - `get_historical_volatility { currency }` — realised volatility
      `[ts_ms, value]` pairs.
  - Each tool: typed `Input` with `JsonSchema`, `Read` effect class,
    `serde_json::Value` output carrying the upstream JSON verbatim.
  - Bad input → `AdapterError::Validation { field: "arguments", ... }`
    with the upstream serde error verbatim, so the LLM sees what's
    wrong.
- `schemars` rolled forward to `1` to align with the major `rmcp`
  re-exports (`rmcp::schemars`) so tool input structs interoperate
  with `Tool::with_input_schema::<T>()` without two-major linkage.
- Static resource registry, `deribit://` URI parser, and read
  dispatch (`src/resources/mod.rs`):
  - `ResourceUri` strongly-typed variants for every documented
    template: `Currencies`, `Instruments { currency }`,
    `Book { instrument }`, `Ticker { instrument }`,
    `Trades { instrument }`.
  - `parse_resource_uri()` accepts the full template set and
    returns `AdapterError::Validation` on anything else (wrong
    scheme, unknown head, missing tail, malformed currency or
    instrument segment).
  - `ResourceUri::to_uri()` round-trips back to the canonical
    string form.
  - `ResourceRegistry::build()` produces the v0.1 catalogue: one
    static entry (`deribit://currencies`) plus four templates
    (`instruments/{currency}`, `book/{instrument}`,
    `ticker/{instrument}`, `trades/{instrument}`).
  - `ResourceRegistry::read()` is the dispatch surface. Behaviour
    when shipped:
    - Static reads: every URI returned a structured `Validation`
      error pointing at v0.1-12.
    - Live reads: every URI returned a structured `Validation`
      error pointing at v0.3.
    Both branches are tightened in subsequent v0.1 issues — see the
    v0.1-12 bullet below for the static wiring.
- `DeribitMcpServer::new` now constructs both registries from the
  context (was empty stubs).
- Static resource reads (`src/resources/static_.rs`):
  - `read_currencies(ctx)` → upstream `get_currencies()`.
  - `read_instruments(ctx, currency)` → upstream
    `get_instruments(currency, None, None)` (kind unfiltered, expired
    excluded).
  - Wired into `ResourceRegistry::read`:
    `Currencies` / `Instruments` route to the upstream HTTP call;
    live URIs (`Book`, `Ticker`, `Trades`) return
    `AdapterError::Internal { reason: "live resources land in v0.3" }`
    so the LLM sees a stable error shape until v0.3 ships.
  - Live-template descriptions and module docs updated to match the
    new error shape.
- stdio transport wiring (`src/main.rs`):
  - Async `tokio::main` runtime; `Config::load` →
    `observability::init` → `AdapterContext::new` →
    `DeribitMcpServer::new` → `serve(stdio())`.
  - INFO banner on startup with the resolved environment label
    (`TESTNET` / `MAINNET`), endpoint, and transport.
  - EOF on stdin propagates through `rmcp`'s service runner into a
    clean exit (`QuitReason::Closed`), exit code 0.
  - HTTP transport branch logs a tracing error and bails with
    `anyhow!("HTTP transport not yet implemented (v0.1-09)")` so the
    binary fails fast — the wiring lands in the next issue.
- `tests/stdio_handshake.rs`: in-process integration test that drives
  the server over a pair of `tokio::io::duplex` pipes, sends one
  `initialize` request, asserts the response shape (protocol version,
  serverInfo, capabilities), and verifies graceful EOF shutdown.
- README polish:
  - "v0.1 is a placeholder release" warning gone — v0.1 ships the
    documented public Read tools, both transports, the static
    resource catalogue, distroless image, Compose recipe, CI, and
    GHCR/crates.io release wiring.
  - Tool list updated to match the actual v0.1 catalogue (14 tools
    across `v0.1-10` and `v0.1-11`, including
    `get_book_summary_by_*`, `get_last_trades`,
    `get_tradingview_chart_data`, `get_funding_rate_history`,
    `get_historical_volatility`).
  - Build-status badge URL fixed for the new GitHub Actions API
    shape and pointed at the `ci.yml` workflow that actually runs.
  - Both Quick-start blocks (Claude Desktop / Docker) gained a
    "Verify" section: a concrete `tools/list` ask for the desktop
    flow, a `curl /healthz` smoke for the container flow.
  - Project-structure block adds `http_transport.rs`, `tests/`,
    `.github/workflows/`. Stale `doc/` references that the public
    repo doesn't ship are dropped.
- Tag-driven release workflow (`.github/workflows/release.yml`):
  - Triggered by pushing a `vX.Y.Z` tag (e.g. `git tag v0.1.0
    && git push origin v0.1.0`).
  - Repo-wide `permissions: { contents: read }` default; per-job
    blocks ramp up only what each step needs (`packages: write`
    for the GHCR push).
  - `validate_tag` job runs first: rejects non-SemVer tags and
    fails the run if `Cargo.toml`'s `package.version` does not
    match the tag (minus the leading `v`). The image and crate
    publishes are gated on it.
  - `image` job builds the container with Docker Buildx, pushes it
    to GHCR (`ghcr.io/<owner>/<repo>`) tagged `:vX.Y.Z` and
    `:latest`, attaches OCI labels (title / source / revision /
    version), and smoke-tests the published artefact via
    `docker run … --version`.
  - GHCR auth uses `secrets.GITHUB_TOKEN`; `permissions:
    packages: write` is granted on the image job only. The
    repo-wide read-only `permissions:` default stays put.
  - `cache-from` / `cache-to: type=gha,mode=max` reuses the
    Buildx cache across release runs.
  - `crates_publish` job runs after the image job is green
    (`needs: image`). It dry-runs `cargo publish` first to surface
    metadata errors before shipping, then publishes the real crate
    via `secrets.CARGO_REGISTRY_TOKEN`. A failing image build
    blocks the crate publish.
- GitHub Actions CI (`.github/workflows/ci.yml`,
  `.github/workflows/coverage.yml`):
  - `ci.yml` jobs: `fmt-check`, `lint` (`cargo clippy
    --all-targets --all-features -- -D warnings`), `test`
    (matrix `stable` + MSRV `1.85` against `cargo test
    --all-features --locked` and `--doc`), `schema` (snapshot
    suite), `integration` (mockito + transport tests),
    `build-release` (`RUSTFLAGS=-D warnings`), `doc`
    (`RUSTDOCFLAGS=-D warnings`).
  - `coverage.yml`: `cargo-tarpaulin` → Codecov upload guarded by
    `secrets.CODECOV_TOKEN`. Honours `codecov.yml` (35 % project,
    15 % patch).
  - All third-party actions pinned to a specific commit SHA with
    a friendly version comment (no `@master` / `@v*`). Every job
    (including `fmt-check`) caches via `Swatinem/rust-cache` with
    a per-job `shared-key` so the registry / git / target dirs are
    warm without cross-contaminating between matrix legs.
  - Top-level `permissions: { contents: read }` on both workflows
    — least-privilege `GITHUB_TOKEN` to reduce blast radius if a
    third-party action is ever compromised.
  - `concurrency: cancel-in-progress` per ref so a quick re-push
    cancels the previous run.
- Reference Compose recipe + env template (`docker-compose.yml`,
  `.env.example`):
  - `docker-compose.yml`: `image: ghcr.io/joaquinbejar/deribit-mcp:latest`,
    `restart: unless-stopped`, `command: --transport=http
    --listen=0.0.0.0:8723 --testnet` (operator flips to mainnet by
    swapping the last argument), `env_file: .env`,
    `DERIBIT_HTTP_BEARER_TOKEN` + `RUST_LOG` carried from the host
    shell, `127.0.0.1:8723:8723` loopback bind.
  - No in-container `HEALTHCHECK`: the runtime is distroless and
    has no shell / `curl` / `nc`; orchestrators probe `/healthz`
    from outside (k8s `httpGet`, Portainer health tab, reverse
    proxy). Inline comment in the Compose file documents this.
  - `.env.example`: documents `DERIBIT_CLIENT_ID`,
    `DERIBIT_CLIENT_SECRET`, `DERIBIT_HTTP_BEARER_TOKEN` (all
    empty placeholders) and a `RUST_LOG` default that operators can
    accept or override.
- `.gitignore` widened from `.env` + `.env.local` to `.env*` with
  `!.env.example` allow-listed, matching the documented policy in
  `.env.example` and ADR-0011: a populated `.env` is never tracked
  even when an operator chooses an unconventional filename.
- README's docker-compose recipe drops the in-container
  `wget` healthcheck — it never worked against the distroless image
  shipped by v0.1-16. The README now spells out the
  external-probe expectation, matching `docker-compose.yml`.
- Container packaging (`Dockerfile`, `.dockerignore`) per ADR-0011:
  - Multi-stage build: `rust:1.85-slim-bookworm` builder pinned to
    MSRV → distroless `gcr.io/distroless/cc-debian12:nonroot`
    runtime.
  - Builder pulls `ca-certificates`, `cmake`, `perl`, and
    `pkg-config` (the last three are required by `aws-lc-sys` from
    the rustls / `deribit-websocket` `rustls-aws-lc` feature). No
    `libssl-dev` — the dep graph is Rustls-based; `openssl-sys` is
    not in `Cargo.lock`.
  - BuildKit cache mounts on `~/.cargo/registry`, `~/.cargo/git`,
    and `target/` so a code-only edit reuses the warm registry +
    incremental target dir; the binary is copied out of the target
    cache before the layer closes.
  - Runtime stage runs as `nonroot:nonroot`, `EXPOSE 8723`, default
    CMD = `--transport=http --listen=0.0.0.0:8723` (testnet by
    default, ADR-0009).
  - OCI labels: title / description / license / source.
  - `.dockerignore` excludes `target/`, `.git/`, `.github/`,
    `.claude/`, `.idea/`, `.vscode/`, `doc/`, `rules/`, every
    `.env*` (with `!.env.example` allow-listed), test snapshots,
    and ad-hoc Markdown so the build context is the source tree
    only.
- End-to-end integration tests (`tests/integration.rs`):
  - `tools_list_without_creds_includes_only_read_class` — registry
    gating sanity check.
  - `tools_call_get_ticker_returns_upstream_payload` — drives a
    `mockito` HTTP server, exercises the full handler / upstream
    HTTP / JSON deserialise path.
  - `tools_call_unknown_returns_validation` — registry miss path.
  - `tools_call_trading_without_allow_trading_returns_not_enabled`
    — guards the ADR-0010 opt-in (Trading absent from registry
    when `--allow-trading` is unset).
  - `resources_read_currencies_returns_upstream_payload` — covers
    the static-resource → upstream HTTP path with mockito.
  - `resources_read_live_uri_returns_internal_until_v03` — `Book`
    URI yields the v0.3 placeholder error.
  - The MCP wire handshake is covered by the existing
    `tests/stdio_handshake.rs` and `tests/http_transport.rs`
    integration tests; this file focuses on the tool / resource
    registries behind a `call` / `read`.
- New dev-dependency: `mockito = "1"` for the upstream HTTP stub.
- Read-only accessors on `ToolEntry` (`descriptor()`, `class()`)
  expose what external integration tests need without giving up the
  frozen-after-build invariant on the underlying fields.
- Schema snapshot tests (`tests/schema.rs`, `tests/snapshots/`):
  - `tool_input_schemas_unchanged` snapshots every public-tool input
    schema produced by `schemars`, sorted by tool name.
  - `adapter_error_wire_shapes_unchanged` snapshots the JSON wire
    shape of every documented `AdapterError` variant (the
    serde-tagged `{"kind": "..."}` representation is part of the
    public contract).
  - `resource_catalogue_unchanged` snapshots the static-entry +
    URI-template payload of `resources/list` /
    `resources/templates/list`.
  - Snapshots live under `tests/snapshots/` and are reviewed
    deliberately on every PR; any change requires a CHANGELOG line
    flagging "additive" vs "breaking" per `doc/TESTING.md` §3.
- New dev-dependency: `insta = "1"` with the `yaml` and `redactions`
  features.
- Graceful shutdown + signal handling (`src/main.rs`):
  - Single shared `tokio_util::sync::CancellationToken` drives shutdown
    for both transports.
  - On Unix: SIGTERM **and** SIGINT (Ctrl-C) cancel the token via a
    `tokio::signal::unix::signal` task; on Windows we fall back to
    SIGINT only. The first signal cancels; subsequent signals are
    no-ops.
  - stdio path uses `serve_with_ct(stdio(), shutdown)` so a signal
    interrupts the running rmcp service alongside EOF on stdin.
  - HTTP path passes the same token to `http_transport::serve` so
    axum + rmcp both observe the cancellation.
  - `main` returns `ExitCode` directly: `0` on clean shutdown, `1`
    on a startup-config / build / bind error (with a single-line
    stderr message — `error: cause: cause` chain joined with `:`,
    no embedded newlines). `2` is reserved for "upstream auth
    failure on first authenticated call" and lands with v0.2.
- `tests/graceful_shutdown.rs`: spins the HTTP transport on a random
  local port, drives `cancel.cancel()` as the unit-test stand-in for
  SIGTERM, and asserts the server task exits within the 5 s grace
  period.
- HTTP / Streamable-HTTP transport (`src/http_transport.rs`):
  - axum router fronts `rmcp::transport::streamable_http_server::StreamableHttpService`
    at `/mcp` and exposes `GET /healthz` (always unauthenticated).
  - Optional static bearer-token auth read only from
    `DERIBIT_HTTP_BEARER_TOKEN` (env / `.env`, no CLI flag, per
    ADR-0004): every `/mcp` request must carry
    `Authorization: Bearer <token>`; mismatches return `401` with a
    `WWW-Authenticate: Bearer` hint. Comparison is constant-time.
  - The middleware is layered on the `/mcp` sub-router only, so
    unknown paths surface a natural 404 and `/healthz` stays
    unauthenticated by construction.
  - Loopback-only host allow-list by default; reverse proxies pre-bind
    a public hostname.
  - Graceful shutdown via a `tokio_util::sync::CancellationToken`
    propagated to both axum and rmcp.
- `src/main.rs` `--transport=http` branch: builds `AdapterContext`,
  spawns a SIGINT handler that cancels the token, and serves the HTTP
  router. INFO banner mirrors the stdio path: env, endpoint, listen
  addr, bearer status (`set` / `none`).
- New direct dependencies: `axum = "0.8"`, `tower = "0.5"`,
  `hyper = "1"` (server + http1 features), `hyper-util = "0.1"`,
  `tokio-util = "0.7"` (rt feature for `CancellationToken`).
- `tests/http_transport.rs`: spins the HTTP server on a random
  loopback port and asserts:
  - `GET /healthz` returns 200 even when a bearer token is configured.
  - `POST /mcp` without an `Authorization` header returns 401.
  - `POST /mcp` with the correct `Bearer <token>` header is not 401.
  - With no bearer configured, `/mcp` is reachable without
    credentials.
- `CHANGELOG.md` following Keep-a-Changelog.

### Repository

- `#![forbid(unsafe_code)]` at the crate root and the binary entry.

[Unreleased]: https://github.com/joaquinbejar/deribit-mcp/compare/HEAD...HEAD
