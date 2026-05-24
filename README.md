# prompt-cache-warmer

[![Crates.io](https://img.shields.io/crates/v/prompt-cache-warmer.svg)](https://crates.io/crates/prompt-cache-warmer)
[![Documentation](https://docs.rs/prompt-cache-warmer/badge.svg)](https://docs.rs/prompt-cache-warmer)
[![License](https://img.shields.io/crates/l/prompt-cache-warmer.svg)](https://crates.io/crates/prompt-cache-warmer)

**Pre-warm Anthropic prompt cache before user traffic hits.**

Anthropic charges 25% more on the first request that creates a cache entry and only 10% on subsequent reads. If you have a long system prompt or a tool list, you want the first cache write to be a cheap synthetic warmup at deploy time, not a real user paying for it (and waiting for it).

```rust
use prompt_cache_warmer::{Warmer, WarmCall, WarmRequest, WarmResponse, Usage};

// Plug in any transport that implements `WarmCall`.
let warmer = Warmer::new(my_anthropic_client);

let result = warmer
    .warm_verified("claude-opus-4-7", LONG_SYSTEM_PROMPT)
    .unwrap();

println!("cache write: {} tokens", result.cache_creation_input_tokens);
println!("verified read: {:?}", result.verified_hit_tokens);
println!("warm cost: ${:.4}", result.cost_usd.unwrap_or(0.0));
```

That's it. `Warmer` injects `cache_control` breakpoints into your system blocks, fires a tiny `max_tokens = 8` call, and optionally fires a second call to assert the cache actually read back.

## Install

```toml
[dependencies]
prompt-cache-warmer = "0.1"
```

Zero runtime dependencies. Bring your own HTTP client by implementing the `WarmCall` trait.

## Why

The first request that creates a cache entry costs 1.25x the regular input price. Every subsequent request that reads it costs 0.10x. If your system prompt is 50k tokens, that first hit is the difference between a snappy response and a user waiting two seconds for the prompt to be processed from scratch.

Run `Warmer` from a deploy hook or a cron and the first user request hits a warm cache.

## API

```rust
// Construct
let warmer = Warmer::new(client);                       // default price table
let warmer = Warmer::with_prices(client, custom_table); // BYO pricing

// Shortcuts
let result = warmer.warm("claude-opus-4-7", system_str)?;
let result = warmer.warm_verified("claude-opus-4-7", system_str)?;

// Full input
let mut input = WarmInput::new("claude-opus-4-7", system_str);
input.tools = my_tools;
input.breakpoints = 2;
let result = warmer.warm_with(input)?;
```

`Warmer<C>` is generic over a `WarmCall` trait:

```rust
pub trait WarmCall {
    type Error: std::error::Error;
    fn call(&self, req: &WarmRequest) -> Result<WarmResponse, Self::Error>;
}
```

Implement it once for whichever HTTP client you use (the official Anthropic SDK wrapper, a Bedrock crate, `reqwest`, or a fake).

```rust
pub struct WarmResult {
    pub model: String,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub latency_ms_warm: u64,
    pub verified_hit_tokens: Option<u64>,
    pub latency_ms_verify: Option<u64>,
    pub cost_usd: Option<f64>,
}
```

## Companion libraries

`prompt-cache-warmer` warms the cache. [`cachebench`](https://github.com/MukundaKatta/cachebench) measures the resulting hit ratio over time. Run the warmer once at deploy, then have `cachebench` keep score.

For runtime cost tracking, pair with [`claude-cost`](https://github.com/MukundaKatta/claude-cost) (Rust) or wire the same multipliers into your own logging.

A Python port lives at [`prompt-cache-warmer`](https://github.com/MukundaKatta/prompt-cache-warmer) on PyPI.

## License

MIT OR Apache-2.0
