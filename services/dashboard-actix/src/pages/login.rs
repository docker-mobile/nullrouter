//! The sign-in shell.
//!
//! This page used to carry 162 lines of inline JavaScript: the status probe, the
//! submit handler, the password-change flow, the lockout countdown, the error
//! mapping, and the post-login redirect sanitiser. None of it was type-checked and
//! only a browser could exercise it.
//!
//! It now serves a mount point for the same Leptos/WASM bundle the dashboard uses.
//! The logic lives in `apps/dashboard-leptos`
//! (`dashboard::login_live` for the derivations, `ui::login` for the markup),
//! where the redirect sanitiser and the auth-skip check have unit tests.
//!
//! What stays here is what must work before any WASM loads: the document shell,
//! the stylesheet, and a `<noscript>` that says why the screen is blank. There is
//! deliberately no fallback form — a form that posted without the bundle would
//! bypass the sanitiser this page exists to apply.

pub(crate) const LOGIN_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>nullrouter Login</title>
  <meta name="description" content="Sign in to the nullrouter dashboard">
  <link rel="stylesheet" href="/assets/dashboard.css">
  <link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">
  <link rel="modulepreload" href="/pkg/dashboard_leptos.js">
</head>
<body class="nr-top-page nr-auth-page">
  <div id="dashboard-root"></div>
  <noscript>
    <main class="nr-auth-wrap">
      <section class="nr-auth-panel">
        <div class="nr-auth-head">
          <span class="nr-logo-mark">9</span>
          <h1>nullrouter</h1>
          <p>Sign-in needs JavaScript and WebAssembly enabled.</p>
        </div>
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
