//! Where does the router's remaining per-request overhead go?
//!
//! `benches/run.sh` measures the router end to end. When that number stops falling, the next
//! question is which *part* of the router the remainder is in, and subtracting hops in `run.sh`
//! cannot answer it: every hop includes the mock's fixed service time, and the interesting costs
//! are two orders of magnitude smaller than that.
//!
//! This probe measures the pieces directly, against the same mock:
//!
//! * `reqwest` — one pooled POST and JSON parse, which is the floor for anything the runtime does.
//! * `translate` — the request and response translation, with no network at all.
//! * `serde` — parse and re-serialise the bodies, since the pipeline does both.
//!
//! Run the mock first, then `hop-probe [mock-addr]`. Reports p50 and p99 over a fixed count, and
//! no absolute pass/fail: it exists to say which piece is worth attention next, and the answer is
//! only meaningful relative to the end-to-end overhead in `benches/results`.

#![allow(
    clippy::print_stdout,
    reason = "a measurement tool whose entire output is a report on stdout"
)]

use std::time::{Duration, Instant};

const DEFAULT_MOCK: &str = "127.0.0.1:8099";
/// Samples per measurement. Enough that p99 means something without the run taking minutes.
const SAMPLES: usize = 2000;

fn request_body() -> serde_json::Value {
    serde_json::json!({
        "model": "bench-model",
        "stream": false,
        "max_tokens": 256,
        "messages": [{
            "role": "user",
            "content": "Explain the difference between a mutex and a channel.",
        }],
    })
}

/// A provider reply in OpenAI shape, the same size the mock returns.
fn response_body() -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-bench",
        "object": "chat.completion",
        "created": 1_700_000_000_i64,
        "model": "bench-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "xxxxxxxxxxxx" },
            "finish_reason": "stop",
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 },
    })
}

/// p50 and p99 of a sample set, in microseconds.
fn report(label: &str, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    let at = |fraction: f64| -> f64 {
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "index arithmetic over a few thousand samples"
        )]
        let index = ((samples.len() as f64 - 1.0) * fraction) as usize;
        samples
            .get(index)
            .map_or(0.0, |sample| sample.as_secs_f64() * 1_000_000.0)
    };
    println!(
        "  {label:<28} p50 {:>9.2} us   p99 {:>9.2} us",
        at(0.50),
        at(0.99)
    );
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mock = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_MOCK.to_owned());
    let url = format!("http://{mock}/v1/chat/completions");
    let body = request_body();
    let reply = response_body();

    println!("hop-probe against {mock}, {SAMPLES} samples each\n");

    // The same client configuration the executor uses, so the pooling behaviour matches.
    let client = match reqwest::Client::builder()
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            println!("could not build a client: {error}");
            return;
        }
    };

    // Warm the pool so the first connection's handshake is not in the samples.
    for _ in 0_u8..5 {
        let _ = client.post(&url).json(&body).send().await;
    }

    // 1. The network floor: one pooled POST, and reading the reply as JSON. Includes the mock's
    //    own service time, so only the *difference* from `oha` against the same mock is this
    //    client's cost.
    let mut network = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        if let Ok(response) = client.post(&url).json(&body).send().await {
            let _ = response.json::<serde_json::Value>().await;
        }
        network.push(started.elapsed());
    }
    report("reqwest POST + parse", network);

    // 2. Translation, no network. Both directions, since a request pays both.
    let state = nullrouter_translate::StreamState::new(nullrouter_translate::Clock::System);
    let route = nullrouter_translate::RequestRoute {
        source: nullrouter_providers::Format::OpenAi,
        target: nullrouter_providers::Format::Claude,
        provider: "anthropic",
        model: "claude-sonnet-4-5",
    };
    let mut translate = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let translated = nullrouter_translate::translate_request(route, &body, false, 0);
        let _ = nullrouter_translate::translate_body(
            nullrouter_providers::Format::Claude,
            nullrouter_providers::Format::OpenAi,
            &reply,
            &state,
        );
        std::hint::black_box(translated);
        translate.push(started.elapsed());
    }
    report("translate both ways", translate);

    // 3. serde alone: the pipeline parses the client's body and serialises what it sends.
    let request_bytes = serde_json::to_vec(&body).unwrap_or_default();
    let mut serde_cost = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let parsed: serde_json::Value = serde_json::from_slice(&request_bytes).unwrap_or_default();
        let written = serde_json::to_vec(&parsed).unwrap_or_default();
        std::hint::black_box(written);
        serde_cost.push(started.elapsed());
    }
    report("serde parse + serialise", serde_cost);

    println!(
        "\nCompare `reqwest POST + parse` against oha's p50 for the same mock: the difference is\n\
         what this client costs over a minimal one. `translate` and `serde` are absolute, and both\n\
         should be small next to the end-to-end overhead in benches/results."
    );
}
