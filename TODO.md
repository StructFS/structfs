* Add a register system to the repl that allows a command prefixed with
  @some_string_here to assign the output of a command to that register.  Note
  that these should also become valid paths to read to and write from as well,
  with full path support being valid.  So I could `@foo read /foo`, then `@bar
  read @foo/baz/qux` and `write /bizzle @baz` etc.
