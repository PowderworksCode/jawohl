// jawohl from Node, through jedem-generated bindings.
//
// Same Rust annotations as the Python test. Nobody wrote this binding either --
// and note the names are camelCase, because that is what a JS caller expects.
//
// `Stream` is a handle, so this is the real incremental parser: one object, fed
// chunk by chunk, keeping its state across calls.
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const jawohl = require("./.jedem/jawohl.node");

let fails = 0;
const check = (label, got, want) => {
  if (JSON.stringify(got) !== JSON.stringify(want)) {
    console.log(`  FAIL ${label}: got ${JSON.stringify(got)}, want ${JSON.stringify(want)}`);
    fails++;
  } else {
    console.log(`  ok   ${label}: ${JSON.stringify(got)}`);
  }
};
const bytes = (s) => new Uint8Array(Buffer.from(s, "utf8"));

const SCHEMA = JSON.stringify({
  type: "object",
  required: ["role"],
  properties: {
    role: { enum: ["user", "admin"] },
    limit: { type: "integer", maximum: 100 },
  },
});

console.log("completion -- the promise jawohl made in 2023");
check("completeJson", jawohl.completeJson('{"query":"rust par'), '{"query":"rust par"}');
check("finishes a literal", jawohl.completeJson('{"a": tru'), '{"a": true}');
check("drops a dangling escape", jawohl.completeJson('{"a": "x\\'), '{"a": "x"}');
check("keeps a complete escape", jawohl.completeJson('{"a": "x\\\\'), '{"a": "x\\\\"}');
check("closing suffix only", jawohl.getClosingStringForPartialJson('{"a":1'), "}");

console.log("malformed input throws, it does not return garbage");
try {
  jawohl.completeJson('{"a": 1}}');
  console.log("  FAIL should have thrown"); fails++;
} catch (e) {
  check("throws", /trailing/.test(String(e)), true);
}

console.log("a Stream is a live object, fed one chunk at a time");
const s = new jawohl.Stream();
s.push(bytes('{"query":"rust '));
check("not done yet", s.isDocumentComplete(), false);
check("string still open", s.status("/query"), jawohl.Syntax.Incomplete);
s.push(bytes('parser"'));
// The same object, remembering the first chunk -- that is what a handle buys.
check("closed by a later chunk", s.status("/query"), jawohl.Syntax.Complete);
s.push(bytes("}"));
check("document closed", s.isDocumentComplete(), true);

console.log("the Rust builders cross unchanged");
// `withMaxDepth` and `withNumberProfile` take `self` and return `Self` in Rust.
// Nothing was annotated or reshaped; the binding mutates in place and returns
// `this`, so the chain reads as it does in Rust.
const shallow = new jawohl.Stream().withMaxDepth(3);
try {
  shallow.push(bytes("[[[[[1]]]]]"));
  console.log("  FAIL depth limit was not applied"); fails++;
} catch {
  console.log("  ok   depth limit applied through the builder");
}
const chained = new jawohl.Stream().withMaxDepth(64);
check("chain returns the same handle", chained.withMaxDepth(64) === chained, true);
// The profile builder takes an enum, and only bites with a schema attached.
const exact = jawohl.Stream
  .fromJsonSchema('{"type":"integer","maximum":100}')
  .withNumberProfile(jawohl.NumberProfile.Exact);
exact.push(bytes("1000"));
// Under Exact, `1000` may still become `1000e-9`, so no bound is decided yet.
check("Exact defers the bound", exact.validation(""), jawohl.Validation.Pending);
const plain = jawohl.Stream.fromJsonSchema('{"type":"integer","maximum":100}');
plain.push(bytes("1000"));
check("PlainDecimal decides it", plain.isIrrecoverable(), true);

console.log("two streams are independent");
const a = new jawohl.Stream(), b = new jawohl.Stream();
a.push(bytes('{"n":1}'));
check("a is complete", a.isDocumentComplete(), true);
check("b is untouched", b.isDocumentComplete(), false);

console.log("the stability guarantee, from JavaScript");
const statusOf = (text, pointer) => {
  const st = new jawohl.Stream();
  st.push(bytes(text));
  return st.status(pointer);
};
check("number undelimited", statusOf('{"n":10', "/n"), jawohl.Syntax.Incomplete);
check("number delimited",   statusOf('{"n":10}', "/n"), jawohl.Syntax.Complete);
check("absent",             statusOf('{"q":"x"', "/nope"), jawohl.Syntax.Missing);

console.log("validation, decided as the bytes arrive");
const v = jawohl.Stream.fromJsonSchema(SCHEMA);
v.push(bytes('{"role":"us'));
check("live prefix", v.validation("/role"), jawohl.Validation.Pending);
check("keep going",  v.isIrrecoverable(), false);
v.push(bytes('er","limit":5'));
check("still fine",  v.isIrrecoverable(), false);
v.push(bytes("0}"));
check("valid", v.validation(""), jawohl.Validation.Valid);

console.log("the answer worth crossing a language boundary for");
const deadAt = (chunks) => {
  const st = jawohl.Stream.fromJsonSchema(SCHEMA);
  for (let i = 0; i < chunks.length; i++) {
    st.push(bytes(chunks[i]));
    if (st.isIrrecoverable()) return i;
  }
  return null;
};
// Cancellation lands on the chunk that decided it, not at end of document.
check("bad enum, chunk 1", deadAt(['{"role":"', "sup", 'er"}']), 1);
check("bad bound, chunk 1", deadAt(['{"role":"user","limit":', "1000", "}"]), 1);
check("good input never dies", deadAt(['{"role":"user","limit":', "50", "}"]), null);

console.log("a bad schema throws rather than silently validating nothing");
try {
  jawohl.Stream.fromJsonSchema("{ not json");
  console.log("  FAIL should have thrown"); fails++;
} catch {
  console.log("  ok   bad schema throws");
}

if (fails) {
  console.log(`\n${fails} failure(s)`);
  process.exit(1);
}
console.log("\nall checks passed");
