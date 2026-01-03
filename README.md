# Browser Render Rust

Rust implementation of browser automation service for vehicle data extraction.

## Features

- **HTTP API**: RESTful endpoints for job management and data retrieval
- **gRPC API**: Optional Protocol Buffers-based API (requires `protoc`)
- **Browser Automation**: Headless Chrome/Chromium via chromiumoxide
- **SQLite Storage**: Session, cookie, and cache management
- **Job Queue**: Asynchronous background job processing

## Project Structure

```
src/
├── main.rs           # Entry point, CLI, server startup
├── config.rs         # Environment-based configuration
├── browser/
│   └── renderer.rs   # Browser automation (login, data extraction)
├── jobs/
│   └── manager.rs    # Async job queue management
├── server/
│   ├── http.rs       # Axum HTTP server
│   └── grpc.rs       # Tonic gRPC server (optional)
└── storage/
    └── sqlite.rs     # SQLite database operations
```

## Requirements

- Rust 1.70+
- Chrome/Chromium browser
- SQLite
- (Optional) protoc for gRPC support

## Build

```bash
# HTTP only (default)
cargo build --release

# With gRPC support (requires protoc)
cargo build --release --features grpc
```

## Configuration

Environment variables (or `.env` file):

| Variable | Default | Description |
|----------|---------|-------------|
| `HTTP_PORT` | 8080 | HTTP server port |
| `GRPC_PORT` | 50051 | gRPC server port |
| `USER_NAME` | - | Login username |
| `COMP_ID` | - | Company ID |
| `USER_PASS` | - | Login password |
| `BROWSER_HEADLESS` | true | Run browser headless |
| `BROWSER_DEBUG` | false | Enable debug logging |
| `SQLITE_PATH` | ./data/browser_render.db | Database path |
| `SESSION_TTL` | 10m | Session timeout |
| `COOKIE_TTL` | 24h | Cookie expiration |

## Usage

```bash
# Start HTTP server (default)
./browser-render

# With custom port
./browser-render --http-port 3000

# HTTP only
./browser-render --server http

# Both HTTP and gRPC (requires grpc feature)
./browser-render --server both
```

## API Endpoints

### HTTP

| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/vehicle/data` | Create data fetch job |
| GET | `/v1/job/{id}` | Get job status |
| GET | `/v1/jobs` | List all jobs |
| GET | `/v1/session/check?session_id=X` | Check session validity |
| DELETE | `/v1/session/clear?session_id=X` | Clear session |
| GET | `/health` | Health check |
| GET | `/metrics` | Server metrics |

### Example

```bash
# Create a job
curl http://localhost:8080/v1/vehicle/data
# Response: {"job_id":"uuid","status":"pending","message":"..."}

# Check job status
curl http://localhost:8080/v1/job/{job_id}

# Health check
curl http://localhost:8080/health
```

## License

MIT
