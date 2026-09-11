;; The cross-host transcript probe: a spec 11 guest that touches every
;; transcript answer kind — entropy and the ns clock (inputs no live
;; rerun can reproduce), a refusal (an unwired path), and two writes
;; whose payload digests both hosts must compute identically.
;;
;; Recorded under the native runtime and replayed by the TS host, and
;; vice versa; the committed transcripts beside this file pin the wire
;; format between the two implementations byte-for-byte.
;;
;; Exit codes name the first expectation that failed, so a divergence
;; in either host points at the exact operation.
(module
  (import "structfs" "read"
    (func $read (param i32 i32 i32) (result i32)))
  (import "structfs" "write"
    (func $write (param i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (global $bump (mut i32) (i32.const 4096))
  (func (export "block_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))

  (data (i32.const 1100) "iso/random/uuid")        ;; 15
  (data (i32.const 1120) "iso/time/now_unix_ns")   ;; 20
  (data (i32.const 1144) "secrets")                ;; 7
  (data (i32.const 1160) "iso/stdio/stdout")       ;; 16
  (data (i32.const 1180) "iso/log/info")           ;; 12
  (data (i32.const 1200) "\22hi\22")               ;; 4
  (data (i32.const 1210) "\22ran\22")              ;; 5
  (data (i32.const 1232)
    "{\22name\22:\22transcript-probe\22,\22serialization\22:\22application/json\22}") ;; 62

  (func (export "manifest") (param $ret i32) (result i32)
    (i32.store (local.get $ret) (i32.const 1232))
    (i32.store (i32.add (local.get $ret) (i32.const 4)) (i32.const 62))
    (i32.const 0))

  (func (export "run") (result i32)
    ;; read iso/random/uuid -> present
    (if (i32.ne
          (call $read (i32.const 1100) (i32.const 15) (i32.const 1024))
          (i32.const 0))
      (then (return (i32.const 1))))
    ;; read iso/time/now_unix_ns -> present
    (if (i32.ne
          (call $read (i32.const 1120) (i32.const 20) (i32.const 1024))
          (i32.const 0))
      (then (return (i32.const 2))))
    ;; read secrets -> permission denied (-2): a refusal is an answer
    (if (i32.ne
          (call $read (i32.const 1144) (i32.const 7) (i32.const 1024))
          (i32.const -2))
      (then (return (i32.const 3))))
    ;; write "hi" to iso/stdio/stdout -> acknowledged
    (if (i32.ne
          (call $write (i32.const 1160) (i32.const 16)
            (i32.const 1200) (i32.const 4) (i32.const 1024))
          (i32.const 0))
      (then (return (i32.const 4))))
    ;; write "ran" to iso/log/info -> acknowledged
    (if (i32.ne
          (call $write (i32.const 1180) (i32.const 12)
            (i32.const 1210) (i32.const 5) (i32.const 1024))
          (i32.const 0))
      (then (return (i32.const 5))))
    (i32.const 0)))
