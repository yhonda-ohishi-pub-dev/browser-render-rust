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

## Architecture

- **axum**: HTTP framework
- **chromiumoxide**: Browser automation
- **sqlx**: Async SQLite
- **tokio**: Async runtime
- **tonic** (optional): gRPC (`--features grpc`)

## Docker & Deploy

```bash
# ローカルビルド
docker build -t ghcr.io/yhonda-ohishi-pub-dev/browser-render-rust:test .

# GHCRにpush
docker push ghcr.io/yhonda-ohishi-pub-dev/browser-render-rust:test

# デプロイスクリプト実行（build → push → GCEデプロイ）
./scripts/deploy.sh
```

### GCE初期セットアップ（1回のみ）
```bash
gcloud compute scp scripts/gce-setup.sh instance-20251207-115015:~ --zone=asia-northeast1-b
gcloud compute ssh instance-20251207-115015 --zone=asia-northeast1-b --command="bash ~/gce-setup.sh"
# /opt/browser-render/.env を編集して認証情報設定
```

### Pre-pushフック
`git push`時に自動デプロイ（`.githooks/pre-push`）
```bash
git config core.hooksPath .githooks  # 有効化済み
```

## TODO

- [x] 実環境でのテスト
- [x] エラーハンドリングの改善
- [x] ログ出力の最適化
- [x] Docker + GCE自動デプロイ
- [ ] メトリクス追加
