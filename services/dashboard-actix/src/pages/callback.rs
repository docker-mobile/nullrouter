//! The provider OAuth callback shell.
//!
//! This page used to carry 59 lines of inline JavaScript: the query parse, the
//! three relay channels (`postMessage`, `BroadcastChannel`, `localStorage`), the
//! panel switch, and the close timer. None of it was type-checked.
//!
//! It now serves a mount point for the same Leptos/WASM bundle the dashboard and
//! sign-in screen use. The logic lives in `apps/dashboard-leptos`
//! (`dashboard::callback_live` for the derivations, `ui::callback` for the markup),
//! where the relay-origin decision is unit-tested — it hands over an authorization
//! code, so who may receive it is worth a test.
//!
//! What stays here is the document shell and a `<noscript>` that still shows the
//! URL to copy by hand, since that fallback needs no script to be useful.

pub(crate) const CALLBACK_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>nullrouter OAuth Callback</title>
  <meta name="description" content="Complete OAuth authorization for nullrouter">
  <link rel="stylesheet" href="/assets/dashboard.css">
  <link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">
  <link rel="modulepreload" href="/pkg/dashboard_leptos.js">
</head>
<body class="nr-top-page nr-callback-page">
  <div id="dashboard-root"></div>
  <noscript>
    <main class="nr-callback-panel">
      <div class="nr-callback-state">
        <span class="nr-callback-icon nr-callback-warn">URL</span>
        <h1>Copy This URL</h1>
        <p>Please copy the URL from the address bar and paste it in the application.</p>
      </div>
    </main>
  </noscript>
  <script type="module">
    import init from "/pkg/dashboard_leptos.js";
    await init("/pkg/dashboard_leptos_bg.wasm");
  </script>
</body>
</html>
"#;
