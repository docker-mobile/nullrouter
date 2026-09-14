# 🩺 Exhaustive Diagnostics & Troubleshooting Playbook

This is the definitive diagnostic manual for NullRouter. When encountering any unexpected behavior, network failure, streaming drop, or configuration issue, use this step-by-step playbook to triage, diagnose, and resolve the root cause.

---

## 🧭 Systematic 4-Step Triage Workflow

Before diving into specific error codes, run this 4-step triage sequence to pinpoint the subsystem responsible:

```
[Issue Encountered]
        │
        ├─► 1. Network & Socket Check: Is Pingora listening on port 20128?
        │      curl -I http://127.0.0.1:20128/api/health
        │
        ├─► 2. Internal Microservice Check: Are Actix runtime and state services responsive?
        │      curl -s http://127.0.0.1:20128/api/state | jq .status
        │
        ├─► 3. Upstream Provider Connectivity: Can NullRouter reach external AI APIs?
        │      curl -s http://127.0.0.1:20128/v1/models | jq '.data | length'
        │
        └─► 4. Streaming & SSE Pipeline: Does a test prompt stream tokens without truncation?
               curl -N -X POST http://127.0.0.1:20128/v1/chat/completions \
                 -H "Content-Type: application/json" \
                 -d '{"model":"auto","messages":[{"role":"user","content":"test"}],"stream":true}'
```

---

## 📚 Error Code Reference Encyclopedia

| Code | Subsystem | Description | Quick Recovery Action |
| :--- | :--- | :--- | :--- |
| **`E1001`** | Gateway (Pingora) | Port `20128` already bound by another process | Kill existing process or specify alternative port |
| **`E1002`** | Gateway (Pingora) | Upstream connection refused / DNS resolution failure | Check internet connectivity, DNS, or proxy vars |
| **`E1003`** | Gateway (Pingora) | Gateway Timeout 504 (Upstream response exceeded 60s) | Check provider status page or increase timeout |
| **`E1004`** | Gateway (Pingora) | Downstream client disconnect (TCP FIN/RST received) | IDE aborted request (e.g. user cancelled prompt) |
| **`E2001`** | Runtime (Actix) | No available route in requested combo/tier | Add API keys in Dashboard for at least one provider |
| **`E2002`** | Runtime (Actix) | Provider authentication failure (HTTP 401/403) | Re-enter API key; check organization ID |
| **`E2003`** | Runtime (Actix) | 429 Too Many Requests across all accounts | Add secondary key or enable Cheap/Free fallback tier |
| **`E2004`** | Runtime (Actix) | Context length exceeded (Prompt exceeds model limit) | Enable RTK token compaction; trim context in IDE |
| **`E3001`** | Translate (SSE) | Malformed JSON chunk or unexpected HTML from upstream | Check if corporate captive portal is intercepting HTTP |
| **`E3002`** | Translate (SSE) | Line buffer overrun / Split CRLF framing error | Check upstream provider compliance; update NullRouter |
| **`E3003`** | Translate (SSE) | Tool call schema mismatch or missing tool ID | Verify agent IDE tool schema compatibility |
| **`E4001`** | State (Actix) | State file lock contention | Ensure only one NullRouter instance accesses state |
| **`E4002`** | State (Actix) | Corrupted `nullrouter-state.json` file | Restore from backup or regenerate clean state |
| **`E4003`** | State (Actix) | Legacy migration format error | Run automatic state migrator or clean temp cache |

---

## 🔍 Deep-Dive Diagnostic Playbooks

### 1. Error `E1001`: Port Collision (Port 20128 Already Bound)

#### Symptom:
NullRouter fails to launch with `Address already in use (os error 98)` or `bind failed for 127.0.0.1:20128`.

#### Diagnosis:
Find the process currently holding port `20128`:

**On Linux:**
```bash
# Using ss
sudo ss -tulpn | grep 20128

# Using lsof
sudo lsof -i :20128

# Using fuser
sudo fuser 20128/tcp
```

**On macOS:**
```bash
sudo lsof -nP -iTCP:20128 -sTCP:LISTEN
```

**On Windows (PowerShell / Command Prompt):**
```powershell
netstat -ano | findstr :20128
# Identify PID in rightmost column, then check process name:
tasklist /fi "pid eq <PID>"
```

#### Resolution:
1. Kill the zombie process holding the port:
   ```bash
   # On Linux/macOS:
   sudo kill -9 <PID>
   ```
2. Or start NullRouter on a different port:
   ```bash
   ./run.sh --port 20129
   # or
   export NULLROUTER_PORT=20129
   cargo run --release
   ```

---

### 2. Error `E1002` & `E1003`: DNS, Firewall & Corporate Proxy Issues

#### Symptom:
Gateway reports `connection refused` or `504 Gateway Timeout` when routing to Anthropic or OpenAI.

#### Diagnosis:
1. Test raw TCP connectivity to provider hostnames:
   ```bash
   # Test Anthropic API endpoint
   curl -v https://api.anthropic.com/v1/messages

   # Test OpenAI API endpoint
   curl -v https://api.openai.com/v1/models
   ```
2. Check if corporate proxy environment variables are intercepting requests:
   ```bash
   echo "HTTP_PROXY=$HTTP_PROXY"
   echo "HTTPS_PROXY=$HTTPS_PROXY"
   echo "ALL_PROXY=$ALL_PROXY"
   echo "NO_PROXY=$NO_PROXY"
   ```
   If a corporate SSL-inspecting proxy (e.g. Zscaler, BlueCoat, Netskope) is active, Pingora may reject untrusted root certificates.

#### Resolution:
1. Configure custom CA root in NullRouter:
   ```bash
   export SSL_CERT_FILE="/etc/ssl/certs/ca-certificates.crt"
   # Or append your corporate root CA to the system store.
   ```
2. Exclude localhost from proxy inspection:
   ```bash
   export NO_PROXY="localhost,127.0.0.1,::1"
   ```

---

### 3. Error `E2003`: Upstream Rate Limits (HTTP 429)

#### Symptom:
Prompts fail with `RateLimitError: 429 Too Many Requests` or tokens stop generating mid-sentence.

#### Diagnosis:
1. Check provider balance and tier in the Dashboard at `http://localhost:20128/dashboard`.
2. Inspect upstream rate limit response headers in logs:
   `anthropic-ratelimit-requests-remaining`, `retry-after`, or `x-ratelimit-remaining-tokens`.

#### Resolution:
1. **Enable Multi-Account Pooling**:
   Add a second API key for the same provider in **Dashboard ➔ Providers**. NullRouter will automatically balance requests round-robin and isolate keys that hit 429.
2. **Configure Multi-Tier Auto-Fallback**:
   Ensure Tier 2 (Cheap: DeepSeek, Groq, GLM) or Tier 3 (Free: Kiro AI, OpenCode) is enabled. When Tier 1 hits 429, NullRouter will transition seamlessly without failing the user's prompt.

---

### 4. Error `E3001` & `E3002`: Streaming SSE Drops & Truncation

#### Symptom:
The editor (e.g. Cursor or Claude Code) reports `Stream terminated prematurely` or `Unexpected end of JSON input`.

#### Diagnosis:
1. **Nginx / Reverse Proxy Buffering**:
   If NullRouter is deployed behind Nginx, Nginx buffers chunked SSE data by default, breaking streaming.
   **Fix**: Add `proxy_buffering off;` and `proxy_cache off;` to your Nginx location block:
   ```nginx
   location /v1/ {
       proxy_pass http://127.0.0.1:20128;
       proxy_http_version 1.1;
       proxy_set_header Connection "";
       proxy_buffering off;
       proxy_cache off;
       proxy_read_timeout 600s;
   }
   ```
2. **Cloudflare Proxy Buffering**:
   If routing through Cloudflare DNS, orange-cloud proxying buffers SSE streams. Disable proxying (set DNS record to "DNS Only" / grey cloud) or use WebSocket/gRPC rules.

---

### 5. Loopback Binding: IPv4 (`127.0.0.1`) vs IPv6 (`::1`)

#### Symptom:
`curl http://localhost:20128/v1/models` works, but an IDE tool configured with `http://localhost:20128` throws `ECONNREFUSED`.

#### Cause:
Modern Node.js (v18+) resolves `localhost` to IPv6 `::1` by default before IPv4 `127.0.0.1`. If the gateway is listening only on IPv4, connections to `localhost` will fail.

#### Resolution:
1. NullRouter's Pingora gateway binds dual-stack by default. Verify using:
   ```bash
   curl -I http://[::1]:20128/api/health
   curl -I http://127.0.0.1:20128/api/health
   ```
2. In your IDE config, explicitly use `http://127.0.0.1:20128/v1` instead of `localhost`.

---

### 6. Linux System Resource Limits (`ulimit -n` & Socket Exhaustion)

#### Symptom:
Under heavy concurrency (multiple agents running simultaneously), logs show `Too many open files (os error 24)`.

#### Resolution:
Check current file descriptor limits:
```bash
ulimit -n
```
If the limit is `1024`, elevate it in `/etc/security/limits.conf`:
```
* soft nofile 65535
* hard nofile 65535
```
Or set dynamically in your shell before launching:
```bash
ulimit -n 65535
./run.sh
```

---

### 7. Upgrading & Migrating from Legacy Configurations

#### Symptom:
Upgrading from earlier setups where legacy files (`~/.9router/9router-state.json` or `%APPDATA%/9router`) existed.

#### How NullRouter Handles Compatibility:
NullRouter includes native migration bridges that automatically:
1. Detect existing legacy configuration in `~/.9router/` or `%APPDATA%/9router`.
2. Seamlessly import provider API keys and model configurations into `~/.nullrouter/nullrouter-state.json`.
3. Retain backwards-compatible fallback paths so existing CLI aliases remain unbroken.
4. If you encounter permissions errors:
   ```bash
   # Ensure ~/.nullrouter permissions
   mkdir -p ~/.nullrouter
   chmod 700 ~/.nullrouter
   ```
