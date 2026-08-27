pub(crate) const CALLBACK_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>9Router OAuth Callback</title>
  <meta name="description" content="Complete OAuth authorization for 9Router">
  <link rel="stylesheet" href="/assets/dashboard.css">
  <link rel="icon" href="/assets/favicon.svg" type="image/svg+xml">
</head>
<body class="nr-top-page nr-callback-page">
  <main class="nr-callback-panel" aria-live="polite">
    <div id="processing" class="nr-callback-state">
      <span class="nr-callback-icon">...</span>
      <h1>Processing...</h1>
      <p>Please wait while we complete the authorization.</p>
    </div>
    <div id="success" class="nr-callback-state nr-auth-hidden">
      <span class="nr-callback-icon nr-callback-ok">OK</span>
      <h1>Authorization Successful!</h1>
      <p id="success-copy">This window will close automatically...</p>
    </div>
    <div id="manual" class="nr-callback-state nr-auth-hidden">
      <span class="nr-callback-icon nr-callback-warn">URL</span>
      <h1>Copy This URL</h1>
      <p>Please copy the URL from the address bar and paste it in the application.</p>
      <code id="callback-url"></code>
    </div>
  </main>
  <script>
    const params = new URLSearchParams(window.location.search);
    const callbackData = {
      code: params.get("code"),
      token: params.get("token"),
      state: params.get("state"),
      error: params.get("error"),
      errorDescription: params.get("error_description"),
      fullUrl: window.location.href,
    };
    const expectedOrigins = [
      window.location.origin,
      "http://localhost:1455",
    ];
    const states = {
      processing: document.getElementById("processing"),
      success: document.getElementById("success"),
      manual: document.getElementById("manual"),
    };
    function show(name) {
      for (const [key, element] of Object.entries(states)) {
        element.classList.toggle("nr-auth-hidden", key !== name);
      }
    }
    function relayCallback() {
      if (window.opener) {
        for (const origin of expectedOrigins) {
          try {
            window.opener.postMessage({ type: "oauth_callback", data: callbackData }, origin);
          } catch (error) {
            console.log("postMessage failed:", error);
          }
        }
      }
      try {
        const channel = new BroadcastChannel("oauth_callback");
        channel.postMessage(callbackData);
        channel.close();
      } catch (error) {
        console.log("BroadcastChannel failed:", error);
      }
      try {
        localStorage.setItem("oauth_callback", JSON.stringify({ ...callbackData, timestamp: Date.now() }));
      } catch (error) {
        console.log("localStorage failed:", error);
      }
    }
    relayCallback();
    if (!(callbackData.code || callbackData.token || callbackData.error)) {
      document.getElementById("callback-url").textContent = window.location.href;
      show("manual");
    } else {
      show("success");
      setTimeout(() => {
        window.close();
        document.getElementById("success-copy").textContent = "You can close this tab now.";
      }, 1500);
    }
  </script>
</body>
</html>
"#;
