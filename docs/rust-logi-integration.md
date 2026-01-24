# browser-render-rust → rust-logi gRPC 統合

## 概要

browser-render-rust から rust-logi の PostgreSQL に直接 gRPC でデータを送信する機能を実装。

## 背景

- **問題**: Hono API (Cloudflare Workers) の `forEach + async` バグで D1 へのデータ保存が失敗
- **解決策**: rust-logi の BulkCreate RPC を使用して PostgreSQL に直接挿入

## アーキテクチャ

```
変更前:
browser-render-rust → POST → hono-api (CF Workers) → D1 (失敗)

変更後:
browser-render-rust → gRPC → rust-logi → PostgreSQL (Cloud SQL)
```

## 実装完了項目

### rust-logi 側
- [x] `packages/logi-proto/proto/dtakologs.proto` に BulkCreate RPC 追加
- [x] `src/services/dtakologs_service.rs` に bulk_create 実装

### browser-render-rust 側
- [x] `build.rs` に logi proto コンパイル追加
- [x] `src/lib.rs` に logi モジュール追加
- [x] `src/main.rs` に logi モジュール追加（binary crate 用）
- [x] `src/config.rs` に `rust_logi_url`, `rust_logi_organization_id` 追加
- [x] `src/browser/renderer.rs` に `send_to_rust_logi` メソッド追加
- [x] `src/browser/renderer.rs` の呼び出し元を `send_to_rust_logi` に変更
- [x] 非推奨の `send_raw_to_hono_api` を削除

## 必須環境変数

```env
# rust-logi の URL（必須）
RUST_LOGI_URL=http://localhost:50051

# 組織ID（必須）
RUST_LOGI_ORGANIZATION_ID=00000000-0000-0000-0000-000000000001
```

## ビルド方法

```bash
# grpc feature を有効にしてビルド（必須）
cargo build --features grpc

# リリースビルド
cargo build --release --features grpc
```

## ローカルテスト手順

### 1. rust-logi 起動（PC1 または同一PC）

```bash
cd rust-logi
./start-proxy.sh  # Cloud SQL Proxy
./start.sh        # rust-logi サーバー
```

### 2. browser-render-rust 起動（PC2 または同一PC）

```bash
cd browser-render-rust

# 環境変数設定
export RUST_LOGI_URL=http://<rust-logi-host>:50051
export RUST_LOGI_ORGANIZATION_ID=00000000-0000-0000-0000-000000000001

# 起動
cargo run --features grpc
```

### 3. データ取得トリガー

```bash
curl http://localhost:8080/v1/vehicle/data
```

### 4. PostgreSQL 確認

```bash
PGPASSWORD=kikuraku psql -h 127.0.0.1 -p 5432 -U postgres -d rust_logi_test \
  -c "SELECT COUNT(*), MAX(data_date_time) FROM dtakologs;"
```

## デプロイ

### rust-logi (Cloud Run)
```bash
cd rust-logi
# Cloud Run にデプロイ（既存の CI/CD を使用）
```

### browser-render-rust (GCE)
```bash
# GCE インスタンスで以下を設定
# /etc/systemd/system/browser-render.service の環境変数に追加:
# RUST_LOGI_URL=https://rust-logi-XXXXX.run.app
# RUST_LOGI_ORGANIZATION_ID=<organization-uuid>
```

## 注意事項

- `--features grpc` なしでビルドすると、データは送信されません（警告ログのみ）
- browser-render-rust は Chrome の安定性のため GCE で運用を継続
- rust-logi の URL と組織ID は必須（未設定時は警告ログ）

## 関連ファイル

| ファイル | 説明 |
|---------|------|
| `build.rs` | proto コンパイル設定 |
| `src/lib.rs` | logi モジュール定義（lib） |
| `src/main.rs` | logi モジュール定義（bin） |
| `src/config.rs` | rust-logi 設定 |
| `src/browser/renderer.rs` | gRPC 送信実装 |
| `proto/logi/dtakologs.proto` | BulkCreate RPC 定義 |
| `proto/logi/common.proto` | 共通メッセージ定義 |
