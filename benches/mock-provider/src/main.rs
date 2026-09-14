//! A provider that does nothing, predictably.
//!
//! The control in every end-to-end measurement. Both routers are pointed at this same
//! binary, so what differs between two runs is the router and not the thing it is
//! talking to.
//!
//! Three properties make it usable as a control, and each is a way a benchmark harness
//! usually goes wrong:
//!
//! * **The service time is fixed, not random.** A jittered sleep turns into variance in
//!   the result, and a reader cannot tell that variance from the router's own.
//! * **No work on the hot path.** Bodies are rendered once at startup, so the mock is
//!   not competing with the router for CPU while being measured.
//! * **It answers both wire formats.** Measuring OpenAI→Claude translation needs a
//!   provider that speaks Claude; using two different mocks would put their difference
//!   inside the result.
//!
//! Run it, then point a router's connection `baseUrl` at it:
//!
//! ```text
//! mock-provider --port 8099 --sleep-ms 25 --frames 200
//! ```
//!
//! `GET /health` reports the configuration it is running with, so a harness can record
//! what it measured against rather than what it meant to.

// The workspace warns on `print_stdout` because services log through `tracing`. This is a
// standalone CLI with no dependencies — adding `tracing` to it would put an allocator and
// a subscriber inside the control — and the one line it prints is its readiness banner,
// which belongs on stdout so a harness can capture what it measured against.
#![allow(clippy::print_stdout)]

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

/// How the mock is configured for one run.
#[derive(Debug, Clone)]
struct Config {
    port: u16,
    /// Fixed service time before the first byte. Models a real provider's think time.
    sleep: Duration,
    /// Frames in a streamed response.
    frames: usize,
    /// Characters of text per frame.
    frame_chars: usize,
    /// Worker threads. Defaults to the machine's parallelism so the mock is not the
    /// bottleneck at concurrency 8.
    workers: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 8_099,
            sleep: Duration::from_millis(25),
            frames: 200,
            frame_chars: 12,
            workers: std::thread::available_parallelism().map_or(4, std::num::NonZero::get),
        }
    }
}

impl Config {
    /// Parse `--flag value` pairs. A bad value is fatal: silently falling back to a
    /// default would mean measuring something other than what was asked for.
    fn from_args() -> Result<Self, String> {
        let mut config = Self::default();
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut index = 0;
        while index < args.len() {
            let flag = args.get(index).map(String::as_str).unwrap_or_default();
            let value = args.get(index + 1);
            let need = |value: Option<&String>| -> Result<String, String> {
                value
                    .cloned()
                    .ok_or_else(|| format!("{flag} needs a value"))
            };
            match flag {
                "--port" => {
                    config.port = need(value)?
                        .parse()
                        .map_err(|_| "--port must be a number".to_owned())?;
                }
                "--sleep-ms" => {
                    let ms: u64 = need(value)?
                        .parse()
                        .map_err(|_| "--sleep-ms must be a number".to_owned())?;
                    config.sleep = Duration::from_millis(ms);
                }
                "--frames" => {
                    config.frames = need(value)?
                        .parse()
                        .map_err(|_| "--frames must be a number".to_owned())?;
                }
                "--frame-chars" => {
                    config.frame_chars = need(value)?
                        .parse()
                        .map_err(|_| "--frame-chars must be a number".to_owned())?;
                }
                "--workers" => {
                    config.workers = need(value)?
                        .parse()
                        .map_err(|_| "--workers must be a number".to_owned())?;
                }
                "--help" | "-h" => return Err(usage()),
                other => return Err(format!("unknown flag {other}\n\n{}", usage())),
            }
            index += 2;
        }
        if config.workers == 0 {
            return Err("--workers must be at least 1".to_owned());
        }
        Ok(config)
    }
}

fn usage() -> String {
    "mock-provider — a fixed-latency provider for router benchmarks\n\n\
     --port N          listen port (default 8099)\n\
     --sleep-ms N      fixed service time before the first byte (default 25)\n\
     --frames N        frames in a streamed response (default 200)\n\
     --frame-chars N   characters of text per frame (default 12)\n\
     --workers N       worker threads (default: available parallelism)\n"
        .to_owned()
}

/// Bodies rendered once, so no work happens per request.
#[derive(Debug)]
struct Bodies {
    openai_json: Vec<u8>,
    claude_json: Vec<u8>,
    openai_stream: Vec<u8>,
    claude_stream: Vec<u8>,
    /// `GET /v1/models`, which a real provider serves and a router probes.
    ///
    /// Without it the mock answered a probe with a chat completion, which the router
    /// correctly rejected as an unreadable model list — so the failure path got
    /// exercised and the success path never did.
    models: Vec<u8>,
    health: Vec<u8>,
}

fn main() -> std::process::ExitCode {
    let config = match Config::from_args() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let bodies = Arc::new(render(&config));
    let listener = match TcpListener::bind(("127.0.0.1", config.port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("could not bind 127.0.0.1:{}: {error}", config.port);
            return std::process::ExitCode::FAILURE;
        }
    };

    println!(
        "mock-provider on 127.0.0.1:{} | sleep={}ms frames={} frame_chars={} workers={}",
        config.port,
        config.sleep.as_millis(),
        config.frames,
        config.frame_chars,
        config.workers
    );

    // A thread pool rather than a thread per connection, so thread-creation cost stays
    // out of the measurement.
    //
    // With keep-alive a worker now holds its connection until the client closes, which
    // makes the pool size a ceiling on concurrent connections rather than on concurrent
    // requests. The ceiling has to exceed the load tool's concurrency plus whatever the
    // router pools upstream, or requests queue behind a busy worker and the queueing is
    // reported as router latency. `--workers` defaults to available parallelism (16 here)
    // against a benchmark concurrency of 8; raise it if either side grows.
    let (sender, receiver) = std::sync::mpsc::channel::<TcpStream>();
    let receiver = Arc::new(std::sync::Mutex::new(receiver));
    for _ in 0..config.workers {
        let receiver = Arc::clone(&receiver);
        let bodies = Arc::clone(&bodies);
        let config = config.clone();
        std::thread::spawn(move || {
            loop {
                let stream = {
                    let Ok(guard) = receiver.lock() else {
                        return;
                    };
                    guard.recv()
                };
                let Ok(stream) = stream else {
                    return;
                };
                let _ = serve(stream, &bodies, &config);
            }
        });
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if sender.send(stream).is_err() {
                    break;
                }
            }
            Err(error) => eprintln!("accept failed: {error}"),
        }
    }
    std::process::ExitCode::SUCCESS
}

/// Render every response body once.
fn render(config: &Config) -> Bodies {
    let text = "x".repeat(config.frame_chars);

    let openai_json = format!(
        r#"{{"id":"chatcmpl-bench","object":"chat.completion","created":1700000000,"model":"bench-model","choices":[{{"index":0,"message":{{"role":"assistant","content":"{text}"}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}}}"#
    );
    let claude_json = format!(
        r#"{{"id":"msg_bench","type":"message","role":"assistant","model":"bench-model","content":[{{"type":"text","text":"{text}"}}],"stop_reason":"end_turn","usage":{{"input_tokens":10,"output_tokens":5}}}}"#
    );

    // Every frame is identical, so render one and repeat it. At 2000 frames the
    // format-per-frame version was doing 2000 allocations to build a constant.
    let openai_frame = format!(
        "data: {{\"id\":\"chatcmpl-bench\",\"object\":\"chat.completion.chunk\",\"model\":\"bench-model\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{text}\"}},\"finish_reason\":null}}]}}\n\n"
    );
    let mut openai_stream = openai_frame.repeat(config.frames);
    openai_stream.push_str("data: {\"id\":\"chatcmpl-bench\",\"object\":\"chat.completion.chunk\",\"model\":\"bench-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n");
    openai_stream.push_str("data: [DONE]\n\n");

    let claude_frame = format!(
        "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{text}\"}}}}\n\n"
    );
    let mut claude_stream = String::new();
    claude_stream.push_str("event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_bench\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"bench-model\",\"content\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":0}}}\n\n");
    claude_stream.push_str("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n");
    claude_stream.push_str(&claude_frame.repeat(config.frames));
    claude_stream.push_str(
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    );
    claude_stream.push_str("event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n");
    claude_stream.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");

    // Reports what it is actually running with, so a harness records the measured
    // configuration rather than the intended one.
    let health = format!(
        r#"{{"ok":true,"sleepMs":{},"frames":{},"frameChars":{},"workers":{}}}"#,
        config.sleep.as_millis(),
        config.frames,
        config.frame_chars,
        config.workers
    );

    // The OpenAI `{"data":[{"id":…}]}` shape, which Anthropic's /v1/models also uses.
    let models = r#"{"object":"list","data":[{"id":"bench-model","object":"model"},{"id":"bench-model-mini","object":"model"}]}"#;

    Bodies {
        openai_json: http_json(&openai_json),
        claude_json: http_json(&claude_json),
        openai_stream: http_sse(&openai_stream),
        claude_stream: http_sse(&claude_stream),
        models: http_json(models),
        health: http_json(&health),
    }
}

// `Connection: keep-alive`, because every real provider supports it and a mock that
// closes every connection is not a neutral control.
//
// It was `close`, and that quietly penalised whichever router pools outbound
// connections. A pooled client keeps the closed socket, tries to reuse it on the next
// request, finds it dead and re-dials -- and nullrouter's streamed responses came out
// at 80 ms against the baseline's 50 ms because of it. With keep-alive the same measurement
// puts nullrouter ahead. A control that changes which router looks faster is not
// measuring the routers.
//
// Both bodies carry Content-Length, so a client knows where each response ends without
// needing the close to delimit it.
fn http_json(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\nKeep-Alive: timeout=60\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn http_sse(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: keep-alive\r\nKeep-Alive: timeout=60\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Read one request, sleep the fixed service time, write the matching body.
fn serve(mut stream: TcpStream, bodies: &Bodies, config: &Config) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    let mut reader = BufReader::new(stream.try_clone()?);

    // One connection, many requests: the responses advertise keep-alive, so a client that
    // takes them at their word and pipelines further requests down the same socket must be
    // answered. Returning after one would leave the client waiting on a socket this mock
    // had already abandoned, which reads as router latency.
    loop {
        match serve_one(&mut stream, &mut reader, bodies, config)? {
            Served::Again => {}
            Served::Done => return Ok(()),
        }
    }
}

/// Whether the connection should stay open for another request.
enum Served {
    Again,
    Done,
}

fn serve_one(
    stream: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    bodies: &Bodies,
    config: &Config,
) -> std::io::Result<Served> {
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        // The client closed. Normal with keep-alive; not an error.
        return Ok(Served::Done);
    }
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_owned();

    // Headers, to find the body length and whether the client wants the socket closed.
    let mut content_length = 0_usize;
    let mut client_wants_close = false;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            } else if name.eq_ignore_ascii_case("connection")
                && value.trim().eq_ignore_ascii_case("close")
            {
                client_wants_close = true;
            }
        }
    }

    // The body has to be drained even though it is unused: leaving it unread makes the
    // client's write block, which would show up as router latency.
    let mut body = vec![0_u8; content_length];
    if content_length > 0 {
        std::io::Read::read_exact(reader, &mut body)?;
    }
    let streaming = memchr_stream_true(&body);

    // Health and the model list answer immediately: neither is a completion, so the
    // configured service time does not apply to them. A probe that paid the sleep would
    // put provider latency into /v1/models, which is not what --sleep-ms means.
    if path.starts_with("/health") {
        stream.write_all(&bodies.health)?;
        stream.flush()?;
        return Ok(if client_wants_close {
            Served::Done
        } else {
            Served::Again
        });
    }
    if path.contains("/models") {
        stream.write_all(&bodies.models)?;
        stream.flush()?;
        return Ok(if client_wants_close {
            Served::Done
        } else {
            Served::Again
        });
    }

    // Fixed, not jittered: variance here becomes variance in the result.
    if !config.sleep.is_zero() {
        std::thread::sleep(config.sleep);
    }

    let claude = path.contains("/messages");
    let response = match (claude, streaming) {
        (true, true) => &bodies.claude_stream,
        (true, false) => &bodies.claude_json,
        (false, true) => &bodies.openai_stream,
        (false, false) => &bodies.openai_json,
    };
    stream.write_all(response)?;
    stream.flush()?;
    Ok(if client_wants_close {
        Served::Done
    } else {
        Served::Again
    })
}

/// Whether the body asks for a stream.
///
/// A substring scan rather than a JSON parse: parsing per request would put the mock's
/// own deserialisation cost inside every measurement.
fn memchr_stream_true(body: &[u8]) -> bool {
    let needle = b"\"stream\":true";
    let compact = body
        .iter()
        .filter(|byte| !byte.is_ascii_whitespace())
        .copied()
        .collect::<Vec<u8>>();
    compact.windows(needle.len()).any(|window| window == needle)
}
