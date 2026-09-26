// The typed error taxonomy, as the native runtime defines it
// (featherweight-runtime's `protocol::ErrorKind`; isotope specs 07, 11, 12).
//
// One row per kind: its spec 11 status, its spec 07 wire type, the label
// transcripts and session logs record, whether it is retryable, and
// whether it is what its status denotes on its own. The rows are pinned to
// the native table: test/errors.test.ts compares them with the committed
// test/fixtures/error-kinds.json, which the native runtime's own tests
// generate and check. Change the taxonomy there first.

import { status, type Status } from "./structfs-host.ts";

export interface ErrorKindRow {
  canonical: boolean;
  kind: string;
  label: string;
  retryable: boolean;
  status: number;
  wire: string;
}

export const errorKinds: readonly ErrorKindRow[] = [
  { canonical: true, kind: "NotFound", label: "not_found", retryable: false, status: -1, wire: "not_found" },
  { canonical: false, kind: "NoRoute", label: "no_route", retryable: false, status: -1, wire: "no_route" },
  { canonical: true, kind: "PermissionDenied", label: "permission_denied", retryable: false, status: -2, wire: "forbidden" },
  { canonical: true, kind: "Conflict", label: "conflict", retryable: false, status: -3, wire: "conflict" },
  { canonical: true, kind: "InvalidArgument", label: "invalid_argument", retryable: false, status: -10, wire: "invalid_argument" },
  { canonical: true, kind: "Overloaded", label: "overloaded", retryable: true, status: -4, wire: "unavailable" },
  { canonical: true, kind: "DeadlineExceeded", label: "deadline_exceeded", retryable: true, status: -5, wire: "timeout" },
  { canonical: true, kind: "ResourceLimit", label: "resource_limit", retryable: false, status: -8, wire: "resource_limit" },
  { canonical: true, kind: "Cancelled", label: "cancelled", retryable: false, status: -6, wire: "cancelled" },
  { canonical: true, kind: "InvalidPath", label: "invalid_path", retryable: false, status: -7, wire: "invalid_path" },
  { canonical: false, kind: "Codec", label: "codec", retryable: false, status: -9, wire: "store_error" },
  { canonical: false, kind: "CodecResourceLimit", label: "codec", retryable: false, status: -8, wire: "resource_limit" },
  { canonical: true, kind: "Other", label: "other", retryable: false, status: -9, wire: "store_error" },
];

const isStatus = (code: number): code is Status =>
  (Object.values(status) as number[]).includes(code);

/// The transcript/session label a bare status denotes (`other` for an
/// unknown code) — what a host that sees only the status records.
export function labelOfStatus(code: number): string {
  return (
    errorKinds.find((row) => row.canonical && row.status === code)?.label ??
    "other"
  );
}

/// The status a transcript label replays as. A `codec` failure whose
/// diagnostic kind is `resource_limit` is a codec resource limit (-8);
/// unknown labels replay as `other` (-9).
export function statusOfLabel(label: string, codecKind?: string): Status {
  if (label === "codec" && codecKind === "resource_limit") {
    return status.RESOURCE_LIMIT;
  }
  const code = errorKinds.find((row) => row.label === label)?.status;
  return code !== undefined && isStatus(code) ? code : status.OTHER;
}

/// Whether a label names a kind this host knows.
export function knownLabel(label: string): boolean {
  return errorKinds.some((row) => row.label === label);
}
