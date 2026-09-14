# 🔒 Security, Privacy & Credential Storage

NullRouter is designed with strict security standards: zero telemetry, 100% local operation, and enterprise-grade secret masking.

---

## 🛡️ Core Security Principles

1. **Zero External Telemetry**: NullRouter never phones home, collects prompt statistics, or reports usage metrics to any third-party server.
2. **Localhost First**: By default, Pingora binds strictly to `127.0.0.1:20128`. It is never exposed to public networks unless explicitly configured.
3. **Zero Request/Response Persistence**: Prompt texts, tool execution results, and completions pass through RAM buffers and are immediately discarded. NullRouter never writes conversation history to disk.
4. **Masked Credential Logging**: All API keys, authorization bearer headers, and OAuth tokens are redacted in console logs and trace output (e.g. `sk-ant-***-wxyz`).

---

## 🔑 Credential Storage & Encryption

Credentials configured via the WebAssembly Dashboard are stored in `nullrouter-state.json`:
- **File System Permissions**: On UNIX systems, `nullrouter-state.json` is created with permissions restricted to the owning user (`chmod 600`).
- **Encrypted Secrets Support**: API keys can be passed via environment variables (e.g. `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`) to prevent saving keys to disk in CI/CD or shared container environments.

---

## 🌐 Deploying to Remote Servers (TLS Termination)

If deploying NullRouter on a central office server or remote VPS:
- **Pingora TLS**: Use Pingora's native TLS termination by pointing `tls_cert` and `tls_key` in the gateway configuration.
- **Authentication**: Set an admin bearer token in `auth-actix` to require downstream clients to supply a valid API key:
  ```bash
  export NULLROUTER_AUTH_TOKEN="my-secure-cluster-key-98273"
  ```
