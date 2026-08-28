# Catalog Actix Service

Actix Web catalog and state defaults service for the nullrouter microservice port.

Run it locally:

```bash
cargo run -p nullrouter-catalog --bin nullrouter-catalog
```

The default bind is `127.0.0.1:20131`. Override it with `NULLROUTER_CATALOG_HOST` and `NULLROUTER_CATALOG_PORT`; `PORT` also overrides the port for process-manager compatibility.

Routes:

- `GET /health`
- `GET /api/catalog` (same payload as `/api/catalog/routes`)
- `GET /api/catalog/routes`
- `GET /api/catalog/providers`
- `GET /api/state/settings`
- `GET /api/state/keys`
- `GET /api/state/usage`
