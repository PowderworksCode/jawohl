"""jawohl from Python, through jedem-generated bindings.

The point is not that these calls work -- it is that nobody wrote them. The
Rust API was annotated where it is defined; this binding is generated from it.

`Stream` is a handle, so this is the real incremental parser: one object, fed
chunk by chunk, keeping its state across calls. Before handles it had to be a
batch shim that re-parsed from byte zero every time you asked it anything.
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

print("completion -- the promise jawohl made in 2023")
check("complete_json", jawohl.complete_json(r'{"query":"rust par'), '{"query":"rust par"}')
check("finishes a literal", jawohl.complete_json(r'{"a": tru'), '{"a": true}')
# NB: a raw string would give TWO backslashes -- a complete escape, and a
# different test. One trailing backslash is the unresolved case.
check("drops a dangling escape", jawohl.complete_json('{"a": "x\\'), '{"a": "x"}')
check("keeps a complete escape", jawohl.complete_json(r'{"a": "x\\'), '{"a": "x\\\\"}')
check("closing suffix only", jawohl.get_closing_string_for_partial_json(r'{"a":1'), "}")

print("malformed input raises, it does not return garbage")
try:
    jawohl.complete_json('{"a": 1}}')
    print("  FAIL should have raised"); fails += 1
except ValueError as e:
    check("raises", "trailing" in str(e), True)

print("a Stream is a live object, fed one chunk at a time")
s = jawohl.Stream()
s.push(b'{"query":"rust ')
check("not done yet", s.is_document_complete(), False)
check("string still open", s.status("/query"), jawohl.Syntax.Incomplete)
s.push(b'parser"')
# The same object, remembering the first chunk -- that is what a handle buys.
check("closed by a later chunk", s.status("/query"), jawohl.Syntax.Complete)
s.push(b"}")
check("document closed", s.is_document_complete(), True)

print("the Rust builders cross unchanged")
# `with_max_depth` and `with_number_profile` take `self` and return `Self` in
# Rust. Nothing was annotated or reshaped; the binding mutates in place and
# hands the same object back, so the chain reads as it does in Rust.
shallow = jawohl.Stream().with_max_depth(3)
try:
    shallow.push(b"[[[[[1]]]]]")
    print("  FAIL depth limit was not applied"); fails += 1
except ValueError:
    print("  ok   depth limit applied through the builder")
chained = jawohl.Stream().with_max_depth(64)
check("chain returns the same handle", chained.with_max_depth(64) is chained, True)
# The profile builder takes an enum, and only bites with a schema attached.
exact = jawohl.Stream.from_json_schema('{"type":"integer","maximum":100}')
exact = exact.with_number_profile(jawohl.NumberProfile.Exact)
exact.push(b"1000")
# Under Exact, `1000` may still become `1000e-9`, so no bound is decided yet.
check("Exact defers the bound", exact.validation(""), jawohl.Validation.Pending)
plain = jawohl.Stream.from_json_schema('{"type":"integer","maximum":100}')
plain.push(b"1000")
check("PlainDecimal decides it", plain.is_irrecoverable(), True)

print("two streams are independent")
a, b = jawohl.Stream(), jawohl.Stream()
a.push(b'{"n":1}')
check("a is complete", a.is_document_complete(), True)
check("b is untouched", b.is_document_complete(), False)

print("the stability guarantee, from Python")
def status_of(text, pointer):
    st = jawohl.Stream()
    st.push(text.encode())
    return st.status(pointer)
check("number undelimited", status_of(r'{"n":10', "/n"), jawohl.Syntax.Incomplete)
check("number delimited",   status_of(r'{"n":10}', "/n"), jawohl.Syntax.Complete)
check("absent",             status_of(r'{"q":"x"', "/nope"), jawohl.Syntax.Missing)

print("statuses are a real enum, not a magic string")
check("is a Syntax", type(status_of('{"a":1}', "/a")).__name__, "Syntax")
try:
    check("an enum is not its name", status_of('{"a":1}', "/a") == "Complete", False)
except TypeError:
    print("  ok   an enum does not compare equal to a string")

print("validation, decided as the bytes arrive")
v = jawohl.Stream.from_json_schema(SCHEMA)
v.push(b'{"role":"us')
check("live prefix", v.validation("/role"), jawohl.Validation.Pending)
check("keep going",  v.is_irrecoverable(), False)
v.push(b'er","limit":5')
check("still fine",  v.is_irrecoverable(), False)
v.push(b"0}")
check("valid", v.validation(""), jawohl.Validation.Valid)

print("the answer worth crossing a language boundary for")
def dead_at(chunks):
    st = jawohl.Stream.from_json_schema(SCHEMA)
    for i, c in enumerate(chunks):
        st.push(c.encode())
        if st.is_irrecoverable():
            return i
    return None
# Cancellation lands on the chunk that decided it, not at end of document.
check("bad enum, chunk 1", dead_at([r'{"role":"', r'sup', r'er"}']), 1)
check("bad bound, chunk 1", dead_at([r'{"role":"user","limit":', r'1000', r"}"]), 1)
check("good input never dies", dead_at([r'{"role":"user","limit":', r"50", r"}"]), None)

print("a bad schema raises rather than silently validating nothing")
try:
    jawohl.Stream.from_json_schema("{ not json")
    print("  FAIL should have raised"); fails += 1
except ValueError:
    print("  ok   bad schema raises")

print("doc comments arrived as docstrings")
check("on a function", (jawohl.complete_json.__doc__ or "").strip().splitlines()[0],
      "Complete a truncated JSON document so that it parses.")
check("on a method", (jawohl.Stream.push.__doc__ or "").strip().splitlines()[0],
      "Feed the next chunk.")

if fails:
    print(f"\n{fails} failure(s)"); sys.exit(1)
print("\nall checks passed")
