//! End-to-end behavior checks for `prompt-cache-warmer`.

use std::cell::RefCell;
use std::fmt;

use prompt_cache_warmer::{
    add_cache_breakpoints, default_prices, to_system_blocks, Block, CacheControl, Message, Tool,
    Usage, WarmCall, WarmInput, WarmRequest, WarmResponse, WarmResult, Warmer,
};

// ---- a scripted fake transport --------------------------------------------

#[derive(Debug)]
struct FakeError(&'static str);
impl fmt::Display for FakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for FakeError {}

struct FakeClient {
    scripts: RefCell<Vec<Usage>>,
    calls: RefCell<Vec<WarmRequest>>,
}

impl FakeClient {
    fn new(scripts: Vec<Usage>) -> Self {
        FakeClient {
            scripts: RefCell::new(scripts),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn call_log(&self) -> Vec<WarmRequest> {
        self.calls.borrow().clone()
    }
}

impl WarmCall for FakeClient {
    type Error = FakeError;
    fn call(&self, req: &WarmRequest) -> Result<WarmResponse, Self::Error> {
        self.calls.borrow_mut().push(req.clone());
        let mut scripts = self.scripts.borrow_mut();
        if scripts.is_empty() {
            return Err(FakeError("script exhausted"));
        }
        let usage = scripts.remove(0);
        Ok(WarmResponse { usage })
    }
}

// ---- block helpers ---------------------------------------------------------

#[test]
fn to_system_blocks_from_string() {
    let out = to_system_blocks("hi");
    assert_eq!(out, vec![Block::from("hi")]);
    assert_eq!(out[0].text, "hi");
    assert_eq!(out[0].cache_control, None);
}

#[test]
fn block_from_string_and_str_are_equivalent() {
    let a: Block = "abc".into();
    let b: Block = String::from("abc").into();
    assert_eq!(a, b);
}

#[test]
fn add_cache_breakpoints_zero_is_noop() {
    let blocks = vec![Block::from("a"), Block::from("b")];
    let out = add_cache_breakpoints(&blocks, 0);
    assert_eq!(out, blocks);
    assert!(out.iter().all(|b| b.cache_control.is_none()));
}

#[test]
fn add_cache_breakpoints_default_marks_last_only() {
    let blocks = vec![Block::from("a"), Block::from("b")];
    let out = add_cache_breakpoints(&blocks, 1);
    assert_eq!(out[0].cache_control, None);
    assert_eq!(out[1].cache_control, Some(CacheControl::Ephemeral));
}

#[test]
fn add_cache_breakpoints_caps_at_four() {
    let blocks: Vec<Block> = (0..10).map(|i| Block::from(format!("b{i}"))).collect();
    let out = add_cache_breakpoints(&blocks, 99);
    let marked = out.iter().filter(|b| b.cache_control.is_some()).count();
    assert_eq!(marked, 4);
}

#[test]
fn add_cache_breakpoints_preserves_existing_marker() {
    let blocks = vec![
        Block {
            text: "a".into(),
            cache_control: Some(CacheControl::Ephemeral),
        },
        Block::from("b"),
    ];
    let out = add_cache_breakpoints(&blocks, 1);
    // First was preserved, last was picked by the breakpoint position.
    assert_eq!(out[0].cache_control, Some(CacheControl::Ephemeral));
    assert_eq!(out[1].cache_control, Some(CacheControl::Ephemeral));
}

#[test]
fn add_cache_breakpoints_empty_input() {
    let out = add_cache_breakpoints(&[], 3);
    assert!(out.is_empty());
}

#[test]
fn add_cache_breakpoints_spreads_and_ends_on_last() {
    // 8 blocks, 3 breakpoints: evenly spaced, last block always marked.
    let blocks: Vec<Block> = (0..8).map(|i| Block::from(format!("b{i}"))).collect();
    let out = add_cache_breakpoints(&blocks, 3);
    let marked: Vec<usize> = out
        .iter()
        .enumerate()
        .filter(|(_, b)| b.cache_control.is_some())
        .map(|(i, _)| i)
        .collect();
    assert_eq!(marked.len(), 3);
    assert_eq!(*marked.last().unwrap(), out.len() - 1);
}

// ---- Warmer::warm ----------------------------------------------------------

#[test]
fn warm_returns_warm_result_with_token_counts() {
    let client = FakeClient::new(vec![Usage {
        input_tokens: 10,
        output_tokens: 4,
        cache_creation_input_tokens: 12_000,
        cache_read_input_tokens: 0,
    }]);
    let warmer = Warmer::new(client);
    let out: WarmResult = warmer.warm("claude-opus-4-7", "long system text").unwrap();
    assert_eq!(out.cache_creation_input_tokens, 12_000);
    assert_eq!(out.cache_read_input_tokens, 0);
    assert_eq!(out.input_tokens, 10);
    assert_eq!(out.output_tokens, 4);
    assert!(out.verified_hit_tokens.is_none());
}

#[test]
fn warm_inserts_cache_control_on_last_block() {
    let warmer = Warmer::new(FakeClient::new(vec![Usage::default()]));
    warmer.warm("claude-opus-4-7", "abc").unwrap();
    let calls = warmer.client().call_log();
    let sent = &calls[0];
    let last = sent.system_blocks.last().unwrap();
    assert_eq!(last.cache_control, Some(CacheControl::Ephemeral));
}

#[test]
fn warm_default_ping_user_message_and_max_tokens() {
    let warmer = Warmer::new(FakeClient::new(vec![Usage::default()]));
    warmer.warm("claude-opus-4-7", "x").unwrap();
    let calls = warmer.client().call_log();
    let sent = &calls[0];
    assert_eq!(
        sent.messages,
        vec![Message {
            role: "user".into(),
            content: "ok".into(),
        }]
    );
    assert_eq!(sent.max_tokens, 8);
}

#[test]
fn warm_uses_supplied_messages_and_tools() {
    let warmer = Warmer::new(FakeClient::new(vec![Usage::default()]));
    let mut input = WarmInput::new("claude-opus-4-7", "x");
    input.messages = vec![Message {
        role: "user".into(),
        content: "hello".into(),
    }];
    input.tools = vec![Tool { name: "t".into() }];
    warmer.warm_with(input).unwrap();
    let calls = warmer.client().call_log();
    let sent = &calls[0];
    assert_eq!(sent.messages[0].content, "hello");
    assert_eq!(sent.tools[0].name, "t");
}

#[test]
fn warm_verified_returns_cache_read_tokens() {
    let warmer = Warmer::new(FakeClient::new(vec![
        Usage {
            cache_creation_input_tokens: 5_000,
            ..Usage::default()
        },
        Usage {
            cache_read_input_tokens: 5_000,
            ..Usage::default()
        },
    ]));
    let out = warmer.warm_verified("claude-opus-4-7", "x").unwrap();
    assert_eq!(out.verified_hit_tokens, Some(5_000));
    assert!(out.latency_ms_verify.is_some());
    assert_eq!(warmer.client().call_log().len(), 2);
}

#[test]
fn cost_estimate_with_default_prices() {
    let warmer = Warmer::new(FakeClient::new(vec![Usage {
        input_tokens: 0,
        output_tokens: 10,
        cache_creation_input_tokens: 1_000_000,
        cache_read_input_tokens: 0,
    }]));
    let out = warmer.warm("claude-opus-4-7", "x").unwrap();
    // 1M write * $5/M * 1.25 = $6.25, + 10 out * $25/M ~= 0.00025.
    let cost = out.cost_usd.expect("cost present for known model");
    assert!((6.20..=6.30).contains(&cost), "got {cost}");
}

#[test]
fn cost_estimate_combines_regular_read_and_output() {
    // Sonnet 4.6: input $3/M, output $15/M. Multipliers: read 0.10x.
    let warmer = Warmer::new(FakeClient::new(vec![Usage {
        input_tokens: 1_000_000,
        output_tokens: 1_000_000,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 1_000_000,
    }]));
    let out = warmer.warm("claude-sonnet-4-6", "x").unwrap();
    // regular: 1M * $3/M = 3.0; read: 1M * $3/M * 0.10 = 0.3; out: 1M * $15/M = 15.0.
    let cost = out.cost_usd.expect("cost present for known model");
    assert!((18.25..=18.35).contains(&cost), "got {cost}");
}

#[test]
fn cost_is_none_for_unknown_model() {
    let warmer = Warmer::new(FakeClient::new(vec![Usage {
        input_tokens: 10,
        ..Usage::default()
    }]));
    let out = warmer.warm("some-unknown-model", "x").unwrap();
    assert!(out.cost_usd.is_none());
}

#[test]
fn default_prices_contains_known_models() {
    let p = default_prices();
    assert!(p.contains_key("claude-opus-4-7"));
    assert!(p.contains_key("claude-sonnet-4-6"));
    assert!(p.contains_key("claude-haiku-4-5"));
}

#[test]
fn warm_with_supplied_breakpoint_count() {
    let warmer = Warmer::new(FakeClient::new(vec![Usage::default()]));
    let blocks: Vec<Block> = (0..4).map(|i| Block::from(format!("b{i}"))).collect();
    let mut input = WarmInput::new("claude-opus-4-7", "ignored");
    input.system_blocks = blocks;
    input.breakpoints = 2;
    warmer.warm_with(input).unwrap();
    let marked = warmer.client().call_log()[0]
        .system_blocks
        .iter()
        .filter(|b| b.cache_control.is_some())
        .count();
    assert_eq!(marked, 2);
}

#[test]
fn warm_propagates_transport_error() {
    let warmer = Warmer::new(FakeClient::new(Vec::new()));
    let err = warmer.warm("claude-opus-4-7", "x").unwrap_err();
    assert_eq!(err.to_string(), "script exhausted");
}
