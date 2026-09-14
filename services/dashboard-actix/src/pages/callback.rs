//! The provider OAuth callback shell: a mount point for the Leptos/WASM bundle.

pub(crate) const CALLBACK_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>nullrouter OAuth Callback</title>
  <meta name="description" content="Complete OAuth authorization for nullrouter">
  <meta name="color-scheme" content="light dark">
  <link rel="stylesheet" href="/assets/dashboard.css">
  <link rel="stylesheet" href="/assets/dashboard/app.css">
  <link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">
  <link rel="modulepreload" href="/pkg/dashboard_leptos.js">
</head>
<body>
  <div id="dashboard-root"></div>
  <noscript>
    <main>
      <div>
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
