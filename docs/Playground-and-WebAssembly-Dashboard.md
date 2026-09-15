# 🖥️ WebAssembly Dashboard & Chat Playground

NullRouter features a modern, ultra-responsive single-page application built entirely in **Rust using Leptos 0.7**, compiled to WebAssembly (`wasm32-unknown-unknown`).

---

## 🎨 Dashboard Design & Token System

- **Zero JavaScript Runtime Dependencies**: The UI compiles to pure WebAssembly and loads in under 100ms.
- **Tailwind CSS & Shadcn Tokens**: Pixel-perfect dark/light theme, accessible contrasts, responsive flex/grid layouts.
- **35 Localized Languages**:
  English, Spanish, Mandarin (Simplified & Traditional), Japanese, German, French, Portuguese (Brazil & Portugal), Russian, Vietnamese, Korean, Italian, Dutch, Polish, Turkish, Arabic, Hindi, Thai, Indonesian, and more.
- **Instant Language Switching**: Dynamic locale switching without page reloads.

---

## 💬 Interactive Chat Playground (`/dashboard/chat`)

The Chat Playground allows developers to test model configurations, inspect token throughput, and visualize reasoning chains before connecting external IDEs.

```
┌─────────────────────────────────────────────────────────────┐
│  Model: [ auto ▾ ]   Temperature: [ 0.7 ]   Stream: [✓]     │
├─────────────────────────────────────────────────────────────┤
│ 👤 User: Write a zero-copy ring buffer in Rust              │
│                                                             │
│ 🤖 Assistant:                                               │
│   ┌─ 🧠 Thought Process (8,192 tokens) ─────────────────┐   │
│   │ [▼ Click to collapse]                               │   │
│   │  - Need fixed-size circular array without heap alloc│   │
│   │  - Use AtomicUsize for head and tail pointers       │   │
│   │  - Handle wrap-around with bitwise masking (power 2)│   │
│   └─────────────────────────────────────────────────────┘   │
│                                                             │
│   Here is a high-performance zero-copy ring buffer:         │
│   ```rust                                                   │
│   pub struct RingBuffer<T, const N: usize> { ... }          │
│   ```                                                       │
└─────────────────────────────────────────────────────────────┘
```

### Key Features of the Playground:
1. **Collapsible `<think>` Reasoning Blocks**:
   - Native parser captures thoughts from Claude Opus 5 / Sonnet 4.6 hybrid thinking, DeepSeek V4, and Gemini 3.8 Flash Thinking (per `models.dev` 2026-09).
   - Formatted in an interactive collapsible panel with live token counters.
2. **Real-Time Token & Latency Metrics**:
   - Time to First Token (TTFT) in milliseconds.
   - Tokens per second (TPS) streaming speed meter.
   - Cache hit status indicator (whether prompt caching was leveraged).
3. **Multi-Model Side-by-Side Comparison**:
   - Send the same prompt to two providers simultaneously to compare speed, response quality, and token cost.
