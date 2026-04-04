# proxy-server

Central HTTP proxy server that routes LLM inference requests to remote workers over WebSocket. Part of the [llm-worker-proxy](https://github.com/ericflo/llm-worker-proxy) system.

## Install

```bash
cargo install proxy-server
```

Or download pre-built binaries from [GitHub Releases](https://github.com/ericflo/llm-worker-proxy/releases).

## Usage

```bash
proxy-server --listen 0.0.0.0:8080 --worker-secret mysecret
```

The proxy accepts standard OpenAI and Anthropic API requests and routes them to connected workers.

See the [main repository](https://github.com/ericflo/llm-worker-proxy) for full documentation, configuration options, and quickstart guides.

## License

MIT
