# Mirror & Proxy Cache

orb-chrysa can mirror upstream registries and act as a pull-through proxy cache.

## Mirror Rules

Mirror rules define a relationship between a local repository prefix and an upstream
registry. orb-chrysa periodically syncs images matching the configured strategy.

### Strategies

| Strategy | Description |
|----------|-------------|
| `all` | Mirror all tags |
| `latest { count }` | Mirror the `count` most recent tags |
| `pattern { pattern }` | Mirror tags matching a glob pattern |

### Directions

| Direction | Description |
|------------|-------------|
| `pull` | Pull images from upstream to local |
| `push` | Push local images to upstream |

### Example

```toml
# Managed via admin API: PUT /api/v1/admin/mirror/rules/my-rule
id = "my-rule"
direction = "pull"
local_prefix = "mirror/library"
upstream_registry = "docker.io"
upstream_prefix = "library"
strategy = { type = "latest", count = 5 }
```

## Proxy Cache

Proxy caches act as pull-through caches. When a client pulls an image from the local
registry, orb-chrysa checks if it exists locally. If not, it fetches from the upstream
registry, caches it, and returns it to the client.

```toml
# Managed via admin API: PUT /api/v1/admin/proxy-cache/my-cache
id = "my-cache"
local_prefix = "cache"
upstream_registry = "docker.io"
warm_filters = [{ type = "all" }]
```

## Warm-Up

Proxy caches support warm-up — pre-fetching images on a schedule before clients request
them. Configured via the `warm_schedule` and `warm_filters` fields.

## Outbound Proxy

Both mirror rules and proxy caches support an optional outbound proxy for reaching
upstream registries through HTTP, SOCKS4, or SOCKS5 proxies.
