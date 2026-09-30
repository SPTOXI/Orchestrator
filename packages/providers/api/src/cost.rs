//! What a model call costs, with the prompt cache (ADR-0018).
//!
//! The APIs report tokens, never prices: the prices are the model's, as the
//! user (or "Buscar modelos") filled them in. `input_tokens` is the whole
//! prompt; the cached reads and writes are parts of it.

use crate::config::{ApiKind, Connection, ModelEntry};
use orchestrator_core::TokenUsage;

/// Share of the input price a cache read costs on Anthropic when the model
/// has no cache price of its own (the vendor's published rule).
const ANTHROPIC_READ: f64 = 0.1;

/// USD per million tokens of each kind of input for one model call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Prices {
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: Option<f64>,
}

/// The prices of `model` on `conn`, or `None` when the model has no input
/// price (the cost is then unknown, not zero).
pub(crate) fn prices(conn: &Connection, model: &ModelEntry) -> Option<Prices> {
    let input = model.input_price;
    let output = model.output_price;
    if input.is_none() && output.is_none() {
        return None;
    }
    let input = input.unwrap_or(0.0);
    let anthropic = conn.kind == ApiKind::Anthropic;
    let cache_read = model.cached_input_price.unwrap_or(if anthropic {
        input * ANTHROPIC_READ
    } else {
        input
    });
    // Only Anthropic bills writes to the cache; elsewhere caching is
    // automatic and a "write" is an ordinary input token.
    let cache_write = if anthropic {
        input
            * conn
                .options
                .cache_ttl
                .unwrap_or_default()
                .write_multiplier()
    } else {
        input
    };
    Some(Prices {
        input,
        cache_read,
        cache_write,
        output,
    })
}

/// `usage` with its cost and what the cache saved, when the model has
/// prices.
pub(crate) fn priced(conn: &Connection, model: &ModelEntry, usage: TokenUsage) -> TokenUsage {
    match prices(conn, model) {
        Some(prices) => with_cost(usage, prices),
        None => usage,
    }
}

pub(crate) fn with_cost(mut usage: TokenUsage, prices: Prices) -> TokenUsage {
    let per = |tokens: u64, price: f64| tokens as f64 * price / 1_000_000.0;
    let read = usage.cached_input_tokens.min(usage.input_tokens);
    let written = usage
        .cache_write_tokens
        .min(usage.input_tokens.saturating_sub(read));
    let plain = usage.input_tokens - read - written;
    let input_cost =
        per(plain, prices.input) + per(read, prices.cache_read) + per(written, prices.cache_write);
    let output_cost = prices.output.map_or(0.0, |p| per(usage.output_tokens, p));
    usage.cost_usd = Some(input_cost + output_cost);
    if read > 0 || written > 0 {
        let full = per(read + written, prices.input);
        usage.cache_saved_usd =
            Some(full - per(read, prices.cache_read) - per(written, prices.cache_write));
    }
    usage
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn conn(kind: &str, ttl: Option<&str>) -> Connection {
        let mut value = json!({
            "id": "c", "name": "C", "kind": kind, "baseUrl": "https://example.test",
        });
        if let Some(ttl) = ttl {
            value["options"] = json!({"cacheTtl": ttl});
        }
        serde_json::from_value(value).unwrap()
    }

    fn model(input: Option<f64>, output: Option<f64>, cached: Option<f64>) -> ModelEntry {
        ModelEntry {
            input_price: input,
            output_price: output,
            cached_input_price: cached,
            ..ModelEntry::new("m")
        }
    }

    fn usage(input: u64, read: u64, written: u64, output: u64) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            cached_input_tokens: read,
            cache_write_tokens: written,
            output_tokens: output,
            ..Default::default()
        }
    }

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 1e-9)
    }

    #[test]
    fn no_price_means_unknown_cost() {
        let priced = priced(
            &conn("openai", None),
            &model(None, None, None),
            usage(1_000, 0, 0, 10),
        );
        assert_eq!(priced.cost_usd, None);
        assert_eq!(priced.cache_saved_usd, None);
    }

    #[test]
    fn plain_input_and_output() {
        // 1M in at $4 + 1M out at $20.
        let priced = priced(
            &conn("openai", None),
            &model(Some(4.0), Some(20.0), None),
            usage(1_000_000, 0, 0, 1_000_000),
        );
        assert!(close(priced.cost_usd, 24.0));
        assert_eq!(priced.cache_saved_usd, None);
    }

    #[test]
    fn anthropic_reads_writes_and_savings() {
        // Opus 5.5: $4 in, $0.20 cached, writes 1.25× = $5.
        let c = conn("anthropic", None);
        let m = model(Some(4.0), Some(20.0), Some(0.2));
        // 1M prompt: 600k read, 300k written, 100k plain; no output.
        let priced = priced(&c, &m, usage(1_000_000, 600_000, 300_000, 0));
        let cost = 0.1 * 4.0 + 0.6 * 0.2 + 0.3 * 5.0;
        assert!(close(priced.cost_usd, cost));
        let saved = 0.9 * 4.0 - 0.6 * 0.2 - 0.3 * 5.0;
        assert!(close(priced.cache_saved_usd, saved));

        // One-hour entries: writes cost 2×.
        let hour = priced_with(&conn("anthropic", Some("1h")), &m, 1_000_000, 0, 1_000_000);
        assert!(close(hour.cost_usd, 8.0));
        assert!(close(hour.cache_saved_usd, -4.0));

        // No cache price on an Anthropic model: 10% of the input.
        let default = priced_with(&c, &model(Some(3.0), None, None), 1_000_000, 1_000_000, 0);
        assert!(close(default.cost_usd, 0.3));
    }

    fn priced_with(
        c: &Connection,
        m: &ModelEntry,
        input: u64,
        read: u64,
        written: u64,
    ) -> TokenUsage {
        priced(c, m, usage(input, read, written, 0))
    }

    #[test]
    fn other_vendors_price_reads_at_full_price_unless_told() {
        let c = conn("openai", None);
        let full = priced_with(&c, &model(Some(2.0), None, None), 1_000_000, 500_000, 0);
        assert!(close(full.cost_usd, 2.0));
        assert!(close(full.cache_saved_usd, 0.0));
        let told = priced_with(
            &c,
            &model(Some(2.0), None, Some(0.5)),
            1_000_000,
            500_000,
            0,
        );
        assert!(close(told.cost_usd, 0.5 * 2.0 + 0.5 * 0.5));
        assert!(close(told.cache_saved_usd, 0.5 * 1.5));
    }

    #[test]
    fn inconsistent_counts_never_underflow() {
        let priced = priced(
            &conn("anthropic", None),
            &model(Some(1.0), None, None),
            usage(10, 50, 50, 0),
        );
        assert!(priced.cost_usd.is_some_and(|c| c >= 0.0));
    }
}
