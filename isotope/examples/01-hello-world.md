# Example: Hello World

The simplest Isotope program: a Block that echoes greetings.

## The Block

A Block is defined by how it handles Requests. This Block:

- Reads a name from `/name`
- Returns a greeting

```python
# Declare interface
write("/iso/self/interface", {
    "paths": {
        "/name": {"write": "Set the name to greet"},
        "/greeting": {"read": "Get the greeting"}
    }
})

# State
name = "World"

# Run loop
while True:
    request = read("/iso/server/requests")

    if request is None:
        break

    if request.op == "write" and request.path == "name":
        name = request.data
        write(request.respond_to, {"result": "ok", "path": "name"})

    elif request.op == "read" and request.path == "greeting":
        greeting = f"Hello, {name}!"
        write(request.respond_to, {"result": "ok", "value": greeting})

    else:
        write(request.respond_to, {
            "result": "error",
            "error": {"type": "not_found", "message": f"Unknown path: {request.path}"}
        })

write("/iso/shutdown/complete", {})
```

## Running Standalone

To run this Block, we need an Assembly:

```yaml
assembly: hello-standalone
version: 1.0.0

blocks:
  greeter: ./greeter-block.wasm

public: greeter
```

When this Assembly runs:
1. `greeter` Block starts (it's the public Block)
2. External requests go to `greeter`

## Using the Block

From another Block or externally:

```python
# Set the name
write("/services/greeter/name", "Isotope")

# Get the greeting
greeting = read("/services/greeter/greeting")
# → "Hello, Isotope!"
```

## Composing in an Assembly

Wire the greeter into a larger service:

```yaml
assembly: greeting-service
version: 1.0.0

blocks:
  api: ./api-block.wasm
  greeter: ./greeter-block.wasm

public: api

wiring:
  api:/services/greeter -> greeter
```

The `api` Block can now use the greeter:

```python
# Inside api Block
write("/services/greeter/name", user_name)
greeting = read("/services/greeter/greeting")
```

## Testing

Test the greeter in isolation by creating a test Assembly:

```yaml
assembly: greeter-test
version: 1.0.0

blocks:
  greeter: ./greeter-block.wasm
  test_harness: ./test-harness.wasm

public: test_harness

wiring:
  test_harness:/services/greeter -> greeter
```

The test harness can verify behavior:

```python
# In test harness
write("/services/greeter/name", "Test")
result = read("/services/greeter/greeting")
assert result == "Hello, Test!"

write("/services/greeter/name", "")
result = read("/services/greeter/greeting")
assert result == "Hello, !"  # Edge case
```

## Key Points

1. **The Block is simple**: It reads Requests, processes them, writes Responses.
   It doesn't know about networking, other Blocks, or where it's mounted.

2. **Composition is external**: The Assembly wires paths. The Block doesn't
   change when composition changes.

3. **Testing is just wiring**: Provide a test harness Block, wire to the
   Block under test, verify behavior.

4. **Interface declaration**: Writing to `/iso/self/interface` enables tooling
   to understand the Block's API.
