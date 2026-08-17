// jawohl from Node, through jedem-generated bindings.
//
// Same Rust surface as the Python test. Nobody wrote this binding either --
// and note the names are camelCase, because that is what a JS caller expects.
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const jawohl = require("./.nodeimport/jawohl.node");

let fails = 0;
const check = (label, got, want) => {
  if (JSON.stringify(got) !== JSON.stringify(want)) {
    console.log(`  FAIL ${label}: got ${JSON.stringify(got)}, want ${JSON.stringify(want)}`);
    fails++;
  } else {
    console.log(`  ok   ${label}: ${JSON.stringify(got)}`);
  }
};

const SCHEMA = JSON.stringify({
  type: "object",
  required: ["role"],
  properties: {
    role: { enum: ["user", "admin"] },
    limit: { type: "integer", maximum: 100 },
  },
});

console.log("names are camelCase, from the same snake_case Rust");
check("completeJson exists", typeof jawohl.completeJson, "function");
check("complete_json does not", jawohl.complete_json, undefined);
check("isIrrecoverable exists", typeof jawohl.isIrrecoverable, "function");

console.log("completion");
check("completeJson", jawohl.completeJson('{"query":"rust par'), '{"query":"rust par"}');
check("finishes a literal", jawohl.completeJson('{"a": tru'), '{"a": true}');
check("drops a dangling escape", jawohl.completeJson('{"a": "x\\'), '{"a": "x"}');

console.log("malformed input throws");
try {
  jawohl.completeJson('{"a": 1}}');
  console.log("  FAIL should have thrown"); fails++;
} catch (e) {
  check("throws", e.message.includes("trailing"), true);
}

console.log("the stability guarantee, from JS");
check("string still open",  jawohl.status('{"q":"rust par', "/q"), "incomplete");
check("string closed",      jawohl.status('{"q":"rust parser"', "/q"), "complete");
check("number undelimited", jawohl.status('{"n":10', "/n"), "incomplete");
check("number delimited",   jawohl.status('{"n":10}', "/n"), "complete");

console.log("validation as it arrives");
check("live prefix", jawohl.validate(SCHEMA, '{"role":"us', "/role"), "pending");
check("dead prefix", jawohl.validate(SCHEMA, '{"role":"sup', "/role"), "irrecoverably_invalid");

console.log("the function worth crossing a language boundary for");
check("cancel: bad enum",  jawohl.isIrrecoverable(SCHEMA, '{"role":"sup'), true);
check("cancel: bad bound", jawohl.isIrrecoverable(SCHEMA, '{"role":"user","limit":1000'), true);
check("keep going",        jawohl.isIrrecoverable(SCHEMA, '{"role":"user","limit":5'), false);

console.log("Option<T> arrives as null");
check("not settled yet", jawohl.settledValue('{"q":"rust par', "/q"), null);
check("settled",         jawohl.settledValue('{"q":"ok"', "/q"), '{"q":"ok"}');

console.log("a synchronous function stays synchronous");
check("no promise", jawohl.completeJson('{"a":1') instanceof Promise, false);

if (fails) { console.log(`\n${fails} failure(s)`); process.exit(1); }
console.log("\nall checks passed");
