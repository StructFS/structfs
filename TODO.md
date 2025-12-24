* If we save to a register, don't print the value
* Factor the repl core so all its interactions / etc. happen through StructFS
  itself.  I.e. if the repl were running in Wasm with only the host only
  providing StructFS for its various ops, then it would be identical.  The
  wrapping stdin/out/etc. should all run over StructFS at this interface
  boundary and the particular CLI command "host" should provide these stores to
  the core repl app.
