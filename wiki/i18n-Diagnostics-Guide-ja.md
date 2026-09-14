# 🩺 NullRouter 障害診断・トラブルシューティング完全ガイド

本ガイドは、NullRouterの運用時に発生するネットワークエラー、ストリーミング（SSE）の中断、レート制限、設定の不整合を迅速に特定し復旧するための公式手順書です。

---

## 🧭 4ステップ・クイック診断手順

問題が発生した場合は、まず以下の4つのコマンドを順番に実行してください：

```bash
# ステップ 1: Pingora ゲートウェイの健全性確認（ポート 20128）
curl -I http://127.0.0.1:20128/api/health

# ステップ 2: Actix 状態管理サービスと設定の正常性確認
curl -s http://127.0.0.1:20128/api/state | jq .status

# ステップ 3: 有効なプロバイダー接続数とモデル一覧の取得
curl -s http://127.0.0.1:20128/v1/models | jq '.data | length'

# ステップ 4: SSE ストリーミング推論パイプラインの動作テスト
curl -N -X POST http://127.0.0.1:20128/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"auto","messages":[{"role":"user","content":"ping"}],"stream":true}'
```

---

## 📚 エラーコード早見表

| エラーコード | 該当コンポーネント | 現象の概要 | 推奨される復旧手順 |
| :--- | :--- | :--- | :--- |
| **`E1001`** | Gateway (Pingora) | ポート `20128` が他のプロセスで使用中 | 競合プロセスを終了するか、`--port` で別ポートを指定 |
| **`E1002`** | Gateway (Pingora) | 外部接続拒否 / DNS 解決エラー | ネットワーク接続、プロキシ設定、ファイアウォールを確認 |
| **`E1003`** | Gateway (Pingora) | 504 Gateway Timeout（上流応答なし） | プロバイダーの稼働状況を確認するか、タイムアウト値を延長 |
| **`E1004`** | Gateway (Pingora) | クライアント側からの切断 | IDE側でリクエストがキャンセルされた正常動作 |
| **`E2001`** | Runtime (Actix) | 利用可能なアクティブルートが存在しない | ダッシュボードで有効なプロバイダーAPIキーを登録 |
| **`E2002`** | Runtime (Actix) | プロバイダー認証失敗（HTTP 401/403） | APIキーの有効期限、入力ミス、利用枠残高を確認 |
| **`E2003`** | Runtime (Actix) | 429 Too Many Requests（レート制限到達） | 複数キー分散（ラウンドロビン）や無料モデルへの自動切り替えを有効化 |
| **`E2004`** | Runtime (Actix) | コンテキストトークン長の上限超過 | RTKトークン圧縮を有効化するか、プロンプト履歴を整理 |
| **`E3001`** | Translate (SSE) | 不正なJSONチャンクまたはHTMLの混入 | キャプティブポータルやVPN等によるHTTP傍受を確認 |
| **`E3002`** | Translate (SSE) | バッファオーバーラン / CRLF分割エラー | NullRouter を最新バージョンへ更新 |
| **`E3003`** | Translate (SSE) | ツール呼び出し（Tool Call）スキーマ不一致 | IDE側のツール定義とモデル対応状況を確認 |
| **`E4001`** | State (Actix) | 設定ファイルのロック競合 | NullRouterプロセスが重複起動していないか確認 |
| **`E4002`** | State (Actix) | `nullrouter-state.json` の破損 | バックアップから復元するか、クリーン再生成 |
| **`E4003`** | State (Actix) | 旧バージョンからの設定移行エラー | 一時キャッシュをクリアし設定ウィザードを実行 |

---

## 🔍 主要トラブルシューティング詳細

### ポート競合の解決 (エラー `E1001`)
`Address already in use` が表示される場合：
- **Linux**: `sudo ss -tulpn | grep 20128` ➔ `sudo kill -9 <PID>`
- **macOS**: `sudo lsof -nP -iTCP:20128 -sTCP:LISTEN` ➔ `sudo kill -9 <PID>`
- **Windows**: `netstat -ano | findstr :20128` ➔ `taskkill /F /PID <PID>`

### Nginxリバースプロキシのバッファリング問題 (エラー `E3001`)
Nginxを経由させる場合は、ストリーミングを妨げないよう設定ブロックに以下を追加してください：
```nginx
proxy_buffering off;
proxy_cache off;
```
