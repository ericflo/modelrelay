# TLS Setup

ModelRelay's proxy server listens on plain HTTP by default.  For
production deployments you should terminate TLS in front of it so that:

- **Clients** reach the API over HTTPS (`https://your-domain/v1/...`)
- **Workers** connect over secure WebSockets (`wss://your-domain/v1/worker/connect`)

Without TLS the worker secret and all inference traffic travel in the
clear.  This matters especially when workers connect over the public
internet rather than a private network.

---

## Option 1: nginx (recommended)

The repository includes a ready-to-use nginx config at
[`examples/tls-nginx.conf`](https://github.com/ericflo/modelrelay/blob/main/examples/tls-nginx.conf).
Copy it into your nginx sites directory and customise the domain and
certificate paths.

### How it works

The config defines two `server` blocks:

1. **Port 80** redirects all HTTP traffic to HTTPS.
2. **Port 443** terminates TLS and proxies to `127.0.0.1:8080` (the
   default `LISTEN_ADDR`).

Two `location` blocks handle the different traffic types:

- **`/v1/worker/connect`** --- the WebSocket endpoint.  The key
  directives are:
  ```nginx
  proxy_http_version 1.1;
  proxy_set_header Upgrade $http_upgrade;
  proxy_set_header Connection "upgrade";
  proxy_read_timeout 86400s;   # keep the long-lived WS open
  proxy_send_timeout 86400s;
  ```
  Without the `Upgrade` / `Connection` headers, nginx will not complete
  the WebSocket handshake and workers will fail to connect.

- **`/v1/`** --- the inference API.  Buffering is disabled so that
  SSE streaming responses pass through without delay:
  ```nginx
  proxy_buffering off;
  proxy_cache off;
  proxy_read_timeout 300s;   # match REQUEST_TIMEOUT_SECS
  ```

### Quick start

```bash
# 1. Obtain a certificate (Let's Encrypt example)
sudo certbot certonly --nginx -d your-domain.example.com

# 2. Install the config
sudo cp examples/tls-nginx.conf /etc/nginx/sites-available/modelrelay.conf
sudo ln -s /etc/nginx/sites-available/modelrelay.conf /etc/nginx/sites-enabled/

# 3. Edit the config: replace your-domain.example.com everywhere
sudo nano /etc/nginx/sites-available/modelrelay.conf

# 4. Test and reload
sudo nginx -t && sudo systemctl reload nginx
```

### Certificate renewal

Let's Encrypt certificates expire after 90 days.  Certbot usually
installs a systemd timer or cron job that renews automatically.  Verify:

```bash
sudo certbot renew --dry-run
```

---

## Option 2: Caddy

[Caddy](https://caddyserver.com/) automatically provisions and renews
TLS certificates from Let's Encrypt with zero configuration.  If you
don't need nginx's flexibility, this is the simplest path.

### Caddyfile

```caddyfile
your-domain.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

That's it.  Caddy handles:

- HTTPS redirect from port 80
- Automatic Let's Encrypt certificate issuance and renewal
- WebSocket upgrade pass-through (no special config needed)
- Unbuffered streaming (the default for `reverse_proxy`)

### Running

```bash
# Install (Debian/Ubuntu)
sudo apt install -y caddy

# Write the Caddyfile
cat > /etc/caddy/Caddyfile <<'EOF'
your-domain.example.com {
    reverse_proxy 127.0.0.1:8080
}
EOF

# Start
sudo systemctl enable --now caddy
```

> **Note:** Caddy must be able to bind ports 80 and 443, and the domain
> must resolve to the server's public IP for the ACME challenge to
> succeed.

---

## Option 3: Manual certificates (certbot standalone)

If you're running neither nginx nor Caddy you can still use Let's
Encrypt with certbot's standalone mode, then point any reverse proxy at
the resulting certificate files:

```bash
sudo certbot certonly --standalone -d your-domain.example.com
```

Certificates land in `/etc/letsencrypt/live/your-domain.example.com/`.
Use `fullchain.pem` and `privkey.pem` with whatever TLS terminator you
prefer (HAProxy, Traefik, etc.).

---

## Configuring workers for TLS

Once TLS is in place, update the worker's `PROXY_URL` to use the secure
scheme:

| Scenario | `PROXY_URL` |
|----------|-------------|
| No TLS (local / private network) | `http://proxy:8080` |
| TLS via reverse proxy | `https://your-domain.example.com` |

The worker uses `PROXY_URL` to derive the WebSocket connection URL.
When the scheme is `https`, the worker connects over `wss://`
automatically.

```bash
# Example: worker connecting over TLS
PROXY_URL=https://your-domain.example.com \
WORKER_SECRET=your-secret \
BACKEND_URL=http://localhost:8000 \
  modelrelay-worker --models llama3-8b
```

> **Tip:** The local backend (`BACKEND_URL`) almost never needs TLS ---
> it runs on the same machine as the worker.  Keep it as plain
> `http://localhost:...`.

---

## Troubleshooting

### Workers can't connect after enabling TLS

1. Verify the certificate is valid: `curl -v https://your-domain.example.com/v1/models`
2. Confirm WebSocket upgrade works: `curl -v -H 'Upgrade: websocket' -H 'Connection: upgrade' https://your-domain.example.com/v1/worker/connect`  (should get a 101 or 400, not a connection error)
3. Check that `proxy_read_timeout` / `proxy_send_timeout` are long enough for the WebSocket (the nginx config uses 86400s)

### Streaming responses arrive buffered

Ensure your reverse proxy has buffering disabled for the `/v1/` path.
In nginx: `proxy_buffering off;`.  Caddy disables buffering by default.

### Certificate renewal fails

Certbot's HTTP-01 challenge needs port 80.  If nginx or Caddy is
running, use the `--nginx` or `--caddy` certbot plugin instead of
`--standalone` to avoid port conflicts.
