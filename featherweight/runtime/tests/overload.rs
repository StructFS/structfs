use featherweight_runtime::{
    AssemblyDef, CallBudget, CallLimits, ExecutionScope, Namespace, NativeBlock, Runtime,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use structfs_core_store::{path, Error, Value};

struct Deaf;
impl NativeBlock for Deaf {
    fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
        while !ns.cell().cancel.is_cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
}

async fn queued(cell: &featherweight_runtime::block::BlockCell, count: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while cell.pending_counts().0 != count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overloaded_block_does_not_starve_peers_and_dropped_calls_release_budget() {
    let budget = CallBudget::new(CallLimits {
        calls: 3,
        calls_per_block: 2,
        bytes: 4096,
        bytes_per_block: 2048,
    });
    let request_budget = budget.child(budget.limits());
    let mut runtime = Runtime::new().with_call_budget(request_budget.clone());
    runtime.register_builtin("deaf", Arc::new(|| Box::new(Deaf) as Box<dyn NativeBlock>));
    let def = AssemblyDef::from_str(
        r#"{"assembly":"overload","blocks":{"a":"builtin:deaf","b":"builtin:deaf"},"public":"a"}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), ".".as_ref())
        .unwrap();
    let mut callers = Vec::new();
    for _ in 0..2 {
        let assembly = assembly.clone();
        callers.push(tokio::spawn(async move {
            assembly.read(path!("waiting")).await
        }));
    }
    queued(assembly.public_cell(), 2).await;
    assert!(matches!(
        assembly.read(path!("excess")).await,
        Err(Error::Overloaded { .. })
    ));
    let peer = assembly.clone();
    let peer_call = tokio::spawn(async move { peer.read_block("b", path!("waiting")).await });
    queued(assembly.cell("b").unwrap(), 1).await;
    assert_eq!(budget.usage().calls, 3);
    assert_eq!(request_budget.usage().calls, 3);
    budget.set_limits(CallLimits {
        calls: 0,
        ..budget.limits()
    });
    assert!(matches!(
        assembly.read(path!("reload")).await,
        Err(Error::Overloaded { .. })
    ));
    for call in callers {
        call.abort();
        assert!(call.await.unwrap_err().is_cancelled());
    }
    peer_call.abort();
    let _ = peer_call.await;
    assert_eq!(budget.usage().calls, 0);
    assert_eq!(budget.usage().bytes, 0);
    assert_eq!(request_budget.usage().calls, 0);
    assert_eq!(request_budget.metrics().admitted, 3);
    assert_eq!(assembly.public_cell().pending_counts(), (0, 0));
    assert_eq!(assembly.cell("b").unwrap().pending_counts(), (0, 0));
    assert!(matches!(
        assembly
            .write(path!("large"), Value::Bytes(vec![0; 4096]))
            .await,
        Err(Error::Overloaded { .. })
    ));
    assert_eq!(budget.usage().calls, 0);
    assembly.shutdown(Duration::ZERO).await;
    assert_eq!(runtime.registered_blocks(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_execution_deadline_cleans_pending_requests_and_stops_blocks() {
    let budget = CallBudget::new(CallLimits::default());
    let scope = ExecutionScope::new(Duration::from_millis(100));
    let mut runtime = Runtime::new()
        .with_call_budget(budget.clone())
        .with_execution_scope(scope);
    runtime.register_builtin("deaf", Arc::new(|| Box::new(Deaf) as Box<dyn NativeBlock>));
    let def = AssemblyDef::from_str(
        r#"{"assembly":"deadline","blocks":{"server":"builtin:deaf"},"public":"server"}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), ".".as_ref())
        .unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(2), assembly.read(path!("waiting")))
        .await
        .unwrap();
    assert!(matches!(answer, Err(Error::DeadlineExceeded { .. })));
    assert_eq!(assembly.public_cell().pending_counts(), (0, 0));
    assert_eq!(budget.usage().calls, 0);
    assembly.shutdown(Duration::ZERO).await;
    assert_eq!(runtime.registered_blocks(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_budget_distinguishes_identical_transcript_ids_in_separate_sessions() {
    let budget = CallBudget::new(CallLimits {
        calls: 2,
        calls_per_block: 1,
        bytes: 4096,
        bytes_per_block: 2048,
    });
    let def = AssemblyDef::from_str(
        r#"{"assembly":"same","blocks":{"server":"builtin:deaf"},"public":"server"}"#,
    )
    .unwrap();
    let mut sessions = Vec::new();
    let mut callers = Vec::new();
    for _ in 0..2 {
        let mut runtime = Runtime::new().with_call_budget(budget.clone());
        runtime.register_builtin("deaf", Arc::new(|| Box::new(Deaf) as Box<dyn NativeBlock>));
        let assembly = runtime
            .instantiate(&def, HashMap::new(), ".".as_ref())
            .unwrap();
        let caller = assembly.clone();
        callers.push(tokio::spawn(
            async move { caller.read(path!("waiting")).await },
        ));
        queued(assembly.public_cell(), 1).await;
        sessions.push((runtime, assembly));
    }
    assert_eq!(
        sessions[0].1.public_cell().id,
        sessions[1].1.public_cell().id
    );
    assert_eq!(budget.usage().calls, 2);
    for caller in callers {
        caller.abort();
        let _ = caller.await;
    }
    for (runtime, assembly) in sessions {
        assembly.shutdown(Duration::ZERO).await;
        assert_eq!(runtime.registered_blocks(), 0);
    }
    assert_eq!(budget.usage().calls, 0);
}
