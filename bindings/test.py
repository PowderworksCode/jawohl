"""jawohl from Python, through jedem-generated bindings.

The point is not that these calls work -- it is that nobody wrote them. The
Rust surface was annotated once; this module is generated.
"""
import sys
import jawohl

fails = 0
def check(label, got, want):
    global fails
    if got != want:
        print(f"  FAIL {label}: got {got!r}, want {want!r}"); fails += 1
    else:
        print(f"  ok   {label}: {got!r}")

SCHEMA = """{
  "type": "object",
  "required": ["role"],
  "properties": {
    "role":  {"enum": ["user", "admin"]},
    "limit": {"type": "integer", "maximum": 100}
  }
}"""

print("completion -- the promise this README made in 2023")
check("complete_json", jawohl.complete_json(r'{"query":"rust par'), '{"query":"rust par"}')
check("finishes a literal", jawohl.complete_json(r'{"a": tru'), '{"a": true}')
# NB: a raw string would give TWO backslashes -- a complete escape, and a
# different test. One trailing backslash is the unresolved case.
check("drops a dangling escape", jawohl.complete_json('{"a": "x\\'), '{"a": "x"}')
check("keeps a complete escape", jawohl.complete_json(r'{"a": "x\\'), '{"a": "x\\\\"}')

print("malformed input raises, it does not return garbage")
try:
    jawohl.complete_json('{"a": 1}}')
    print("  FAIL should have raised"); fails += 1
except ValueError as e:
    check("raises", "trailing" in str(e), True)

print("statuses are a real enum, not a magic string")
check("is a Syntax", type(jawohl.status('{"a":1}', "/a")).__name__, "Syntax")
check("is a Validation", type(jawohl.validate(SCHEMA, '{"role":"user"}', "")).__name__, "Validation")
try:
    jawohl.status('{"a":1}', "/a") == "Complete"
    check("an enum is not its name", jawohl.status('{"a":1}', "/a") == "Complete", False)
except TypeError:
    print("  ok   an enum does not compare equal to a string")

print("the stability guarantee, from Python")
check("string still open",   jawohl.status(r'{"q":"rust par', "/q"), jawohl.Syntax.Incomplete)
check("string closed",       jawohl.status(r'{"q":"rust parser"', "/q"), jawohl.Syntax.Complete)
check("number undelimited",  jawohl.status(r'{"n":10', "/n"), jawohl.Syntax.Incomplete)
check("number delimited",    jawohl.status(r'{"n":10}', "/n"), jawohl.Syntax.Complete)
check("absent",              jawohl.status(r'{"q":"x"', "/nope"), jawohl.Syntax.Missing)

print("validation as it arrives")
check("live prefix",   jawohl.validate(SCHEMA, r'{"role":"us', "/role"), jawohl.Validation.Pending)
check("dead prefix",   jawohl.validate(SCHEMA, r'{"role":"sup', "/role"), jawohl.Validation.IrrecoverablyInvalid)
check("valid",         jawohl.validate(SCHEMA, r'{"role":"admin"}', ""), jawohl.Validation.Valid)

print("the function worth crossing a language boundary for")
check("cancel: bad enum",  jawohl.is_irrecoverable(SCHEMA, r'{"role":"sup'), True)
check("cancel: bad bound", jawohl.is_irrecoverable(SCHEMA, r'{"role":"user","limit":1000'), True)
check("keep going",        jawohl.is_irrecoverable(SCHEMA, r'{"role":"user","limit":5'), False)

print("Option<T> arrives as None, not a sentinel")
check("not settled yet", jawohl.settled(r'{"q":"rust par', "/q"), None)
check("settled",         jawohl.settled(r'{"q":"ok"', "/q"), '{"q":"ok"}')

print("constraints jawohl could not lower are reported")
check("report mentions if", "if" in jawohl.lowering_report('{"type":"string","if":{}}'), True)
check("clean schema, empty report", jawohl.lowering_report('{"type":"string"}').strip().endswith("0 unsupported"), True)

print("doc comments arrived as docstrings")
check("docstring", (jawohl.complete_json.__doc__ or "").strip().splitlines()[0],
      "Complete a truncated JSON document so that it parses.")

if fails:
    print(f"\n{fails} failure(s)"); sys.exit(1)
print("\nall checks passed")
