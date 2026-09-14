//! The sign-in shell: a mount point for the Leptos/WASM bundle.

pub(crate) const LOGIN_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>nullrouter Login</title>
  <meta name="description" content="Sign in to the nullrouter dashboard">
  <meta name="color-scheme" content="light dark">
  <link rel="stylesheet" href="/assets/dashboard.css">
  <link rel="stylesheet" href="/assets/dashboard/app.css">
  <link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">
  <link rel="modulepreload" href="/pkg/dashboard_leptos.js">
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
<body>
  <div id="dashboard-root"></div>
  <noscript>
    <main>
      <section>
        <h1>nullrouter</h1>
        <p>Sign-in needs JavaScript and WebAssembly enabled.</p>
      </section>
    </main>
  </noscript>
  <script type="module">
    import init from "/pkg/dashboard_leptos.js";
    await init("/pkg/dashboard_leptos_bg.wasm");
  </script>
</body>
</html>
"#;
