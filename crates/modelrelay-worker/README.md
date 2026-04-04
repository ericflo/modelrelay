# worker-daemon

Worker daemon that connects to the proxy server and forwards LLM inference requests to a local backend (llama-server, Ollama, vLLM, etc.). Part of the [ModelRelay](https://github.com/ericflo/modelrelay) system.

## Install

```bash
cargo install worker-daemon
```

Or download pre-built binaries from [GitHub Releases](https://github.com/ericflo/modelrelay/releases).

## Usage

```bash
worker-daemon \
  --proxy-url http://<proxy-host>:8080 \
  --worker-secret mysecret \
  --backend-url http://127.0.0.1:8000 \
  --models llama3.2:3b
```

The worker connects out to the proxy over WebSocket, so no inbound ports are needed on the GPU box.

See the [main repository](https://github.com/ericflo/modelrelay) for full documentation, configuration options, and quickstart guides.

## License

MIT
