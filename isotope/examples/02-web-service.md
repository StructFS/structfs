# Example: Web Service

A realistic web service with authentication, caching, and a database.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    user-service (Assembly)                   │
│                                                             │
│  ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐  │
│  │ Gateway │───▶│  Auth   │───▶│   API   │───▶│  Cache  │  │
│  │ (public)│    │         │    │         │    │         │  │
│  └─────────┘    └─────────┘    └────┬────┘    └─────────┘  │
│                                     │                       │
│                                     ▼                       │
│                               ┌─────────┐                   │
│                               │   DB    │                   │
│                               │         │                   │
│                               └─────────┘                   │
└─────────────────────────────────────────────────────────────┘
```

## Assembly Definition

```yaml
assembly: user-service
version: 1.0.0

blocks:
  gateway: ./gateway-block.wasm
  auth: ./auth-block.wasm
  api: ./api-block.wasm
  cache: ./cache-block.wasm
  db: ./db-block.wasm

public: gateway

imports:
  logger: "Logging service from parent"

wiring:
  # Gateway routes to auth and api
  gateway:/services/auth -> auth
  gateway:/services/api -> api

  # API uses cache and db
  api:/services/cache -> cache
  api:/services/db -> db

  # Cache can fall through to db
  cache:/services/db -> db

  # Everyone can log
  gateway:/services/logger -> $logger
  auth:/services/logger -> $logger
  api:/services/logger -> $logger

config:
  gateway:
    port: 8080
  cache:
    ttl_seconds: 300
  db:
    pool_size: 10

failure:
  cache: isolate  # Cache failure is non-fatal
```

## The Gateway Block

Routes external requests, handles HTTP translation:

```python
write("/iso/self/interface", {
    "paths": {
        "/health": {"read": "Health check"},
        "/users/{id}": {"read": "Get user", "write": "Update user"},
        "/users": {"write": "Create user"}
    }
})

while True:
    req = read("/iso/server/requests")
    if req is None:
        break

    # Authenticate first
    auth_result = read(f"/services/auth/verify?token={extract_token(req)}")
    if auth_result.get("error"):
        write(req.respond_to, {"result": "error", "error": auth_result["error"]})
        continue

    # Route to API
    if req.path == "health":
        write(req.respond_to, {"result": "ok", "value": {"status": "healthy"}})
    elif req.path.startswith("users"):
        # Forward to API
        if req.op == "read":
            result = read(f"/services/api/{req.path}")
        else:
            result = write(f"/services/api/{req.path}", req.data)
        write(req.respond_to, {"result": "ok", "value": result})

write("/iso/shutdown/complete", {})
```

## The Auth Block

Verifies tokens:

```python
write("/iso/self/interface", {
    "paths": {
        "/verify": {"read": "Verify a token (pass token as query param)"}
    }
})

while True:
    req = read("/iso/server/requests")
    if req is None:
        break

    if req.op == "read" and req.path.startswith("verify"):
        token = extract_query_param(req.path, "token")
        if is_valid_token(token):
            user_id = decode_token(token)
            write(req.respond_to, {"result": "ok", "value": {"user_id": user_id}})
        else:
            write(req.respond_to, {
                "result": "error",
                "error": {"type": "unauthorized", "message": "Invalid token"}
            })

write("/iso/shutdown/complete", {})
```

## The API Block

Business logic with cache-aside pattern:

```python
write("/iso/self/interface", {
    "paths": {
        "/users/{id}": {"read": "Get user by ID", "write": "Update user"},
        "/users": {"write": "Create new user"}
    }
})

while True:
    req = read("/iso/server/requests")
    if req is None:
        break

    if req.op == "read" and req.path.startswith("users/"):
        user_id = req.path.split("/")[1]

        # Try cache first
        cached = read(f"/services/cache/users/{user_id}")
        if cached:
            write(req.respond_to, {"result": "ok", "value": cached})
            continue

        # Cache miss - hit database
        user = read(f"/services/db/users/{user_id}")
        if user:
            # Populate cache
            write(f"/services/cache/users/{user_id}", user)
            write(req.respond_to, {"result": "ok", "value": user})
        else:
            write(req.respond_to, {
                "result": "error",
                "error": {"type": "not_found", "message": f"User {user_id} not found"}
            })

    elif req.op == "write" and req.path.startswith("users/"):
        user_id = req.path.split("/")[1]

        # Update database
        write(f"/services/db/users/{user_id}", req.data)

        # Invalidate cache
        write(f"/services/cache/invalidate/users/{user_id}", {})

        write(req.respond_to, {"result": "ok", "path": f"users/{user_id}"})

    elif req.op == "write" and req.path == "users":
        # Create new user
        result_path = write("/services/db/users", req.data)
        write(req.respond_to, {"result": "ok", "path": result_path})

write("/iso/shutdown/complete", {})
```

## Testing

### Unit Test: API Block with Mocks

```yaml
assembly: api-test
version: 1.0.0

blocks:
  api: ./api-block.wasm
  mock_cache: ./mock-cache.wasm
  mock_db: ./mock-db.wasm
  test_runner: ./test-runner.wasm

public: test_runner

wiring:
  test_runner:/services/api -> api
  api:/services/cache -> mock_cache
  api:/services/db -> mock_db
  test_runner:/mocks/cache -> mock_cache
  test_runner:/mocks/db -> mock_db
```

The test runner can:
1. Set up mock data via `/mocks/db`
2. Call API via `/services/api`
3. Verify cache was populated via `/mocks/cache`

### Integration Test: Full Assembly

```yaml
assembly: user-service-integration-test
version: 1.0.0

blocks:
  service: ./user-service.assembly.yaml  # The whole Assembly as a Block!
  test_runner: ./test-runner.wasm

public: test_runner

imports:
  logger: test-doubles:logger

wiring:
  test_runner:/services/user -> service
```

The test runner tests the complete Assembly as a black box.

## Deployment Variants

### Development

```yaml
assembly: user-service-dev
extends: user-service

config:
  gateway:
    port: 3000
  auth:
    bypass: true  # Skip auth in dev
  db:
    connection: "postgres://localhost/dev"
```

### Production

```yaml
assembly: user-service-prod
extends: user-service

config:
  gateway:
    port: 8080
    tls:
      cert: /secrets/cert.pem
      key: /secrets/key.pem
  db:
    connection: ${DATABASE_URL}
    pool_size: 50
  cache:
    ttl_seconds: 600

failure:
  cache: restart
  db: fail-fast
```

## Key Points

1. **Separation of concerns**: Gateway handles routing, auth handles tokens,
   API handles business logic, cache and DB handle storage. Each is a simple
   Block.

2. **Wiring defines architecture**: The Assembly YAML IS the architecture
   diagram. No separate documentation needed.

3. **Testing at any level**: Test individual Blocks with mocks, or test the
   entire Assembly as a unit.

4. **Environment via config**: Dev, staging, prod differ only in configuration.
   Same Blocks, same wiring, different settings.

5. **Failure isolation**: Cache failure doesn't kill the service. Database
   failure does. This is explicit in the Assembly.

6. **Observable by convention**: All Blocks can log via `/services/logger`.
   The parent Assembly provides the logger implementation.
