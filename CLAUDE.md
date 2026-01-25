# CLAUDE.md

## Project Overview

Browser Render Rust - GoプロジェクトからのRust移植版。ブラウザ自動化によるVehicleデータ取得とHono API連携サービス。

## Build & Run

```bash
# ビルド
cargo build                    # 開発
cargo build --release          # リリース
cargo build --features grpc    # gRPC機能付き

# 実行
cargo run -- --server http     # HTTPサーバー
cargo run -- --http-port 3000  # カスタムポート
cargo run -- --debug           # デバッグモード
```

## ログ設定

```bash
# JSON形式でファイル出力（本番環境推奨）
cargo run -- --log-format json --log-file app.log

# モジュール別ログレベル制御
RUST_LOG=browser_render::browser=debug,info cargo run
```

| CLI引数 | 環境変数 | デフォルト | 説明 |
|---------|----------|------------|------|
| `--log-format` | `LOG_FORMAT` | `text` | `text` / `json` |
| `--log-file` | `LOG_FILE` | (なし) | ファイル出力有効化 |
| `--log-dir` | `LOG_DIR` | `./logs` | ログディレクトリ |
| `--log-rotation` | `LOG_ROTATION` | `daily` | `daily` / `hourly` / `never` |
| - | `RUST_LOG` | (なし) | モジュール別レベル制御 |

## テスト

```bash
# 統合テスト（認証情報は.envから、順次実行必須）
cargo test --test browser_integration_test -- --ignored --nocapture --test-threads=1

# モックサーバー単体テスト
cargo test --test browser_integration_test test_mock_server_standalone -- --nocapture
```

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/v1/vehicle/data` | GET | Vehicleデータ取得ジョブ作成 |
| `/v1/etc/scrape` | POST | ETC明細スクレイプ（即時実行） |
| `/v1/etc/scrape/queue` | POST | ETC明細スクレイプ（idle時実行） |
| `/v1/etc/scrape/batch` | POST | ETC明細スクレイプ（複数アカウント、即時実行） |
| `/v1/etc/scrape/batch/queue` | POST | ETC明細スクレイプ（複数アカウント、idle時実行） |
| `/v1/etc/scrape/batch/env` | POST | ETC明細スクレイプ（環境変数からアカウント取得、即時実行） |
| `/v1/etc/scrape/batch/env/queue` | POST | ETC明細スクレイプ（環境変数からアカウント取得、idle時実行） |
| `/v1/job/:id` | GET | ジョブステータス確認 |
| `/v1/jobs` | GET | 全ジョブ一覧 |
| `/v1/jobs/queue` | GET | キュー状態確認 |
| `/health` | GET | ヘルスチェック |

### ETC Scrape Request Body（単一アカウント）
```json
{
  "user_id": "xxx",
  "password": "xxx",
  "download_path": "./downloads",
  "headless": true
}
```

### ETC Batch Scrape Request Body（複数アカウント）
```json
{
  "accounts": [
    {"user_id": "user1", "password": "pass1"},
    {"user_id": "user2", "password": "pass2"}
  ],
  "download_path": "./downloads",
  "headless": true
}
```

### 環境変数でのアカウント設定

| 環境変数 | 説明 | 例 |
|----------|------|-----|
| `ETC_ACCOUNTS` | アカウント情報（JSON配列） | `[{"user_id":"u1","password":"p1"}]` |
| `ETC_DOWNLOAD_PATH` | ダウンロード先パス | `/data/downloads` |

```bash
# .envファイル例
ETC_ACCOUNTS='[{"user_id":"user1","password":"pass1"},{"user_id":"user2","password":"pass2"}]'
ETC_DOWNLOAD_PATH=/data/etc-downloads
```

### 環境変数バッチエンドポイント
リクエストボディはオプション（download_path/headlessの上書き用）：
```bash
# 環境変数のみで実行
curl -X POST http://localhost:8080/v1/etc/scrape/batch/env

# download_pathを上書き
curl -X POST http://localhost:8080/v1/etc/scrape/batch/env \
  -H "Content-Type: application/json" \
  -d '{"download_path": "/custom/path"}'
```

**バッチ処理の特徴：**
- 複数アカウントを1ジョブで順次処理
- セッションフォルダ（YYYYMMDD_HHMMSS形式）に全CSVを保存
- アカウントごとの進捗・ステータスを個別追跡
- 1つでも失敗→ジョブ全体はFailed（ただし成功分のCSVは保存済み）
- **自動クリーンアップ**: 古いセッションフォルダを自動削除（最新10個を保持）

## Architecture

- **axum**: HTTP framework
- **chromiumoxide**: Browser automation
- **sqlx**: Async SQLite
- **tokio**: Async runtime
- **tonic** (optional): gRPC (`--features grpc`)
- **scraper-service**: ETC明細スクレイパー（workspace member）

## Docker & Deploy

### 自動デプロイ
`git push`で自動的にビルド・デプロイが実行される（pre-pushフック）。

```bash
git push  # → build → push to GHCR → deploy to GCE
```

### 手動デプロイ
```bash
./scripts/deploy.sh
```

### GCE初期セットアップ（1回のみ）
```bash
# 1. セットアップスクリプトをコピー・実行
gcloud compute scp scripts/gce-setup.sh instance-20251207-115015:~ --zone=asia-northeast1-b
gcloud compute ssh instance-20251207-115015 --zone=asia-northeast1-b --command="bash ~/gce-setup.sh"

# 2. ローカルの.envをGCEにコピー（GHCR_TOKEN含む）
gcloud compute scp .env instance-20251207-115015:/tmp/.env --zone=asia-northeast1-b
gcloud compute ssh instance-20251207-115015 --zone=asia-northeast1-b --command="sudo mv /tmp/.env /opt/browser-render/.env"
```

### Docker設定
- `--network host`: ポート公開なし、localhost:8080でアクセス可能
- `--shm-size=2g`: Chromium用共有メモリ
- chromedp/headless-shellベースイメージ使用

### Cron設定（GCE上）

Vehicleデータ取得（10分おき）:
```bash
# /etc/cron.d/vehicle-fetch
*/10 * * * * root curl -sf http://localhost:8080/v1/vehicle/data >> /opt/browser-render/logs/vehicle-cron.log 2>&1
```

ETC明細スクレイプ（任意の時間に設定可能）:
```bash
# /etc/cron.d/etc-scrape
# 毎日6時にキューに追加（idle時に自動実行）
0 6 * * * root curl -sf -X POST -H "Content-Type: application/json" \
  -d '{"user_id":"xxx","password":"xxx"}' \
  http://localhost:8080/v1/etc/scrape/queue >> /opt/browser-render/logs/etc-cron.log 2>&1

# 即時実行したい場合
# 0 6 * * * root curl -sf -X POST -H "Content-Type: application/json" \
#   -d '{"user_id":"xxx","password":"xxx"}' \
#   http://localhost:8080/v1/etc/scrape >> /opt/browser-render/logs/etc-cron.log 2>&1
```

### ヘルスチェック
```bash
gcloud compute ssh instance-20251207-115015 --zone=asia-northeast1-b --command="curl -sf http://localhost:8080/health"
```

## TODO

- [x] 実環境でのテスト
- [x] エラーハンドリングの改善
- [x] ログ出力の最適化
- [x] Docker + GCE自動デプロイ
- [x] ETC明細スクレイパー統合
- [x] ETC複数アカウントバッチ処理
- [ ] メトリクス追加
- [x] 動画通知機能（Monitoring_DvrNotification2）の修正

---

## 動画通知機能 - 解決済み (2026-01-25)

### 問題の原因
`Monitoring_DvrNotification2` の呼び出し引数が間違っていた。

**間違い:**
```javascript
VenusBridgeService.Monitoring_DvrNotification2(callback);  // 引数1つ
```

**正しい形式:**
```javascript
// sort引数形式: "fieldName,dir,pageIndex,pageSize"
const sort = ",," + "0" + "," + "100";
VenusBridgeService.Monitoring_DvrNotification2(sort, callback);  // 引数2つ
```

### 修正内容
- [rust-scraper/src/dtakolog/scraper.rs](rust-scraper/src/dtakolog/scraper.rs) L913-917
- sort引数を追加: `const sort = ",," + "0" + "," + "100";`

### テスト結果
- 修正前: 60秒タイムアウト、コールバック発火せず
- 修正後: 500msで結果受信、正常動作

### テスト用コード
小さなテストコードを作成済み:
```bash
# 素早いテスト実行
cargo run -p scraper-service --example dvr_test
```
- [rust-scraper/examples/dvr_test.rs](rust-scraper/examples/dvr_test.rs)
