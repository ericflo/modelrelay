# worker-protocol

WebSocket protocol types shared between the proxy server and worker daemon in the [llm-worker-proxy](https://github.com/ericflo/llm-worker-proxy) system.

This crate defines the message types used for communication between the central proxy and remote workers over WebSocket connections. It is a dependency of both `proxy-server` and `worker-daemon`.

## Usage

```toml
[dependencies]
worker-protocol = "0.1"
```

```rust
use worker_protocol::{ServerMessage, WorkerMessage};
```

See the [main repository](https://github.com/ericflo/llm-worker-proxy) for full documentation and usage examples.

## License

MIT
