// The handful of web globals this host touches, declared minimally.
//
// The full `dom` and `webworker` libs cannot coexist in one strict
// compilation, and this codebase runs in three global scopes (browser
// window, browser worker, Node). Rather than pick one lib and lie
// about the others, declare exactly what is used: the WebAssembly JS
// API, BufferSource, and the worker-scope messaging surface. Runtime
// presence is always feature-checked before use.

type BufferSource = ArrayBufferView | ArrayBuffer;

declare namespace WebAssembly {
  interface Memory {
    readonly buffer: ArrayBuffer;
  }

  interface Instance {
    readonly exports: Record<string, unknown>;
  }

  interface Module {
    readonly _brand?: "module";
  }

  type ImportValue = ((...args: never[]) => unknown) | number | Memory;
  type Imports = Record<string, Record<string, ImportValue>>;

  function instantiate(
    bytes: BufferSource,
    imports?: Imports,
  ): Promise<{ instance: Instance; module: Module }>;
}

/// The worker-scope messaging surface (browser workers only; in Node
/// `self` is checked for absence and worker_threads is used instead).
interface WorkerScopeMessageEvent {
  readonly data: unknown;
}

declare const self:
  | {
      postMessage(message: unknown): void;
      onmessage: ((event: WorkerScopeMessageEvent) => void) | null;
    }
  | undefined;

declare const window: unknown;
