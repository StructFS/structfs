# Example: Web Service

A realistic web service with authentication, caching, and a database.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    user-service (Assembly)                   │
│                                                             │
│  ┌─────────┐    ┌─────────┐    ┌─────────┐    ┌─────────┐  │
│  │  HTTP   │───▶│  Auth   │───▶│  API    │───▶│  Cache  │  │
│  │ Gateway │    │ Block   │    │ Block   │    │ Block   │  │
│  └─────────┘    └─────────┘    └────┬────┘    └─────────┘  │
│                                     │                       │
│                                     ▼                       │
│                               ┌─────────┐                   │
│                               │   DB    │                   │
│                               │ Block   │                   │
│                               └─────────┘                   │
└─────────────────────────────────────────────────────────────┘
```

## Assembly Definition

```yaml
assembly: user-service
version: 1.0.0

blocks:
  gateway: stdlib:http-gateway
  auth: ./auth-block
  api: ./api-block
  cache: stdlib:redis-cache
  db: stdlib:postgres

config:
  gateway:
    port: 8080
    routes:
      POST /users: api.create_user
      GET /users/{id}: api.get_user
      PUT /users/{id}: api.update_user
      DELETE /users/{id}: api.delete_user

  cache:
    ttl: 300  # 5 minutes

  db:
    connection_string: ${DATABASE_URL}

wiring:
  # Gateway sends authenticated requests to API
  gateway.requests -> auth:/input
  auth:/output -> api:/input
  api:/output -> gateway.responses

  # API uses cache, falls through to DB
  api.cache -> cache:/
  api.db -> db:/queries

  # Auth verifies tokens
  auth.tokens -> /ctx/iso/crypto/jwt/verify

exports:
  http: gateway.http
  health: api.health
  metrics: /ctx/iso/metrics
```

## The Auth Block

```
Block: auth

On request at /input:
  1. Read /input/request
  2. Extract Authorization header
  3. If no header:
     Write to /output: 401 Unauthorized
     Return
  4. Read /tokens/verify with token
  5. If invalid:
     Write to /output: 403 Forbidden
     Return
  6. Add user_id to request context
  7. Write augmented request to /output
```

The Auth Block:
- Receives requests from gateway
- Validates JWT tokens via `/tokens/verify` (wired to system crypto)
- Forwards authenticated requests or returns errors

## The API Block

```
Block: api

Exports:
  - health: Health check store
  - create_user, get_user, update_user, delete_user: Request handlers

On GET /input (get_user):
  1. Extract user_id from request
  2. Read /cache/{user_id}
  3. If cache hit: return cached user
  4. Read /db/users/{user_id}
  5. If not found: return 404
  6. Write to /cache/{user_id} for future requests
  7. Return user

On POST /input (create_user):
  1. Validate request body
  2. Write to /db/users (returns new ID)
  3. Read back created user
  4. Return 201 with user

On PUT /input (update_user):
  1. Validate request body
  2. Write to /db/users/{user_id}
  3. Delete /cache/{user_id} (invalidate)
  4. Return updated user

On DELETE /input (delete_user):
  1. Delete /db/users/{user_id}
  2. Delete /cache/{user_id}
  3. Return 204
```

The API Block:
- Doesn't know about HTTP directly (gateway handles that)
- Doesn't know about authentication (auth handles that)
- Just implements CRUD operations using paths

## Testing

### Unit Test: API Block

```yaml
test: api-get-user-cached
blocks:
  api: ./api-block
mounts:
  # Mock cache with a user
  api:/cache: memory-store
  api:/db: mock-db

setup:
  write api:/cache/user-123 {"id": "123", "name": "Alice"}

run:
  write api:/input {"method": "GET", "path": "/users/123"}

assertions:
  - read api:/output.status == 200
  - read api:/output.body.name == "Alice"
  # DB was not queried (cache hit)
  - read mock-db:/query_count == 0
```

### Integration Test: Full Assembly

```yaml
test: user-service-integration
blocks:
  service: ./user-service  # The whole Assembly
mounts:
  # Use test database
  service:/blocks/db: test-postgres

setup:
  # Seed test data
  write test-postgres:/users {"id": "1", "name": "Seed User"}

run:
  # Make HTTP request through gateway
  write service:/input {
    "method": "GET",
    "path": "/users/1",
    "headers": {"Authorization": "Bearer valid-test-token"}
  }

assertions:
  - read service:/output.status == 200
  - read service:/output.body.id == "1"
```

## Deployment Variants

### Development

```yaml
assembly: user-service-dev
extends: user-service
config:
  gateway:
    port: 3000
  db:
    connection_string: "postgres://localhost/dev"
  auth:
    # Skip real auth in dev
    bypass: true
```

### Production

```yaml
assembly: user-service-prod
extends: user-service
config:
  gateway:
    port: 8080
    tls:
      cert: /secrets/tls/cert.pem
      key: /secrets/tls/key.pem
  db:
    connection_string: ${DATABASE_URL}
    pool_size: 20
  cache:
    cluster: ${REDIS_CLUSTER}
replicas: 3
```

### Testing

```yaml
assembly: user-service-test
extends: user-service
mounts:
  # Replace real services with mocks
  /blocks/db: test-doubles:postgres
  /blocks/cache: test-doubles:redis
config:
  auth:
    test_tokens: ["test-token-1", "test-token-2"]
```

## Key Points

1. **Separation of concerns**: Each Block does one thing. Auth doesn't know
   about databases. API doesn't know about HTTP.

2. **Testable at any level**: Test a Block alone, test the Assembly, test
   through HTTP—same patterns.

3. **Environment via configuration**: Dev, staging, prod differ only in
   config and mounts.

4. **Observable by default**: Metrics exported automatically. Add tracing
   by wiring `/ctx/iso/trace`.
