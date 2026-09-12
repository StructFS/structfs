# structfs-profiles

Versioned state, operation, binary-stream, interactive, configuration, and process
capability declarations. `meta/profiles` discovery is read-only and does not invoke
effects. A declaration names its support level; fixture-only process behavior
never implies an OS sandbox or a production process adapter.

The default `host` feature provides `Profiled`, `OperationHandle` and `HeadlessHost`. Sessions reserve
exclusive presentation surfaces under explicit owners. Input uses session identity
and consecutive sequence numbers; queue admission, processed input, and rendered
state revision are separate acknowledgments. Full queues reject admission without
advancing sequence. Repeated equal keys remain separate events.

Disable default features for portable schemas and validation, including guest SDK
use. State and operation payloads use StructFS Value v1. Input bytes are logical
UTF-8 payload weight plus 64 bytes of metadata and session identity per event.


Operation handles reserve result capacity before invoking a callback. Cancellation
is a request; `joined` reports actual completion. Release clears retained results
while noncooperative work stays charged until joined. The granting application
registers the returned handle path. Callback working memory remains its provider's
responsibility. See Isotope's application capability profiles specification.
