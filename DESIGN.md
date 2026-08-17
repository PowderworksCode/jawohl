# jawohl 2.0 — design

> A cross-language incremental parser and validator for streaming structured data.
> Parse and validate LLM structured output while it is still being generated.

**Status:** design; no 2.0 code yet. 1.0 still ships from `src/lib.rs`.
**Binding strategy:** jawohl 2.0 writes **no bindings by hand**. It is a full
[jedem][] consumer from day one — jedem is the sibling project that projects a
Rust crate's functions into other languages, and §8 is built around that
dependency and its cost.
**Scope:** the full 2.0 architecture, sequenced into phases that ship standalone.

[jedem]: https://github.com/PowderworksCode/jedem

Confidence marks: **[verified]** = run in this session; **[speculation]** = my
design reasoning, unproven.

---

## 1. What changes, and why 1.0 can't be polished into it

1.0 is 58 lines: count unclosed `{`/`[`/`"`, append the closers. It builds clean,
passes its 2 tests, and has zero dependencies [verified].

It is also **wrong on most partial input, silently.** Ten representative partial
documents through `complete_json`, checking whether the result parses as JSON
[verified — run against the crate this session]:

| input | 1.0 output | parses |
|---|---|---|
| `{"a": tru` | `{"a": tru}` | **no** — partial keyword |
| `{"a": "x\` | `{"a": "x\"}` | **no** — the trailing `\` escapes the quote it just appended |
| `{"a": "x\u00` | `{"a": "x\u00"}` | **no** — truncated `\u` escape |
| `{"a":` | `{"a":}` | **no** — key with no value |
| `{"a":1,` | `{"a":1,}` | **no** — trailing comma |
| `{"que` | `{"que"}` | **no** — partial key becomes a valueless member |
| `{"limit": 10` | `{"limit": 10}` | yes — **but see §3.2**, `10` may become `100` |
| `{"query":"rust par` | `{"query":"rust par"}` | yes |
| `{"k":"v","arr":[1,2,{"n":"v` | `…"v"}]}` | yes |
| `{"a": "he said \"hi` | `…\"hi"}` | yes |

Six of ten emit invalid JSON, and **all six return `Ok`**. 1.0 has no notion of
failure short of a bracket mismatch, so it cannot report that it produced
garbage. The README is honest that it "will not fix all partial json strings,"
but the failure mode is silent corruption, not a declined result.

That is a **structural** limit, not a bug list. A closer-counter has no model of
where it is in the grammar, so it cannot know that `tru` needs `e`, that `,`
needs a following member, or that a value is unfinished. Every one of those needs
a real incremental parser — which is 2.0's core. And the `{"limit": 10}` row is
the deeper point: it is *syntactically* fine and *semantically* unstable, which
no amount of closer-counting can detect.

**What carries over:** the idea, the name, the MIT license, and `complete_json`'s
signature — which 2.0 keeps as a derived convenience (§7) so 1.0 callers are not
broken.

---

## 2. Architecture

The owner's diagram, with the one addition that matters for sequencing — a hard
line between what needs jedem and what does not:

```
  Pydantic / Zod / Valibot / DataAnnotations / Jakarta / garde
                          │
                          ▼   (host-language adapters — hand-written, thin)
                    JSON Schema
                          │
════════════════ jedem boundary ════════════════
                          ▼
                     jawohl-core  (Rust)
        ┌─────────────────┼─────────────────┐
        ▼                 ▼                 ▼
  incremental       constraint IR      event log
    parser        + incr. validation
        └─────────────────┼─────────────────┘
                          ▼
                    partial state
```

Two independent tracks, and keeping them separate is what makes the jedem
coupling survivable (§8):

- **Core track — pure Rust, blocked by nothing.** Parser, partial state,
  constraint IR, incremental validation, events. This is all of the hard
  engineering and none of the FFI.
- **Surface track — gated on jedem.** Every language surface, in jedem's
  capability order.

**JSON Schema is the interchange format, not the execution format.** The core
compiles it to a constraint IR shaped for streaming evaluation (§4). The host
adapters (Pydantic → JSON Schema, Zod → JSON Schema) are **hand-written per
language and sit above jedem's generated surface** [confirmed by the owner] —
host-language code that produces a string, not something jedem generates. This
is what keeps "bindings stay thin" true: the generated binding is thin; the
adapter is a separate, small, idiomatic library per ecosystem.

---

## 3. The incremental parser and partial state

### 3.1 Model

A push-driven, O(n) state machine over a byte stream. It never rescans: each byte
is consumed once, updating an explicit stack of open containers plus the
in-progress token. Chunk boundaries are irrelevant — the machine is resumable at
any byte, including mid-escape-sequence and mid-`\uXXXX`.

State is addressed by **JSON Pointer** (`/query`, `/messages/0/role`). Every path
carries two orthogonal states:

```
syntax:     Missing | Incomplete | Complete
validation: Pending | ValidSoFar | Valid | Invalid | IrrecoverablyInvalid
```

`Missing` matters: it is how `required` is expressed before the object closes,
and it is distinct from a present-but-unfinished value.

### 3.2 The stability guarantee, and what it actually costs

> **Once a path is `Complete`, no future input can change its value.**

This is the load-bearing promise — it is what makes it safe to fire a search on
`ValueCompleted("/query", "rust parser")` while the model is still writing
`limit`. Making it *structurally* true, rather than usually true, drives three
non-obvious rules:

**Numbers are only `Complete` when delimited.** `{"limit": 10` — 1.0 happily
emits `10` [verified], but the next byte may be `0`, making it `100`. A number is
`Complete` only on `,`, `}`, `]`, whitespace, or end-of-document-with-explicit-finish.
**This is the single most common way a naive implementation breaks the
guarantee**, and it is invisible in testing because the output parses.

**Strings expose a stable prefix, not a stable value.** The decoded prefix grows
append-only, so it can be published as it arrives — but only **up to the last
complete escape**. `"x\` has stable prefix `x`; the `\` is pending. `"x\u00` has
stable prefix `x`; four hex digits are needed. A `ValueProgressed` payload
therefore carries the decoded-stable prefix, never the raw bytes.

**Keywords are `Complete` on their last byte.** `t`/`tr`/`tru` are `Incomplete`;
`true` is `Complete`, because no longer JSON token has `true` as a prefix. No
delimiter needed — unlike numbers. `tru` followed by anything but `e` is a syntax
error, which is exactly what 1.0 cannot detect.

**Containers** are `Complete` on their closing bracket.

### 3.3 Completion is irrevocable, including under later document failure

A consequence worth stating explicitly because it is a real trade, not a
detail: if `/query` completes and the document *later* turns out malformed
(`{"query":"x"]`), `/query` stays `Complete`. The guarantee is about the value,
not the document.

That is the right default — the whole point is to start work early — but it means
**a consumer acting on `ValueCompleted` may have acted on a doomed tool call.**
The mitigation is in the consumer's hands and should be documented loudly: act on
`ValueCompleted` for latency, or wait for `DocumentCompleted` for atomicity.
jawohl exposes the choice; it does not make it.

---

## 4. Incremental validation

This is the part with the least prior art and the most design risk.

### 4.1 The organising question

For each constraint, given a **prefix** `P` of an eventual complete value `V`,
which of these can we prove *now*?

- **Already satisfied and cannot be unsatisfied** → `ValidSoFar`
- **Already violated and cannot be repaired** → `IrrecoverablyInvalid` (the
  valuable one — it enables early cancellation)
- **Neither** → `Pending`

The answer depends on how the constraint behaves under *extension* of the prefix.
Classifying the JSON Schema keyword set by that property is the core design work:

| Keyword | Early verdict possible? | Mechanism |
|---|---|---|
| `type` | **Irrecoverable, immediately** | the first non-space byte fixes the type; `[` under `type:"string"` is dead on arrival |
| `maxLength` | **Irrecoverable** | decoded length only grows |
| `minLength` | **ValidSoFar** once reached | length only grows, so satisfaction is permanent |
| `enum` / `const` | **Irrecoverable** | prefix-match against a **trie** of the members; no member with this prefix ⇒ dead |
| `pattern` | **Irrecoverable** | compile to a DFA; the prefix is dead iff it lands in a state with no path to an accepting state (`regex-automata` exposes this) |
| `maxItems` / `maxProperties` | **Irrecoverable** | counts only grow |
| `minItems` / `minProperties` | **ValidSoFar** once reached | counts only grow |
| `uniqueItems` | **Irrecoverable** on first duplicate | duplicates are permanent |
| `additionalProperties: false` | **Irrecoverable** when an unknown key *completes* | the key is final at its closing quote |
| `required` | **ValidSoFar** when all present; `Invalid` at `}` | absence is only decidable at close |
| `minimum` / `maximum` / `exclusive*` | **see §4.2 — not soundly, in general** | exponent notation |
| `multipleOf` | no | `1` may become `12` |
| `allOf` | **Irrecoverable** if any branch is | intersection |
| `anyOf` | **Irrecoverable** only if *all* branches are | union — requires per-branch state |
| `oneOf` | **Irrecoverable** if all branches dead, or ≥2 branches already `Valid` and closed | otherwise needs completion to count |
| `not` | inverted; early verdicts flip and are usually unavailable | — |

The headline, and it is not obvious going in: **string-domain constraints admit
sound early rejection; numeric bounds do not.**

### 4.2 The numeric-bounds problem — the sharpest issue in this design

The owner's motivating early-cancel example is `limit <= 100` against
`"limit": 1000`. Intuitively `1000 > 100` and we should cancel immediately.

**We cannot, soundly.** `1000` may still become `1000e-9`, which is `0.000001`.
And the `e` need not come next — `1000.5e-9` is legal too — so no amount of
lookahead makes it sound. Under exact analysis, the set of completions of any
numeric prefix has infimum ≈ 0 and supremum ≈ ∞, and **no numeric bound is ever
decidable before the value is delimited.** That silently deletes the feature.

The resolution is to make the assumption explicit and enforced rather than
implied:

```rust
enum NumberProfile {
    /// Sound for all JSON. Numeric bounds resolve only at delimitation.
    Exact,
    /// Assume plain decimal (no `e`/`E` exponent). Default.
    PlainDecimal,
}
```

Under `PlainDecimal`, a prefix's completions form a bounded interval — `1000` ⇒
`[1000, 1001)` — so `maximum: 100` is decidably irrecoverable and early
cancellation works as the owner's doc describes.

**The enforcement is what makes this honest:** if an `e` or `E` is ever observed
in `PlainDecimal` mode, the stream does **not** quietly re-widen and does not
pretend the earlier verdict was fine. It raises a hard, named profile-violation
error. Either the guarantee held, or the consumer is told it was broken. This is
the same principle as the owner's *"jawohl should never silently pretend
unsupported native semantics were preserved,"* applied to the parser.

Default `PlainDecimal`, because LLM tool-call arguments essentially never use
exponent notation and the feature is worth having. [speculation — the
justification is empirical and I have not measured it; the soundness analysis
above is exact.]

### 4.3 Aggregation and propagation

A path's validation state is the fold of its constraints: any
`IrrecoverablyInvalid` wins; else any `Invalid`; else `Pending` if any pending;
else `ValidSoFar`; `Valid` only when the path is also syntactically `Complete`.

Irrecoverability propagates **up** — a dead child kills its parent — except
through `anyOf` and `not`, where a dead branch is ordinary. When the *root*
becomes `IrrecoverablyInvalid`, the document cannot be completed validly and the
consumer can cancel generation. That is the early-cancellation payoff, and it
falls out of the propagation rule rather than needing its own machinery.

---

## 5. Events

The core emits an append-only log; consumers drain it. It is **not** a diff over
snapshots — that is the owner's stated requirement and it is also what keeps the
FFI cheap, since a snapshot per token would be the dominant cost.

```
ValueStarted(path, kind)
ValueProgressed(path, stable_prefix)      // strings only; see §3.2
ValueCompleted(path, value)               // the stability guarantee applies here
ValidationFailed(path, constraint, state) // ValidSoFar → Invalid | IrrecoverablyInvalid
ValidationCompleted(path, state)
DocumentCompleted(state)
```

**Ordering guarantees** (these need to be promised, or consumers cannot write
correct handlers):

1. A path's events are totally ordered.
2. A child's `ValueCompleted` precedes its parent's.
3. `ValidationFailed` for a path never precedes that path's `ValueStarted`.
4. `DocumentCompleted` is last, and is emitted exactly once.

**Events cross the FFI typed.** The event enum is a structured discriminated
union in every language — jedem's only union projection (jedem design §3); there
is no JSON-envelope mode to fall back to. A consumer never parses a string to
learn what happened.

**Errors are events, not exceptions.** `ValidationFailed` is a domain outcome —
the consumer is *supposed* to keep receiving events and decide whether to cancel.
This maps onto jedem's per-op stream error model, with jawohl setting it to
errors-as-events rather than the throwing default (jedem design §5.3). Only *parser*
failure — malformed input, or a §4.2 profile violation — terminates the stream.

---

## 6. Native validators and transformations

### 6.1 The split

Portable constraints compile to the IR and run in Rust, incrementally. Anything
that cannot — `check_database(value)`, cross-field rules, custom predicates —
stays in the host and runs there.

**Native validators run on `ValueCompleted` for their path**, never on a prefix.
A host predicate has no notion of "valid so far," and calling it on a partial
value would be both wrong and expensive.

This is exactly jedem's **value-returning, fallible callback** — the feature
fluessig explicitly rejects [verified, `fluessig/src/api.rs:585`] and jedem
adds (jedem design §5.1). It is safe here for the reason jedem's rule requires:
`push()` is synchronous and host-called, so the callback fires re-entrantly on
the host's own thread, inside that call. **jawohl must not offer a native
validator on an async or streaming op**, or it forfeits the guarantee.

### 6.2 The honesty contract

Every adapter reports what it actually lowered:

```
14 constraints compiled
 2 native validators retained
 1 transformation deferred
```

This is a **first-class API**, not a log line — `stream.lowering_report()` —
because it is the only way a user learns their `mode="before"` validator is not
running where they think. Silence here is the failure mode the owner's doc names,
and the report is the mechanism that prevents it.

### 6.3 Transformations

Kept strictly separate from validation, and run **after** the value completes and
after portable validation. `trim()`, `lowercase()`, `str → datetime`, `str → URL`.

The honest edge: Pydantic's `mode="before"` validators and coercions run *before*
validation and can change what is validated. jawohl cannot reproduce that
incrementally — the value must be complete before a host transform can run, but
the constraints were already evaluated against the untransformed prefix. Such
validators are **not lowerable** and must be reported as deferred (§6.2) rather
than silently reordered.

---

## 7. API surface

Three levels, as the owner specified — each a superset of the last.

**Level 1 — completion.** The 1.0 signature, kept for compatibility, now derived
from the real parser and therefore correct on the six cases 1.0 breaks (§1):

```rust
jawohl::complete_json(input) -> Result<String, ParseError>
```

**Level 2 — streaming parse.** No schema.

```rust
let mut s = jawohl::Stream::new();
s.push(chunk)?;
s.snapshot();          // the partial value
s.changes();           // drain the event log
s.status("/query");    // Missing | Incomplete | Complete
```

**Level 3 — streaming parse + validation.**

```rust
let mut s = jawohl::Stream::from_json_schema(schema)?;
s.push(chunk)?;
s.validation("/query");        // Pending | ValidSoFar | Valid | Invalid | IrrecoverablyInvalid
s.lowering_report();           // §6.2
```

Raw JSON Schema is always supported and is the substrate; `jawohl.Stream(MyPydanticModel)`
and `jawohl.stream(MyZodSchema)` are host-side sugar over it.

**In jedem terms** [mapped against the jedem design — jedem has no code yet]:
`Stream::new` is a **ctor** and `from_json_schema` a **factory**, both minting
the same handle; `push`/`snapshot`/`status` are synchronous **methods** on it;
`changes()` is a **stream op**; a native validator is a **value-returning
callback param**. jawohl exercises all three of
jedem's hard three — which is what makes it a good acid test and also what
makes §8 the risk.

---

## 8. Phasing

Two tracks. The core track is unblocked; the surface track is gated on jedem.

### Core track — pure Rust, starts immediately

| Phase | Delivers | Ships as |
|---|---|---|
| **C1** | Incremental parser, partial state, stability guarantee, `complete_json` rebuilt on it | a Rust crate that is already strictly better than 1.0 — the six broken cases fixed |
| **C2** | Event log + ordering guarantees; `snapshot`/`status`/`changes` | Level 2 complete |
| **C3** | JSON Schema → constraint IR; the §4.1 classification; trie + DFA machinery | — |
| **C4** | Incremental validation, propagation, early cancellation, `NumberProfile` | Level 3 complete |
| **C5** | Native-validator hook + lowering report | needs a callback, so it lands with S3 |

**C1 alone justifies the rewrite** and needs nothing from jedem.

### Surface track — gated, in jedem's capability order

| Phase | Needs from jedem | Delivers |
|---|---|---|
| **S1** | steps 1–2 (free fns → python, node) | `complete_json` in Python + TS — closes the README promise open since May 2023 |
| **S2** | step 3 (**handle mint**) | `Stream` with `push`/`snapshot`/`status` — the first genuinely useful surface |
| **S3** | steps 4–5 (**streams**, **value-returning callbacks**) | `changes()` + native validators |
| **S4** | step 6 (**.NET backend**) | .NET — technology unprescribed, whichever Rust→C# bindgen works — then the Pydantic/Zod/Valibot adapters |

### The cost of "full jedem consumer from day one", stated plainly

You chose this over staged hand-written bindings, so the consequence should be on
the record: **jawohl 2.0 has no Python, TypeScript or .NET surface until jedem
ships handle-minting on those backends.** Today handle-minting exists on node and
python only, and **.NET does not exist at all** [verified] — so S4, jawohl's
fourth target language, is gated behind an entire new jedem backend.

Two things make that survivable, and they are the reason I would still take this
bet:

1. **The core track absorbs the wait.** C1–C4 is the majority of the engineering
   and none of it is blocked. By the time jedem reaches step 3, jawohl should
   have a complete, tested Rust core waiting for a surface.
2. **jawohl is the acid test that pulls jedem forward.** `findings.md`'s
   method — author the complete real surface before freezing the IR — is what
   caught every gap in fluessig. jawohl's surface is small, precise, and hits all
   three hard cases, which makes it a far better forcing function than a demo
   crate.

The residual risk is real: **jawohl's ship date is now jedem's slowest
backend.** If that becomes unacceptable, the cheapest release valve is a
temporary hand-written pyo3 binding for S1 only (a dozen lines for two free
functions), thrown away when jedem step 1 lands. I have not built that in; it
is available if the schedule bites.

---

## 9. Risks and open questions

1. **`oneOf` / `anyOf` / `not` are decided at completion, not incrementally.**
   Judging them on a prefix needs live state for every branch, and branches
   nest. The shipped behaviour is the documented degradation: `Pending` until
   the value is whole, never a guess. `allOf` is an intersection and composes
   incrementally for free. Revisit if a consumer needs early rejection through
   a union — it would need a branch-count cap.
2. **`$ref` and recursion — resolved: supported.** Omitting it looked
   defensible on paper and is not: Pydantic emits `$defs` plus `$ref` for every
   nested model, as does `zod-to-json-schema`, so a schema-first consumer hits
   it immediately. The constraint IR is therefore an arena rather than a tree,
   since `{"$ref": "#"}` is a cycle; recursive and mutually recursive refs
   resolve by handing back the node under construction. Remote refs are refused
   rather than fetched — jawohl does no I/O.
3. **Which JSON Schema draft.** 2020-12 presumably. `unevaluatedProperties` is
   materially harder to evaluate incrementally than `additionalProperties` and is
   a candidate for explicit non-support.
4. **`NumberProfile` default.** §4.2 defaults to `PlainDecimal` on an
   unmeasured empirical claim. Worth checking against real tool-call traffic
   before it becomes a compatibility commitment.
5. **Streaming input encoding.** Chunks arriving mid-UTF-8-sequence — the parser
   must be resumable at the byte level, not the `char` level. 1.0 iterates
   `input.chars()` and takes a `&str`, so it sidesteps this entirely; 2.0 cannot.
6. **The name.** Positioning moves from "JSON repair" to "incremental parser and
   validator." `complete_json` stays as one entry point among several, and the
   README's framing needs to lead with the new claim.
7. **Org — resolved.** jawohl is in `genauai`, not `PowderworksCode`
   [verified]. The owner is doing a **transfer**, not a fork, and is handling it
   directly. Nothing in this design touches `genauai/jawohl`.
8. **Only `complete_json` fits jedem's v1 boundary.** jedem v1 is functions
   over plain values — no callbacks, handles, or streams (jedem design §7). Of
   jawohl's surface only Level 1 qualifies; `Stream` is a handle mint, `changes()`
   is a stream, native validators are callbacks. With jawohl a full jedem
   consumer from day one, §8's surface track is therefore gated end to end on
   jedem steps 3–5. The core track (C1–C4) is unaffected and remains the right
   place to spend the wait.

---

## 10. Beyond JSON — a plan for other streamed formats

Design only; nothing below is scheduled, and none of it is 2.0.

### What actually generalises

jawohl's contribution is not "parse JSON incrementally" — streaming parsers
exist for every format listed here. It is a four-part contract on top of one:

1. **Path-addressed partial state** — `/messages/0/role` is `Incomplete`.
2. **A stability guarantee** — once `Complete`, the value cannot change.
3. **An event log** — what changed, not a snapshot to diff.
4. **Prefix validation** — this constraint is *already* decided.

Only (1) needs anything format-specific, and only the parser needs replacing.
The right shape is therefore a format-independent core — paths, states,
stability, events, the constraint IR and evaluator — with the tokenizer behind
a trait, and one crate per format above it. That split should happen *before*
the second format, not during it, or the JSON assumptions calcify.

### The finding that governs everything else

**The stability guarantee is weaker in every other format, and in YAML it
nearly collapses.**

JSON is unusually friendly: a string ends at its quote, a container at its
bracket, and only numbers need a following delimiter. That last case is already
the one that trips people up (§3.2). In most other formats, *almost every value*
is the number case:

| format | when is a scalar `Complete`? |
|---|---|
| JSON | quote / bracket; numbers need a delimiter |
| XML | at the closing tag — explicit and prompt |
| TOML | at end of line, unless a multi-line string or array is open |
| CSV | at the delimiter, unless the field is quoted |
| YAML | **often not until the *next* line's indentation arrives**, because a plain scalar may continue across lines and a block ends only at dedent |
| HTML | never quite: implied closes and error recovery make "complete" a judgement call |

A consumer's whole reason to use jawohl is acting early on values that will not
change. If a format cannot say `Complete` until much later, jawohl delivers far
less there — and the honest response is to say so per format rather than to
present one guarantee and quietly weaken it.

### Format by format

**SSE framing + JSONL — do this first.** Neither is a data format; both are
framing around documents jawohl already handles. SSE is *how every LLM API
actually delivers* the JSON jawohl exists to parse, so today a caller must strip
`data:` lines themselves before feeding us. JSONL is a document sequence.
Together they are a small amount of work for the largest share of real use, and
they need no new stability analysis at all. **Highest value-to-cost by a wide
margin.**

**XML and tag-structured output — do this second.** Well-formedness is strict
and closing tags are explicit, so stability is nearly as good as JSON: an
element completes at its close tag, text at the next `<`. Real relevance —
several tool-call conventions are tag-flavoured rather than JSON. Validation has
no clean analogue (XSD is heavy and nobody emits it for LLM output), so the
first version would be parse-and-partial-state only, with the constraint layer
left off rather than faked.

**Markdown — high demand, and the one I would now decline.** Revised after
surveying the field, which turns out to be crowded and to have arrived at
jawohl's own model without us.

`flux-md-core` and `brookmd-core` describe themselves as "incremental,
streaming-aware markdown parser with speculative closure", and their contract is
ours almost word for word: *committed blocks never change*, each `append` returns
a `Patch` of what moved, blocks carry stable monotonic IDs. `mdstream` advertises
"committed + pending blocks". `mdstitch` "closes unterminated syntax
token-by-token" — which is `complete_json` for markdown. On the JS side Vercel's
Streamdown does the same repair via its `remend` preprocessor, and semidown,
solid-streaming-markdown and llmrender all occupy the same ground.

That convergence is worth taking as **evidence the four-part contract is the
right decomposition** — several teams reached it independently. It is also the
reason not to enter: the two parts jawohl would bring are already there, and the
part that makes jawohl distinct — prefix *validation* — has no analogue, because
there is nothing to validate in prose. We would arrive late with no
differentiator.

Two transferable findings from that survey, both of which apply to formats we
*do* take:

- **Repair has a priority order, and it is empirical, not structural.** The
  field converges on: unclosed code fences first (most visually jarring), then
  bold/italic markers, inline code, links, math. Worth remembering when
  `complete_json`'s behaviour is ever tuned — some repairs matter far more to a
  reader than others.
- **Repair must be context-aware, and the failure is subtle.** A `**` inside a
  fenced code block is Python exponentiation, not emphasis; closing it corrupts
  the code. This is the same class as jawohl's escape-state tracking, where a
  trailing `\` must not be treated as closing a string — and evidence that the
  rule generalises past JSON.

One architectural note in our favour, if the decision is ever revisited. Most of
the JS field does **repair-then-reparse-the-whole-string** rather than true
incremental parsing, which is quadratic over a stream and causes a documented
class of bug: `marked`'s lexer only emits a code token once the closing fence
arrives, so an unclosed fence is classified as a *paragraph* and renders as prose
until it suddenly flips. That is precisely jawohl 1.0's failure — a parser built
for complete documents misclassifying a prefix — and precisely what a real state
machine avoids. Better architecture in a market that has not asked for it.

**YAML — hardest, and the guarantee suffers.** Beyond the completeness problem
above, anchors and aliases (`&a` / `*a`) mean a value can reference another, so
path-addressing meets a graph rather than a tree, and multi-document streams
(`---`) need the JSONL sequencing story first. YAML is also a JSON superset, so
a shared core is plausible. My read: **defer until something demands it**, and be
explicit that its `Complete` set is much smaller.

**TOML and CSV — cheap, low demand.** Both are line-oriented with tractable
stability rules; CSV's field-ends-at-delimiter rule is *exactly* JSON's number
rule, and column types give validation something real to check. Neither is a
common LLM output format. Do them if a data-pipeline consumer appears.

**HTML — defer.** Error recovery and implied close tags make "complete" a
judgement call rather than a guarantee, which is the one thing jawohl sells.
`lol_html` already covers streaming rewriting well.

### Recommended order

1. **SSE + JSONL framing** — near-free, and it is how LLM JSON actually arrives.
2. **Extract the format-independent core**, before a second grammar exists.
3. **XML / tag-structured** — the first genuinely different grammar.
4. Reassess. **YAML** only on demand; **Markdown deliberately declined** — see
   above, the field arrived at our model first and the half we would add does
   not exist there.

### What would make this a mistake

Shipping a second format before extracting the core, so JSON's assumptions
harden into the shared layer. Or presenting one stability guarantee across
formats when the table above says it differs — that would trade the single
property jawohl is trusted for against breadth nobody asked for.
