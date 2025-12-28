# The Meta Lens Pattern

A **lens** in StructFS is a virtual prefix over a shared namespace that provides
added functionality. The **meta lens** exposes introspective information about
paths accessible in a given store.

## Properties

### Prefix Separation

The meta lens occupies a `meta/` prefix within the store's namespace. For any
path `P` that the store handles, `meta/P` returns information *about* that path
rather than returning the actual data *at* that path.

```
/ctx/sys/fs/handles/0       -> file content (data)
/ctx/sys/fs/meta/handles/0  -> path affordances (meta)
```

### Structural Consistency

The meta lens mirrors the structure of the underlying namespace. If `P` is a
valid path, then `meta/P` returns metadata for that path. If `P` is invalid,
`meta/P` returns an appropriate error or null.

## Suffix Meta vs Prefix Meta

Some stores use a suffix pattern (`path/meta`) for metadata about the underlying
data. The prefix pattern (`meta/path`) serves a different purpose:

- **Suffix meta**: Properties of the data (file size, modification time)
- **Prefix meta**: Properties of the path itself (what operations work, what to expect)

These are orthogonal. A file handle might have both:
- `handles/0/meta` - the file is 4096 bytes, last modified yesterday
- `meta/handles/0` - this path is readable and writable, position affects reads

## Case Study: FsStore

The filesystem store provides an example of how meta lens might work in practice.

### Without Meta

```
/ctx/sys/fs/
    open              # write {path, mode} -> handle path
    handles/
        0             # read/write file content
        0/position    # current seek position
        0/meta        # file size, is_file, is_dir (suffix pattern)
        0/close       # write to close
```

### With Meta

```
/ctx/sys/fs/meta/
    open              # what does open accept?
    handles/
        0             # what can I do with this handle?
        0/position    # what is position? can I write to it?
```

Reading `meta/handles/0` might return:

```json
{
  "readable": true,
  "writable": true,
  "state": {
    "position": 1024,
    "encoding": "utf8"
  },
  "fields": {
    "position": "read/write integer, current offset",
    "meta": "read-only file properties",
    "close": "write to close handle"
  }
}
```

### Writable Meta

If `meta/handles/0/position` is writable, writing to it seeks the file. The meta
lens becomes both introspection and a control interface for path-level state.

```
write /ctx/sys/fs/meta/handles/0/position 500
```

This is consistent with StructFS's two-verb model: if something is observable
via read, it can be modified via write.

## Open Questions

This pattern is exploratory. Some things we don't know yet:

- Should meta responses have a consistent schema, or is freeform adequate?
- How should meta interact with overlay stores that compose multiple sub-stores?
- Is `meta/meta/P` meaningful?
- Should meta paths be enumerable?

The FS implementation will help answer these questions empirically.
