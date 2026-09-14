# 🩺 Manual de Diagnóstico y Resolución de Problemas de NullRouter

Esta guía proporciona un flujo sistemático para identificar y solucionar errores de red, transmisión en streaming (SSE), límites de tasa y conflictos de configuración en NullRouter.

---

## 🧭 Flujo Rápido de Triage en 4 Pasos

Ejecuta estos 4 comandos en tu terminal para verificar el estado de los componentes:

```bash
# Paso 1: Comprobar el gateway Pingora en el puerto 20128
curl -I http://127.0.0.1:20128/api/health

# Paso 2: Comprobar el estado interno de los microservicios Actix
curl -s http://127.0.0.1:20128/api/state | jq .status

# Paso 3: Verificar los proveedores configurados y modelos activos
curl -s http://127.0.0.1:20128/v1/models | jq '.data | length'

# Paso 4: Probar la transmisión de tokens en streaming (SSE)
curl -N -X POST http://127.0.0.1:20128/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"auto","messages":[{"role":"user","content":"hola"}],"stream":true}'
```

---

## 📚 Diccionario Completo de Códigos de Error

| Código | Subsistema | Descripción | Acción de Recuperación |
| :--- | :--- | :--- | :--- |
| **`E1001`** | Gateway (Pingora) | Puerto `20128` ocupado por otro proceso | Detener el proceso en conflicto o cambiar de puerto con `--port` |
| **`E1002`** | Gateway (Pingora) | Conexión rechazada / Error de resolución DNS | Verificar conexión a internet, proxies o cortafuegos |
| **`E1003`** | Gateway (Pingora) | Gateway Timeout 504 (>60s sin respuesta) | Verificar estado del proveedor o latencia de red |
| **`E1004`** | Gateway (Pingora) | Cliente desconectado abruptamente | Petición cancelada por el usuario en el editor/IDE |
| **`E2001`** | Runtime (Actix) | No hay rutas activas disponibles | Añadir al menos una clave API válida en el panel de control |
| **`E2002`** | Runtime (Actix) | Autenticación fallida (HTTP 401/403) | Comprobar validez de la clave API o saldo de la cuenta |
| **`E2003`** | Runtime (Actix) | Límite de tasa 429 en todos los proveedores | Habilitar rotación multi-cuenta o capa gratuita (Kiro/OpenCode) |
| **`E2004`** | Runtime (Actix) | Límite de tokens de contexto superado | Activar compresión RTK en el panel |
| **`E3001`** | Translate (SSE) | Fragmento JSON malformado de origen | Verificar si hay un portal cautivo interfiriendo |
| **`E3002`** | Translate (SSE) | Desbordamiento de búfer / Error CRLF | Actualizar NullRouter a la versión más reciente |
| **`E3003`** | Translate (SSE) | Incompatibilidad de esquema de llamada a herramienta | Revisar definición de herramientas del IDE |
| **`E4001`** | State (Actix) | Conflicto de bloqueo de archivo de estado | Asegurarse de ejecutar solo una instancia de NullRouter |
| **`E4002`** | State (Actix) | Archivo `nullrouter-state.json` corrupto | Restaurar desde copia de seguridad o regenerar |
| **`E4003`** | State (Actix) | Error de migración de configuración heredada | Limpiar caché temporal o reiniciar el asistente |

---

## 🔍 Problemas Frecuentes y Soluciones

### Conflicto de Puerto (Error `E1001`)
Si el puerto `20128` está en uso:
- **Linux**: `sudo ss -tulpn | grep 20128` seguido de `sudo kill -9 <PID>`
- **macOS**: `sudo lsof -nP -iTCP:20128 -sTCP:LISTEN` y `sudo kill -9 <PID>`
- **Windows**: `netstat -ano | findstr :20128` y `taskkill /F /PID <PID>`

### Búfer de Nginx rompiendo el Streaming SSE (Error `E3001`)
Si ejecutas NullRouter detrás de Nginx, añade:
```nginx
proxy_buffering off;
proxy_cache off;
```
