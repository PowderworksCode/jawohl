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

## 10. Scope: JSON and Markdown, in every language jedem reaches

Design only; nothing below is scheduled. Earlier drafts of this section surveyed
seven formats. The scope is now **two**, and the rest are declined with reasons.

### Why these two, and why the value is reach

The techniques here are known and several teams have them. What almost nobody has
is **the same capability in the language they work in**:

| | streaming markdown | partial JSON |
|---|---|---|
| JavaScript / TypeScript | a dozen | several |
| Rust | several | jiter's partial mode |
| Python | one, over markdown-it-py | `partial-json-parser` |
| Java, PHP, C#, Ruby, Go, Swift | **blog posts about approaches** | essentially nothing |

That is the whole opportunity. A Python developer building a chat UI has one
option; a Java, PHP or C# developer has none and writes the repair logic by hand.
jawohl plus jedem is one Rust core and a binding in each of those languages,
which is worth more than any single technique above.

JSON and Markdown are also simply **what models emit.** Tool calls and structured
output are JSON; everything a model says to a human is Markdown. Between them
they cover essentially all streamed model output that anyone renders or acts on.

**This plan is bounded by jedem.** Each format is worth only as many languages as
jedem delivers it to; shipped Rust-only, it has declined its own reason for
existing.

### Why not the others

- **HTML — declined on security** (see below), and its implied closes make
  "complete" a judgement call rather than a guarantee.
- **YAML** — validation would come free (its schema story *is* JSON Schema), but
  the stability guarantee nearly collapses: a plain scalar may continue onto the
  next line and a block ends only at dedent, so almost nothing completes
  promptly. Revisit on demand.
- **XML** — technically the best fit of any format, since XSD's Unique Particle
  Attribution makes content models deterministic regexes over children, so
  prefix-liveness is exact and needs no anchoring caveat. But XSD lives where
  regulation mandates it, no model emits one, and it is absent from the
  LLM-native path. Best fit, fewest users.
- **TOML, CSV** — cheap and both validate with JSON Schema or Table Schema, but
  neither is a common model output format.

### Security is why HTML is out — and Markdown is not automatically safe

Streaming model output into a renderer is an injection surface. The output is
untrusted text; HTML is a language for making a browser do things. Generating it
from model output means prompt injection and XSS are not edge cases but the
expected traffic. jawohl will not build the thing that makes that easy.

**Three consequences follow for Markdown, and they are design constraints, not
caveats:**

**1. Markdown is not HTML-free.** CommonMark permits raw HTML blocks and inline
HTML, so a naive Markdown pipeline is an HTML pipeline with extra steps. Every
serious streaming renderer in the field sanitizes downstream for exactly this
reason. jawohl's position: **raw HTML is surfaced as its own event kind, never
folded silently into text.** A consumer must decide about it explicitly; the
default must not be "it looked like prose, so we passed it through."

**2. Speculative closure can synthesise syntax that was never in the input.**
This hazard is specific to what jawohl does and has no analogue in batch parsing.
Repairing a truncated construct means *writing markup the model never emitted* —
and a partial `<a href="javascript:` or a half-arrived attribute could be
"completed" into something executable. The rule is therefore: **repair may close
constructs that are inert, and must never close an HTML construct.** An unclosed
code fence gets its fence; an unclosed tag gets nothing but an event saying so.

**3. The stability guarantee has a security reading.** Acting on a value that is
`Complete` is safer than acting on a prefix, because a prefix can still change
meaning — `"http` may become `"https://…"` or `"javascript:…"`. This is a second
argument for the guarantee jawohl already provides, and the reason
`ValueCompleted` rather than `ValueProgressed` should be the documented point at
which a consumer acts on anything that will reach a renderer or a tool.

Sanitisation itself stays the consumer's job — jawohl emits structure, it does
not render — but jawohl must not make that job harder by inventing syntax, and
must not hide the parts that need sanitising.

### What Markdown inherits, and what it does not

Three of the four contract parts apply. Prose has nothing to validate, so C3 and
C4 sit out — **except at the frontmatter**, which is YAML or TOML and therefore
validates with JSON Schema. Validating a document's frontmatter while its body
streams falls out for free, and is a real static-site and CMS case.

The stability rules differ from JSON's and are the substance of the work:

| construct | `Complete` when |
|---|---|
| fenced code | the closing fence arrives |
| paragraph | a blank line — and not before the *next* line rules out a setext heading |
| list item | the next item, a dedent, or a blank line |
| table row | the newline, unless a cell is still open |
| inline emphasis | the closing run, which may never arrive |
| link | the closing `)`; a half-written `[text](htt` is not a link yet |

Two findings from the field, both worth adopting:

- **Repair has a priority order, and it is empirical.** The convergent order is
  unclosed code fences first (most visually jarring), then emphasis, inline code,
  links, math. Some repairs matter far more to a reader than others.
- **Repair must be context-aware.** A `**` inside a fenced code block is Python
  exponentiation, not emphasis; closing it corrupts the code. Same class as
  jawohl's escape-state tracking, where a trailing `\` must not be read as
  closing a string.

And one architectural note in our favour: most of the JS field does
**repair-then-reparse-the-whole-string** rather than true incremental parsing,
which is quadratic over a stream and produces a documented bug class — `marked`'s
lexer emits a code token only once the closing fence arrives, so an unclosed
fence is classified as a *paragraph* and renders as prose until it suddenly
flips. That is exactly jawohl 1.0's failure, and exactly what a real state
machine avoids.

### Why the validation half is worth keeping

JSON Schema is not one option among several; it is the schema layer. OpenAPI 3.1
adopted it outright, and Anthropic, OpenAI and Google now all enforce it at the
*sampling* level with grammar-constrained decoding. It is the interface to the
model, not a thing you check afterwards.

Which raises the argument that most justifies C3 and C4: the reliability question
has moved from *"will the model emit valid JSON?"* to **"which slice of JSON
Schema does this provider actually honour?"** Anthropic drops numeric bounds,
OpenAI bans unions and imposes ceilings, Gemini's limits are undocumented. The
same schema can be enforced on one provider and **silently weakened** on another,
with no error and no signal.

That is the failure mode jawohl exists to refuse. A constraint the provider
quietly ignored is a constraint nobody checked, unless the consumer checks it
themselves as output arrives. jawohl's validation is not a duplicate of
provider-side constraint decoding — it is the **backstop for the parts the
provider dropped**, and the lowering report is where a caller learns which
constraints are actually live.

### The plan

1. **SSE + JSONL framing.** Neither is a data format; both are framing around
   JSON jawohl already handles, and SSE is how every provider actually delivers
   it — today every caller strips `data:` lines by hand. No new grammar, no new
   stability analysis, and it lands in every jedem language at once.
2. **Extract the format-independent core** — paths, states, stability, events,
   constraint IR, evaluator — with the tokenizer behind a trait. Before the
   second grammar exists, not during it.
3. **Markdown**, under the constraints above: raw HTML as its own event, repair
   that never closes an HTML construct, and the stability table as the spec.
4. **Breadth over depth from there.** A third format is worth less than the
   second and third *language*, because reach is the value.

### What would make this a mistake

Shipping a second format before extracting the core, so JSON's assumptions harden
into the shared layer. Presenting one stability guarantee across formats when the
table above says it differs. Repairing HTML constructs, or letting raw HTML reach
a consumer without saying so. Or shipping either format Rust-only — which would
be doing the work and declining the reason for it.
