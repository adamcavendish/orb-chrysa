# Storage Configuration

```toml
[storage.s3]
endpoint = "http://rustfs:9000"
bucket = "orb-chrysa"
region = "us-east-1"
access_key = "rustfsadmin"
secret_key = "rustfsadmin"
path_style = true

[storage.s3.redirect]
enabled = false
public_endpoint = "http://localhost:9000"
expires_secs = 900
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `endpoint` | string | (required) | S3-compatible endpoint URL |
| `bucket` | string | (required) | S3 bucket name |
| `region` | string | `"us-east-1"` | AWS region |
| `access_key` | string | (required) | S3 access key |
| `secret_key` | string | (required) | S3 secret key |
| `path_style` | bool | `false` | Use path-style addressing |
| `redirect.enabled` | bool | `false` | Enable S3 redirect mode |
| `redirect.public_endpoint` | string | `""` | Public S3 endpoint for redirect |
| `redirect.expires_secs` | integer | 900 | Pre-signed URL expiry in seconds |
