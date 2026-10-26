//! The public landing page.
//!
//! Server-rendered, and the one document in this service that does not mount the WASM bundle: it has to be
//! readable before anything is signed in, and a marketing page that needs WebAssembly to show a paragraph is
//! a page half its visitors never see.
//!
//! # The classes are the dashboard's own utilities
//!
//! This page used to carry a private `nr-*` vocabulary (`nr-top-nav`, `nr-landing-hero`, `nr-button`) that
//! appeared in no stylesheet in the repository. It therefore rendered as an unstyled column of headings and
//! links -- no layout, no colour, no buttons -- and nothing failed a build to say so, because a CSS class that
//! does not exist is not an error.
//!
//! Every class below is a Tailwind utility compiled into `app.css`, which this page links. The theme tokens
//! (`bg-background`, `text-muted-foreground`, `bg-primary`) resolve through the same `:root` and `.dark`
//! blocks the dashboard uses, so the page follows the operator's light/dark choice and looks like the product.

pub(crate) const LANDING_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>nullrouter - One Endpoint for All AI Providers</title>
  <meta name="description" content="One endpoint for OpenAI, Anthropic, Gemini, local models, and CLI tools.">
  <meta name="color-scheme" content="light dark">
  <link rel="stylesheet" href="/assets/dashboard/app.css">
  <link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">
  <script>
    (function () {
      var dark;
      try {
        var stored = window.localStorage.getItem("nullrouter.theme");
        if (stored === "light" || stored === "dark") {
          dark = stored === "dark";
        }
      } catch (_) {}
      if (dark === undefined) {
        try {
          dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
        } catch (_) {
          dark = false;
        }
      }
      if (dark) {
        document.documentElement.classList.add("dark");
      }
    })();
  </script>
</head>
<body class="bg-background text-foreground font-sans antialiased">
  <header class="sticky top-0 z-10 border-b border-border bg-background/85 backdrop-blur-md">
    <div class="mx-auto flex h-14 max-w-6xl items-center gap-3 px-4 sm:px-6">
      <a class="flex items-center gap-2.5 rounded-md outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50" href="/landing" aria-label="nullrouter landing">
        <span class="grid size-7 shrink-0 place-items-center rounded-md bg-primary">
          <svg class="size-[18px] text-primary-foreground" viewBox="0 0 32 32" fill="none" aria-hidden="true">
            <g stroke="currentColor" stroke-width="2.6" stroke-linecap="round">
              <path d="M5 9h6l4 4"/><path d="M5 16h6"/><path d="M5 23h6l4-4"/><path d="M20 16h7"/>
            </g>
            <circle cx="16" cy="16" r="3.4" fill="currentColor"/>
            <path d="M12.4 19.6 19.6 12.4" stroke="var(--color-primary, currentColor)" stroke-width="2.6" stroke-linecap="round" class="text-primary"/>
          </svg>
        </span>
        <span class="text-[0.9375rem] font-semibold tracking-tight">nullrouter</span>
      </a>
      <nav class="ml-auto flex items-center gap-1 text-sm">
        <a class="hidden rounded-md px-3 py-1.5 text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground sm:block" href="#how-it-works">How it works</a>
        <a class="hidden rounded-md px-3 py-1.5 text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground sm:block" href="#features">Features</a>
        <a class="inline-flex h-8 items-center rounded-md bg-primary px-3 text-[0.8125rem] font-medium text-primary-foreground transition-colors hover:bg-primary/90" href="/dashboard">Open Dashboard</a>
      </nav>
    </div>
  </header>

  <main>
    <section class="mx-auto grid max-w-6xl gap-10 px-4 py-16 sm:px-6 lg:grid-cols-2 lg:items-center lg:py-24">
      <div class="space-y-5">
        <p class="inline-flex items-center gap-1.5 rounded-full border border-border bg-muted/50 px-2.5 py-1 text-xs font-medium text-muted-foreground">
          <span class="size-1.5 rounded-full bg-success"></span>
          Self-hosted, single binary
        </p>
        <h1 class="text-balance text-4xl font-semibold tracking-tight sm:text-5xl">
          One endpoint for <span class="text-muted-foreground">all AI providers</span>
        </h1>
        <p class="max-w-xl text-base leading-relaxed text-muted-foreground">
          A local router in front of OpenAI, Anthropic, Gemini, and the models you host yourself.
          Point your tools at one URL and change providers without touching them again.
        </p>
        <div class="flex flex-wrap items-center gap-2 pt-1">
          <a class="inline-flex h-10 items-center rounded-md bg-primary px-5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90" href="/dashboard">Get started</a>
          <a class="inline-flex h-10 items-center rounded-md border border-input bg-background px-5 text-sm font-medium transition-colors hover:bg-accent" href="#how-it-works">How it works</a>
        </div>
        <p class="font-mono text-xs text-muted-foreground">
          Works with Claude Code, Codex, Cline, RooCode, Cursor, and anything that speaks OpenAI.
        </p>
      </div>

      <div class="rounded-xl border border-border bg-card p-5 shadow-xs" aria-label="Route flow">
        <div class="flex items-center justify-between gap-3 rounded-lg border border-border bg-muted/40 px-3 py-2.5">
          <span class="text-sm text-muted-foreground">CLI and SDKs</span>
          <code class="font-mono text-xs text-muted-foreground">/v1</code>
        </div>
        <div class="mx-auto h-6 w-px bg-border"></div>
        <div class="flex items-center justify-between gap-3 rounded-lg border border-primary/30 bg-primary px-3 py-2.5">
          <span class="flex items-center gap-2 text-sm font-medium text-primary-foreground">
            <span class="size-1.5 rounded-full bg-success"></span>
            nullrouter
          </span>
          <code class="font-mono text-xs text-primary-foreground/70">:20128</code>
        </div>
        <div class="mx-auto h-6 w-px bg-border"></div>
        <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
          <span class="rounded-md border border-border bg-background px-2 py-1.5 text-center text-xs text-muted-foreground">OpenAI</span>
          <span class="rounded-md border border-border bg-background px-2 py-1.5 text-center text-xs text-muted-foreground">Anthropic</span>
          <span class="rounded-md border border-border bg-background px-2 py-1.5 text-center text-xs text-muted-foreground">Gemini</span>
          <span class="rounded-md border border-border bg-background px-2 py-1.5 text-center text-xs text-muted-foreground">Local</span>
        </div>
      </div>
    </section>

    <section class="border-y border-border bg-muted/25" id="get-started">
      <div class="mx-auto grid max-w-6xl gap-8 px-4 py-14 sm:px-6 lg:grid-cols-2 lg:items-start">
        <div class="space-y-5">
          <h2 class="text-2xl font-semibold tracking-tight">Running in thirty seconds</h2>
          <p class="text-sm leading-relaxed text-muted-foreground">
            Install, open the dashboard, configure a provider, then point your tools at the local endpoint.
          </p>
          <ol class="space-y-4">
            <li class="flex gap-3">
              <span class="grid size-6 shrink-0 place-items-center rounded-full border border-border bg-background font-mono text-xs text-muted-foreground">1</span>
              <span class="space-y-0.5">
                <strong class="block text-sm font-medium">Start nullrouter</strong>
                <span class="block text-xs leading-relaxed text-muted-foreground"><code class="font-mono">./run.sh</code> brings up every service.</span>
              </span>
            </li>
            <li class="flex gap-3">
              <span class="grid size-6 shrink-0 place-items-center rounded-full border border-border bg-background font-mono text-xs text-muted-foreground">2</span>
              <span class="space-y-0.5">
                <strong class="block text-sm font-medium">Open the dashboard</strong>
                <span class="block text-xs leading-relaxed text-muted-foreground">Add a provider connection and an API key.</span>
              </span>
            </li>
            <li class="flex gap-3">
              <span class="grid size-6 shrink-0 place-items-center rounded-full border border-border bg-background font-mono text-xs text-muted-foreground">3</span>
              <span class="space-y-0.5">
                <strong class="block text-sm font-medium">Route requests</strong>
                <span class="block text-xs leading-relaxed text-muted-foreground">Use <code class="font-mono">http://localhost:20128</code> as your base URL.</span>
              </span>
            </li>
          </ol>
        </div>
        <pre class="overflow-x-auto rounded-xl border border-border bg-card p-4 font-mono text-xs leading-relaxed text-foreground shadow-xs"><code>$ ./run.sh
&gt; Starting nullrouter...
&gt; Server running on http://localhost:20128
&gt; Dashboard: http://localhost:20128/dashboard
&gt; Ready to route.</code></pre>
      </div>
    </section>

    <section class="mx-auto max-w-6xl px-4 py-14 sm:px-6" id="how-it-works">
      <h2 class="text-2xl font-semibold tracking-tight">How it works</h2>
      <div class="mt-6 grid gap-3 sm:grid-cols-3">
        <article class="rounded-xl border border-border bg-card p-5">
          <span class="inline-flex items-center rounded-md bg-muted px-1.5 py-0.5 font-mono text-[0.6875rem] text-muted-foreground">CLI</span>
          <h3 class="mt-3 text-sm font-medium">CLI and SDKs</h3>
          <p class="mt-1 text-xs leading-relaxed text-muted-foreground">Your requests start from the tools you already use. Change only the base URL.</p>
        </article>
        <article class="rounded-xl border border-border bg-card p-5">
          <span class="inline-flex items-center rounded-md bg-primary px-1.5 py-0.5 font-mono text-[0.6875rem] text-primary-foreground">nr</span>
          <h3 class="mt-3 text-sm font-medium">nullrouter</h3>
          <p class="mt-1 text-xs leading-relaxed text-muted-foreground">The local router picks a configured provider and keeps one endpoint stable.</p>
        </article>
        <article class="rounded-xl border border-border bg-card p-5">
          <span class="inline-flex items-center rounded-md bg-muted px-1.5 py-0.5 font-mono text-[0.6875rem] text-muted-foreground">AI</span>
          <h3 class="mt-3 text-sm font-medium">Providers</h3>
          <p class="mt-1 text-xs leading-relaxed text-muted-foreground">Requests reach OpenAI, Anthropic, Gemini, local models, or whatever you add next.</p>
        </article>
      </div>
    </section>

    <section class="border-t border-border bg-muted/25" id="features">
      <div class="mx-auto max-w-6xl px-4 py-14 sm:px-6">
        <h2 class="text-2xl font-semibold tracking-tight">What the dashboard gives you</h2>
        <div class="mt-6 grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
          <article class="rounded-xl border border-border bg-card p-5">
            <h3 class="text-sm font-medium">One endpoint</h3>
            <p class="mt-1 text-xs leading-relaxed text-muted-foreground">Every provider behind a single standard API URL.</p>
          </article>
          <article class="rounded-xl border border-border bg-card p-5">
            <h3 class="text-sm font-medium">Credentials in one place</h3>
            <p class="mt-1 text-xs leading-relaxed text-muted-foreground">OAuth and API keys managed together, encrypted at rest.</p>
          </article>
          <article class="rounded-xl border border-border bg-card p-5">
            <h3 class="text-sm font-medium">Usage and cost</h3>
            <p class="mt-1 text-xs leading-relaxed text-muted-foreground">Requests, tokens, and spend tracked per model.</p>
          </article>
          <article class="rounded-xl border border-border bg-card p-5">
            <h3 class="text-sm font-medium">Live routing state</h3>
            <p class="mt-1 text-xs leading-relaxed text-muted-foreground">See which service answers a path, and what is being held back.</p>
          </article>
        </div>
      </div>
    </section>
  </main>

  <footer class="border-t border-border">
    <div class="mx-auto flex max-w-6xl flex-col gap-2 px-4 py-6 text-xs text-muted-foreground sm:flex-row sm:items-center sm:justify-between sm:px-6">
      <span>nullrouter &middot; self-hosted AI model router</span>
      <a class="underline-offset-4 hover:underline" href="/dashboard">Open the dashboard</a>
    </div>
  </footer>
</body>
</html>
"##;
