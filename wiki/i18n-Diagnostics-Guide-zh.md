# 🩺 NullRouter 详尽系统诊断与故障排查手册

本手册汇总了 NullRouter 在各种操作系统（Linux、macOS、Windows/WSL2）及网络环境下的完整故障诊断流程、全量错误码释义及针对性解决方案。

---

## 🧭 四步快速排错流程

当遇到请求失败、连接中断或流式输出异常时，请按顺序执行以下四步排查命令：

```bash
# 步骤 1：检查 Pingora 网关是否正常监听 20128 端口
curl -I http://127.0.0.1:20128/api/health

# 步骤 2：检查 Actix 状态服务及当前配置加载状态
curl -s http://127.0.0.1:20128/api/state | jq .status

# 步骤 3：检查上游提供商连接池与模型统一目录
curl -s http://127.0.0.1:20128/v1/models | jq '.data | length'

# 步骤 4：测试流式 SSE 推理链路（零拷贝输出验证）
curl -N -X POST http://127.0.0.1:20128/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"auto","messages":[{"role":"user","content":"ping"}],"stream":true}'
```

---

## 📚 错误代码大全与速查字典

| 错误代码 | 所属子系统 | 故障描述 | 建议排查与恢复动作 |
| :--- | :--- | :--- | :--- |
| **`E1001`** | 网关 (Pingora) | 端口 `20128` 已被其他进程占用 | 查找并终止冲突进程，或使用 `--port` 更换端口 |
| **`E1002`** | 网关 (Pingora) | 上游 API 连接被拒 / DNS 解析失败 | 检查网络连通性、本地代理环境变量或企业防火墙 |
| **`E1003`** | 网关 (Pingora) | 网关 504 超时（上游超过 60 秒未响应） | 查看服务商状态页，或检查模型推理负载 |
| **`E1004`** | 网关 (Pingora) | 客户端异常断开 (收到 TCP FIN/RST) | 通常为用户在 IDE 中主动取消或点击了 Stop |
| **`E2001`** | 运行时 (Actix) | 当前组合策略或分层中无可用健康路由 | 请在控制台中至少配置一个有效提供商的 API Key |
| **`E2002`** | 运行时 (Actix) | 上游认证失败 (HTTP 401/403) | 检查 API Key 是否输入错误、过期或欠费 |
| **`E2003`** | 运行时 (Actix) | 触发 429 速率限制 (全部账号均受限) | 启用多账号轮询或配置第 2/3 层级（便宜/免费）自动兜底 |
| **`E2004`** | 运行时 (Actix) | 超过上下文最大长度限制 | 开启 RTK Token 智能压缩功能，或清理过长历史记录 |
| **`E3001`** | 协议转换 (SSE) | 上游返回畸形 JSON 块或非标准响应 | 检查是否存在企业网络认证页面（Captive Portal）劫持 |
| **`E3002`** | 协议转换 (SSE) | 缓冲区溢出 / CRLF 分帧解析异常 | 检查上游服务商兼容性；升级 NullRouter 至最新版本 |
| **`E3003`** | 协议转换 (SSE) | 工具调用 (Tool Call) 参数结构不匹配 | 验证客户端 IDE 的工具定义与模型支持情况 |
| **`E4001`** | 状态中心 (Actix)| 状态文件互斥锁争用 | 确保没有重复启动多个 NullRouter 实例访问同一配置文件 |
| **`E4002`** | 状态中心 (Actix)| `nullrouter-state.json` 文件损坏 | 从备份恢复或删除并重新自动生成干净的状态文件 |
| **`E4003`** | 状态中心 (Actix)| 历史遗留配置格式迁移异常 | 运行迁移工具或清理缓存临时文件 |

---

## 🔍 重点疑难问题深度剖析

### 1. 端口占用排查 (错误 `E1001`)
当终端提示 `Address already in use` 时：

**Linux 环境：**
```bash
sudo ss -tulpn | grep 20128
# 或
sudo lsof -i :20128
# 终止对应进程：
sudo kill -9 <PID>
```

**macOS 环境：**
```bash
sudo lsof -nP -iTCP:20128 -sTCP:LISTEN
sudo kill -9 <PID>
```

**Windows / WSL2 环境：**
```powershell
netstat -ano | findstr :20128
taskkill /F /PID <PID>
```

---

### 2. 企业代理与抓包软件证书冲突 (错误 `E1002`)
在配置了企业 VPN（如 Zscaler、深信服）或开发抓包代理时，Pingora 可能会因证书链未受信而拒绝握手。
- 设置证书文件环境变量：
  ```bash
  export SSL_CERT_FILE="/etc/ssl/certs/ca-certificates.crt"
  ```
- 确保本地回路地址不走外部代理：
  ```bash
  export NO_PROXY="localhost,127.0.0.1,::1"
  ```

---

### 3. 上游限流无感容灾 (错误 `E2003`)
开发过程中经常遇到 Claude 或 OpenAI 速率限制：
1. **添加多账号轮询**：在控制台 **Providers** 中为同一提供商添加多个 API 密钥，NullRouter 会自动通过权重分发负载，并在某个 Key 触发 429 时自动隔离该 Key 60 秒。
2. **多层级智能兜底**：设置 `auto` 组合：
   `Claude 4 (订阅优先) ➔ DeepSeek R1 (便宜兜底) ➔ Kiro AI / OpenCode (完全免费)`。
   触发 429 时客户端 IDE 完全无感知，直接平滑继续输出代码。

---

### 4. 反向代理流式缓冲阻断 (错误 `E3001`)
若将 NullRouter 放置于 Nginx 或云端反代之后，必须在 Nginx 配置中禁用缓冲区，否则会导致 SSE 无法实时流式打字：
```nginx
location /v1/ {
    proxy_pass http://127.0.0.1:20128;
    proxy_buffering off;
    proxy_cache off;
    proxy_read_timeout 600s;
}
```

---

### 5. 系统文件描述符上限 (`ulimit -n`)
在并发 Agent 密集调用的高负载场景下，若出现 `Too many open files`：
```bash
# 查看当前限制
ulimit -n

# 临时提升上限为 65535
ulimit -n 65535
./run.sh
```
