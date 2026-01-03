# CLAUDE.md

## Project Overview

Browser Render Rust - GoプロジェクトからのRust移植版。ブラウザ自動化によるVehicleデータ取得とHono API連携サービス。

## Current Status

- **Phase**: 初期実装完了
- **Build Status**: ✅ コンパイル成功
- **HTTP Server**: ✅ 実装完了
- **gRPC Server**: ✅ 実装完了 (オプショナル、protocが必要)
- **Browser Automation**: ✅ 実装完了
- **Storage**: ✅ 実装完了
- **Job Queue**: ✅ 実装完了

## Reference Project

元のGoプロジェクトは `browser_render_go_ref/` にクローン済み（.gitignoreで除外）。

## Build Commands

```bash
# 開発ビルド
cargo build

# リリースビルド
cargo build --release

# gRPC機能付きビルド
cargo build --features grpc
```

## Run Commands

```bash
# HTTPサーバー起動
cargo run -- --server http

# カスタムポート
cargo run -- --http-port 3000

# デバッグモード
cargo run -- --debug
```

## Key Dependencies

- **axum**: HTTP framework
- **chromiumoxide**: Browser automation
- **sqlx**: Async SQLite
- **tokio**: Async runtime
- **tonic** (optional): gRPC

## Architecture Notes

- gRPCはオプショナル機能 (`--features grpc`)
- protocが未インストールの場合、HTTPのみで動作
- ブラウザはheadlessモードがデフォルト

## TODO

- [x] 実環境でのテスト（統合テスト完了）
- [x] エラーハンドリングの改善（2026-01-03完了）
- [x] ログ出力の最適化（2026-01-03完了）
- [ ] メトリクス追加

## ログ設定

### CLI引数

| 引数 | デフォルト | 説明 |
|------|------------|------|
| `--log-format` | `text` | 出力形式: `text` または `json` |
| `--log-file` | (なし) | ファイル名（指定でファイル出力有効） |
| `--log-dir` | `./logs` | ログディレクトリ |
| `--log-rotation` | `daily` | ローテーション: `daily`, `hourly`, `never` |

### 環境変数

| 変数 | デフォルト | 説明 |
|------|------------|------|
| `RUST_LOG` | (なし) | モジュール別ログレベル制御 |
| `LOG_FORMAT` | `text` | 出力形式 |
| `LOG_FILE` | (なし) | ファイル名 |
| `LOG_DIR` | `./logs` | ディレクトリ |
| `LOG_ROTATION` | `daily` | ローテーション |

### 使用例

```bash
# JSON形式でファイル出力（本番環境推奨）
cargo run -- --log-format json --log-file app.log

# モジュール別ログレベル制御
RUST_LOG=browser_render::browser=debug,info cargo run

# 本番設定
cargo run -- --log-format json --log-file app.log --log-dir /var/log/browser-render
```

## 統合テスト

### 実装済み

`tests/browser_integration_test.rs` に統合テストを実装済み。

#### テスト内容
- **MockHonoServer**: axumベースのモックサーバー（ポート自動割当）
- **test_vehicle_data_extraction_and_post**: ブラウザ→ログイン→データ取得→モックサーバーへPOST
- **test_session_persistence**: セッション永続化テスト
- **test_mock_server_standalone**: モックサーバー単体テスト

#### テスト実行
```bash
# 統合テスト実行（認証情報は.envから読み込み、順次実行が必須）
cargo test --test browser_integration_test -- --ignored --nocapture --test-threads=1

# モックサーバー単体テスト（認証不要）
cargo test --test browser_integration_test test_mock_server_standalone -- --nocapture
```

**重要**: ブラウザテストは `--test-threads=1` で順次実行する必要があります。並列実行すると競合が発生します。

#### 設定可能なAPI URL
`HONO_API_URL`環境変数でHono APIのURLを変更可能（デフォルト: `https://hono-api.mtamaramu.com/api/dtakologs`）

### 修正履歴

#### 2026-01-03: 複数のバグ修正

1. **JavaScript実行エラー修正** (`src/browser/renderer.rs:447-452`)
   - chromiumoxideの`evaluate()`はトップレベルコードとして実行されるため、`return`文が使えない
   - 修正: 即時実行関数（IIFE）でラップ `(() => { ... })()`

2. **Chrome競合問題修正** (`src/browser/renderer.rs:76-87`)
   - 複数ブラウザインスタンスが同じ`user_data_dir`を使おうとして競合
   - 修正: プロセスID+ナノ秒タイムスタンプでユニークなディレクトリを生成

3. **headlessモードのバグ修正** (`src/browser/renderer.rs:84-86`)
   - 修正前: `browser_headless=true` のとき `with_head()` を呼び出し（GUIモード）
   - 修正後: `browser_headless=false` のときのみ `with_head()` を呼び出し

4. **テストアサーション修正** (`tests/browser_integration_test.rs:280-286`)
   - 実際のAPIデータには`Status`フィールドがなく、`State`/`AllState`フィールドを使用
   - テストのアサーションを実際のデータ構造に合わせて修正

#### 2026-01-03: エラーハンドリングの改善

1. **RendererError型の拡張** (`src/browser/renderer.rs:22-100`)
   - コンテキスト情報を含む構造体型エラーに変更（Browser, NavigationFailed, ApiError）
   - `http_status_code()`: エラーからHTTPステータスコードへのマッピング
   - `is_retryable()`: リトライ可能かどうかの判定メソッド
   - ヘルパーメソッド追加: `browser()`, `navigation()`, `api()`

2. **パニックリスクの修正** (`src/server/http.rs:126`)
   - `serde_json::to_value(job).unwrap()` を `Json(job)` に簡素化

3. **エラーコンテキストの改善** (`src/browser/renderer.rs`)
   - 全ての`map_err`にコンテキスト情報を追加（例: "check login form", "inject vehicle data script"）
   - NavigationFailedにURL情報を含める
   - ApiErrorにHTTPステータスコードを含める

4. **HTTPステータスコードの適切なマッピング** (`src/server/http.rs`)
   - `RendererError::http_status_code()`を使用して適切なステータスコードを返却
   - 401: LoginFailed, SessionError
   - 400: NavigationFailed
   - 500: Browser, ExtractionFailed, Storage, JsonError
   - 502: HttpRequest（upstream error）

5. **無視されていたエラーのログ出力追加**
   - Cookie設定エラー: `debug!`レベルでログ
   - キャッシュエラー: `debug!`レベルでログ
   - ページクローズエラー: `debug!`レベルでログ
   - ポップアップ処理エラー: `debug!`レベルでログ
   - データディレクトリ作成エラー: `warn!`レベルでログ
