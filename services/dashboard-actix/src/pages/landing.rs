pub(crate) const LANDING_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>9Router - One Endpoint for All AI Providers</title>
  <meta name="description" content="One endpoint for OpenAI, Anthropic, Gemini, local models, and CLI tools.">
  <link rel="stylesheet" href="/assets/dashboard.css">
  <link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">
</head>
<body class="nr-top-page nr-landing-page">
  <header class="nr-top-nav">
    <a class="nr-top-brand" href="/landing" aria-label="9Router landing">
      <span class="nr-logo-mark">9</span>
      <span>9Router</span>
    </a>
    <nav>
      <a href="#how-it-works">How it works</a>
      <a href="#features">Features</a>
      <a href="/dashboard">Dashboard</a>
    </nav>
  </header>
  <main>
    <section class="nr-landing-hero">
      <div class="nr-hero-copy">
        <p class="nr-kicker">v1.0 is now live</p>
        <h1>One Endpoint for <span>All AI Providers</span></h1>
        <p class="nr-hero-text">AI endpoint proxy with web dashboard. Works with Claude Code, OpenAI Codex, Cline, RooCode, Cursor, and other CLI tools.</p>
        <div class="nr-hero-actions">
          <a class="nr-button nr-button-primary" href="/dashboard">Get Started</a>
          <a class="nr-button nr-button-secondary" href="https://github.com/decolua/9router#readme" rel="noopener noreferrer">Read Documentation</a>
        </div>
      </div>
      <div class="nr-hero-panel" aria-label="Route flow">
        <div class="nr-flow-row"><span>CLI and SDKs</span><strong>/v1</strong></div>
        <div class="nr-flow-line"></div>
        <div class="nr-flow-row nr-flow-row-active"><span>9Router Hub</span><strong>20128</strong></div>
        <div class="nr-flow-line"></div>
        <div class="nr-provider-cloud">
          <span>OpenAI</span><span>Anthropic</span><span>Gemini</span><span>Local</span>
        </div>
      </div>
    </section>
    <section class="nr-top-band" id="get-started">
      <div>
        <h2>Get Started in 30 Seconds</h2>
        <p>Install 9Router, open the dashboard, configure providers, and point your tools to the local endpoint.</p>
        <ol class="nr-steps">
          <li><strong>Install 9Router</strong><span>Run the npx command to start the server.</span></li>
          <li><strong>Open Dashboard</strong><span>Configure providers and API keys.</span></li>
          <li><strong>Route Requests</strong><span>Use http://localhost:20128 as your base URL.</span></li>
        </ol>
      </div>
      <pre class="nr-terminal"><code>$ npx 9router
&gt; Starting 9Router...
&gt; Server running on http://localhost:20128
&gt; Dashboard: http://localhost:20128/dashboard
&gt; Ready to route.</code></pre>
    </section>
    <section class="nr-top-band nr-how" id="how-it-works">
      <h2>How 9Router Works</h2>
      <div class="nr-feature-grid nr-feature-grid-three">
        <article><span class="nr-card-icon">CLI</span><h3>CLI and SDKs</h3><p>Your requests start from your preferred tools. Change only the base URL.</p></article>
        <article><span class="nr-card-icon">9</span><h3>9Router Hub</h3><p>The local router selects configured providers and keeps one endpoint stable.</p></article>
        <article><span class="nr-card-icon">AI</span><h3>AI Providers</h3><p>Requests reach OpenAI, Anthropic, Gemini, local models, or future providers.</p></article>
      </div>
    </section>
    <section class="nr-top-band" id="features">
      <h2>Powerful Features</h2>
      <div class="nr-feature-grid">
        <article><span class="nr-card-icon">URL</span><h3>Unified Endpoint</h3><p>Access all providers through one standard API URL.</p></article>
        <article><span class="nr-card-icon">KEY</span><h3>OAuth &amp; API Keys</h3><p>Manage credentials and provider connections in one place.</p></article>
        <article><span class="nr-card-icon">USE</span><h3>Usage Tracking</h3><p>Track requests, tokens, and costs across models.</p></article>
        <article><span class="nr-card-icon">WEB</span><h3>Dashboard</h3><p>Use the web interface for providers, combos, and routing state.</p></article>
      </div>
    </section>
  </main>
</body>
</html>
"##;
