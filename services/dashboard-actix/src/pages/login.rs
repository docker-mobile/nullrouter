pub(crate) const LOGIN_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>9Router Login</title>
  <meta name="description" content="Sign in to the 9Router dashboard">
  <link rel="stylesheet" href="/assets/dashboard.css">
  <link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">
</head>
<body class="nr-top-page nr-auth-page">
  <main class="nr-auth-wrap">
    <section class="nr-auth-panel" aria-labelledby="login-title">
      <div class="nr-auth-head">
        <span class="nr-logo-mark">9</span>
        <h1 id="login-title">9Router</h1>
        <p id="login-copy">Enter your password to access the dashboard</p>
      </div>
      <form id="password-form" class="nr-auth-form" method="post" action="/api/auth/login">
        <label for="password">Password</label>
        <input id="password" name="password" type="password" autocomplete="current-password" placeholder="Enter password" required>
        <label id="new-password-row" class="nr-auth-hidden" for="new-password">New password</label>
        <input id="new-password" class="nr-auth-hidden" name="newPassword" type="password" autocomplete="new-password" placeholder="Set new password">
        <p id="auth-error" class="nr-auth-error" role="alert"></p>
        <p id="auth-hint" class="nr-auth-hint">Default password is <code>123456</code></p>
        <button id="login-button" class="nr-button nr-button-primary" type="submit">Login</button>
      </form>
      <button id="oidc-button" class="nr-button nr-button-secondary nr-auth-hidden" type="button">Sign in with OIDC</button>
    </section>
  </main>
  <script>
    const form = document.getElementById("password-form");
    const password = document.getElementById("password");
    const newPassword = document.getElementById("new-password");
    const newPasswordRow = document.getElementById("new-password-row");
    const button = document.getElementById("login-button");
    const error = document.getElementById("auth-error");
    const hint = document.getElementById("auth-hint");
    const copy = document.getElementById("login-copy");
    const oidcButton = document.getElementById("oidc-button");
    const resetHint = "Forgot password? Reset to default via 9Router CLI -> Settings -> Reset Password to Default.";
    let mustChangePassword = false;
    let submitting = false;
    let retryAfter = 0;
    let retryInterval = 0;
    function setError(message) {
      error.textContent = message || "";
    }
    function dashboardTarget() {
      const requestedTarget = new URLSearchParams(window.location.search).get("next");
      if (!requestedTarget) return "/dashboard";
      try {
        const target = new URL(requestedTarget, window.location.origin);
        const isDashboard = target.pathname === "/dashboard" || target.pathname.startsWith("/dashboard/");
        if (target.origin !== window.location.origin || !isDashboard) return "/dashboard";
        return `${target.pathname}${target.search}${target.hash}`;
      } catch (_error) {
        return "/dashboard";
      }
    }
    function redirectToDashboard() {
      window.location.replace(dashboardTarget());
    }
    function renderButton() {
      button.disabled = submitting || retryAfter > 0;
      if (retryAfter > 0) {
        button.textContent = `Wait ${retryAfter}s`;
      } else if (submitting) {
        button.textContent = mustChangePassword ? "Saving..." : "Logging in...";
      } else {
        button.textContent = mustChangePassword ? "Set password" : "Login";
      }
    }
    function retryAfterSeconds(response, data) {
      const rawSeconds = response.headers.get("Retry-After") || data.retryAfter;
      const seconds = Number(rawSeconds);
      if (!Number.isFinite(seconds) || seconds <= 0) return 0;
      return Math.min(Math.max(Math.ceil(seconds), 0), 3600);
    }
    function startRetryCountdown(seconds) {
      window.clearInterval(retryInterval);
      retryAfter = seconds;
      renderButton();
      if (retryAfter <= 0) return;
      retryInterval = window.setInterval(() => {
        retryAfter -= 1;
        if (retryAfter <= 0) {
          retryAfter = 0;
          window.clearInterval(retryInterval);
          retryInterval = 0;
        }
        renderButton();
      }, 1000);
    }
    function boundedLoginError(response, data) {
      if (response.status === 401) {
        const remaining = Number(data.remainingBeforeLock);
        if (Number.isInteger(remaining) && remaining >= 0 && remaining <= 100) {
          return `Invalid password. ${remaining} attempt(s) left before lockout.`;
        }
        return "Invalid password.";
      }
      if (response.status === 429) return "Too many failed attempts. Try again later.";
      if (response.status === 403) return "Password login is unavailable.";
      if (response.status === 400) return "Enter a valid password.";
      return "Unable to sign in. Please try again.";
    }
    function setMustChange() {
      mustChangePassword = true;
      copy.textContent = "Set a new password before accessing the dashboard remotely";
      hint.textContent = "Choose a replacement password before continuing.";
      newPassword.classList.remove("nr-auth-hidden");
      newPasswordRow.classList.remove("nr-auth-hidden");
      newPassword.required = true;
      newPassword.focus();
      renderButton();
    }
    async function loadStatus() {
      const controller = new AbortController();
      const timeout = window.setTimeout(() => controller.abort(), 5000);
      try {
        const response = await fetch("/api/auth/status", {
          credentials: "same-origin",
          signal: controller.signal,
        });
        if (!response.ok) return;
        const status = await response.json();
        // An existing session is the ONLY thing that skips this screen. This
        // deliberately does not honour a `requireLogin: false`: login is
        // unconditional in nullrouter, so a response carrying that flag would be
        // either a stale build or a spoofed one, and acting on it would be an
        // auth bypass driven by a JSON field.
        if (status.authenticated === true) {
          redirectToDashboard();
          return;
        }
        const oidcReady = status.oidcConfigured === true && ["oidc", "both"].includes(status.authMode);
        if (oidcReady) {
          oidcButton.textContent = status.oidcLoginLabel || "Sign in with OIDC";
          oidcButton.classList.remove("nr-auth-hidden");
        }
        if (status.authMode === "oidc" && oidcReady) {
          copy.textContent = "Sign in with your OIDC provider to access the dashboard";
          form.classList.add("nr-auth-hidden");
        }
      } catch (_error) {
        return;
      } finally {
        window.clearTimeout(timeout);
      }
    }
    oidcButton.addEventListener("click", () => {
      window.location.href = "/api/auth/oidc/start";
    });
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      if (submitting || retryAfter > 0) return;
      submitting = true;
      setError("");
      renderButton();
      try {
        const body = mustChangePassword ? { currentPassword: password.value, newPassword: newPassword.value } : { password: password.value };
        const response = await fetch(mustChangePassword ? "/api/settings" : "/api/auth/login", {
          method: mustChangePassword ? "PATCH" : "POST",
          headers: { "Content-Type": "application/json" },
          credentials: "same-origin",
          body: JSON.stringify(body),
        });
        const parsed = await response.json().catch(() => ({}));
        const data = parsed && typeof parsed === "object" ? parsed : {};
        if (response.ok) {
          if (data.mustChangePassword) {
            setMustChange();
            return;
          }
          redirectToDashboard();
          return;
        }
        setError(boundedLoginError(response, data));
        if (response.status === 429) {
          hint.textContent = resetHint;
          startRetryCountdown(retryAfterSeconds(response, data));
        }
      } catch (_error) {
        setError("Sign-in service is unavailable. Please try again.");
      } finally {
        submitting = false;
        renderButton();
      }
    });
    loadStatus();
  </script>
</body>
</html>
"#;
