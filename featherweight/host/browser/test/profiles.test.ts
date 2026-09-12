import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { HeadlessSession, validate } from "../profiles.ts";
import type { InputEnvelope } from "../profiles.ts";
test("interactive v1 shared corpus and independent acknowledgments", () => {
 const cases = JSON.parse(readFileSync(new URL("../../../../packages/profiles/tests/fixtures/input-v1.json", import.meta.url), "utf8")) as { event: Omit<InputEnvelope,"sequence"> & {sequence:number}; ok:boolean }[];
 let last = 0n;
 for (const c of cases) { const e = {...c.event,sequence:BigInt(c.event.sequence)}; if(c.ok){validate(e,"s",last);last=e.sequence;}else assert.throws(()=>validate(e,"s",last)); }
 const a = new HeadlessSession("a",2,1024); const b = new HeadlessSession("b",1,1024);
 for(const sequence of [1n,2n])a.submit({version:1,session:"a",sequence,input:{type:"key",text:"x"}});
 assert.throws(()=>a.submit({version:1,session:"a",sequence:3n,input:{type:"key",text:"x"}}));assert.equal(a.accepted,2n);
 assert.equal(a.next()?.sequence,1n);a.acknowledge(1n);assert.equal(a.next()?.sequence,2n);a.acknowledge(2n);
 assert.equal(a.rendered,undefined);a.present({epoch:"e",revision:4n});assert.equal(a.processed,2n);
 a.release();b.submit({version:1,session:"b",sequence:1n,input:{type:"paste",text:"peer"}});assert.equal(b.next()?.sequence,1n);
 assert.throws(()=>validate({version:1,session:"s",sequence:1n,input:{type:"key",text:"\ud800"}},"s",0n));
 validate({version:1,session:"s",sequence:(1n<<64n)-1n,input:{type:"close"}},"s",(1n<<64n)-2n);
});
