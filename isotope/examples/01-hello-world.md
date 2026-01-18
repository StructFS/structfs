# Example: Hello World

The simplest Isotope program: a Block that reads a name and writes a greeting.

## The Block

A Block is defined by its behavior: what it reads, what it writes.

```
Block: greeter

On start:
  1. Read /input/name
  2. If found: greeting = "Hello, " + name + "!"
     Else: greeting = "Hello, World!"
  3. Write greeting to /output/greeting
  4. Exit successfully
```

This Block:
- Reads from one path (`/input/name`)
- Writes to one path (`/output/greeting`)
- Has no other dependencies

## Running Standalone

To run this Block, we need an Assembly that provides its namespace:

```
assembly: hello-standalone
blocks:
  greeter: ./greeter-block
mounts:
  # Provide input from a literal value
  greeter:/input/name <- "Isotope"
```

When this Assembly runs:
1. `greeter` Block starts
2. Reads `/input/name` → gets "Isotope"
3. Writes `/output/greeting` → "Hello, Isotope!"
4. Block exits

## Running With Dynamic Input

More realistically, input comes from somewhere:

```
assembly: hello-dynamic
blocks:
  greeter: ./greeter-block

mounts:
  # Input comes from Assembly's input
  greeter:/input <- /input

  # Output goes to Assembly's output
  greeter:/output -> /output

exports:
  # Expose the greeter's output
  output: /output
```

Now the Assembly itself takes input:

```
# From outside the Assembly:
write /assemblies/hello-dynamic/input/name "World"
read /assemblies/hello-dynamic/output/greeting
→ "Hello, World!"
```

## Running As HTTP Service

Wire it to an HTTP handler:

```
assembly: hello-http
blocks:
  greeter: ./greeter-block
  http: stdlib:http-server

wiring:
  # HTTP requests become greeter inputs
  http.requests -> greeter:/input

  # Greeter outputs become HTTP responses
  greeter:/output -> http.responses

config:
  http:
    port: 8080
    routes:
      GET /hello/{name}:
        input: /name
        output: /greeting
```

Now:
```
curl http://localhost:8080/hello/Isotope
→ "Hello, Isotope!"
```

The greeter Block is unchanged. Only the wiring differs.

## Testing

Test the greeter Block in isolation:

```
test: greeter-test
blocks:
  greeter: ./greeter-block

setup:
  # Provide test input
  write greeter:/input/name "Test"

assertions:
  # Check output
  read greeter:/output/greeting == "Hello, Test!"
```

Or test error handling:

```
test: greeter-no-input
blocks:
  greeter: ./greeter-block

setup:
  # Don't provide input

assertions:
  # Should use default
  read greeter:/output/greeting == "Hello, World!"
```

## Key Points

1. **The Block is simple**: It just reads and writes paths. It doesn't know
   where data comes from or where it goes.

2. **Composition is external**: The Assembly wires paths together. The Block
   doesn't change when composition changes.

3. **Testing is easy**: Provide inputs, check outputs. No mocking framework
   needed.

4. **Deployment is configuration**: HTTP, CLI, queue-based—just different
   Assemblies around the same Block.
