# Why Stores All The Way Down

This document explains why Isotope uses store operations for everything.

## The Problem With Multiple Interfaces

Traditional operating systems have accumulated many interfaces:

- System calls (read, write, ioctl, mmap, ...)
- Signals (SIGTERM, SIGINT, SIGUSR1, ...)
- Shared memory
- Pipes and FIFOs
- Unix domain sockets
- Network sockets
- Files
- procfs, sysfs, devfs
- D-Bus, XPC, COM
- Environment variables

Each interface has:
- Its own semantics
- Its own error handling
- Its own documentation
- Its own learning curve
- Its own implementation complexity

Worse, they don't compose. You can't take a network socket and mount it as a
file. You can't send a signal through a pipe. You can't query D-Bus through
procfs.

## Plan 9's Answer

Plan 9 showed a different way: everything is a file, everything speaks 9P.

- Process information? `/proc/{pid}/*`
- Network connections? `/net/tcp/{conn}/*`
- Window system? `/dev/draw/*`
- Remote resources? `import` them into your namespace

This was revelatory. Suddenly you could:
- `cat /proc/1/status` to inspect init
- `echo hangup > /net/tcp/0/ctl` to close a connection
- `cp /net/tcp/0/data remote:/net/tcp/1/data` to proxy connections

One interface, universal composition.

## Why Not Just Files?

Files are great, but they have limitations:

1. **Byte streams**: Files are sequences of bytes. Structured data requires
   encoding/decoding at every boundary.

2. **Limited metadata**: Files have a fixed set of attributes (size, mtime,
   permissions). Application-specific metadata must be encoded in the content.

3. **No schema**: A file doesn't describe its own structure. You need external
   knowledge to interpret it.

4. **Awkward for requests**: Files are good for data at rest. Request/response
   patterns (like HTTP) require convention (write request file, read response
   file).

## Stores: Files++

A Store is a file server that:

1. **Speaks Values**: Stores exchange structured data (maps, arrays, strings,
   numbers) not just bytes. The structure is preserved end-to-end.

2. **Supports patterns**: Different stores can implement different interaction
   patterns (key-value, queue, stream, request-response) behind the same
   read/write interface.

3. **Composes naturally**: Mount store A at path `/a`, store B at path `/b`,
   and you have a composite store that routes by prefix.

4. **Location-transparent**: A Store might be in-memory, on disk, in another
   process, or on a remote machine. The interface is the same.

## The Payoff

With stores as the universal interface:

### Testing becomes trivial

```
# Production: real HTTP client
mount /ctx/iso/http → http_client_store

# Testing: mock responses
mount /ctx/iso/http → mock_http_store
```

The Block under test doesn't change. It still reads and writes paths.

### Debugging becomes inspection

```
# Interpose a logging store
mount /services/database → logging_store(real_db_store)

# Now every read/write is logged
```

### Migration becomes remounting

```
# Move a service to a remote machine
unmount /services/auth
mount /services/auth → remote://auth-server/

# Clients don't change
```

### Scaling becomes fan-out

```
# Single instance
mount /services/worker → worker_1

# Scaled out
mount /services/worker → load_balancer([worker_1, worker_2, worker_3])
```

## The Cost

There are costs to universal stores:

1. **Indirection**: Every operation goes through the store interface. This
   adds overhead compared to direct function calls.

2. **Loss of type safety**: Stores exchange Values, which are dynamically
   typed. Static type checking happens at the boundary, not throughout.

3. **Learning curve**: Developers must think in paths and store operations,
   not objects and method calls.

4. **Unfamiliar patterns**: Request-response via write-then-read feels strange
   to developers used to function calls.

We believe the benefits outweigh the costs, but this is a trade-off, not a
free lunch.

## Prior Art

- **Plan 9**: Everything is a file, 9P protocol
- **Inferno**: Styx protocol (successor to 9P), Limbo language
- **FUSE**: User-space file systems
- **9P2000.L**: Linux's 9P implementation
- **REST**: Resources identified by URIs, uniform interface
- **Actor Model**: Message-passing between isolated actors
- **Erlang/OTP**: Processes communicate only via messages
- **Capability systems**: Access mediated by unforgeable tokens (paths)
