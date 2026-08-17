"""Generate the realistic half of jawohl's test corpus.

Every escape sequence below is written with a doubled backslash, so this file
is pure ASCII while the documents it emits contain the two-character sequences
a JSON parser actually has to decode.
"""

import io
import json
import os

files = {}

files["tool_call.json"] = (
    '{"name":"search_web","arguments":{"query":"rust incremental json parser",'
    '"limit":10,"filters":{"lang":"en","after":"2024-01-01","site":null},'
    '"rerank":true}}'
)

files["chat_completion.json"] = (
    '{"id":"chatcmpl-9xQ2","object":"chat.completion","created":1730000000,'
    '"model":"claude-opus-5","choices":[{"index":0,"message":{"role":"assistant",'
    '"content":"Here is the answer.","tool_calls":[{"id":"call_1",'
    '"type":"function","function":{"name":"get_weather",'
    '"arguments":"{\\"city\\":\\"Zurich\\"}"}}]},"finish_reason":"tool_calls"}],'
    '"usage":{"prompt_tokens":42,"completion_tokens":17,"total_tokens":59}}'
)

# Literal multi-byte characters: 2-, 3- and 4-byte UTF-8 sequences, so the
# chunk-splitting tests have real boundaries to land inside.
files["unicode.json"] = (
    '{"ascii":"plain","latin":"café naïve","cjk":"日本語",'
    '"emoji":"\U0001f389\U0001f680","rtl":"مرحبا",'
    '"combining":"é","mixed":"a日\U0001f680z","empty":"","spaces":"  "}'
)

# Escape SEQUENCES as text -- the file contains backslash-u-0-0-0-0, not a NUL.
files["escapes.json"] = (
    '{"quote":"say \\"hi\\"","backslash":"a\\\\b","slash":"a\\/b",'
    '"control":"\\b\\f\\n\\r\\t","nul":"\\u0000","bom":"\\ufeff",'
    '"latin_esc":"\\u00e9","cjk_esc":"\\u4e2d","astral_pair":"\\ud83d\\ude00",'
    '"nested_quotes":"he said \\"she said \\\\\\"no\\\\\\"\\"",'
    '"json_in_string":"{\\"a\\":[1,2,{\\"b\\":null}]}"}'
)

files["numbers.json"] = (
    '{"zero":0,"neg_zero":-0,"int":42,"neg":-42,"big":9007199254740991,'
    '"frac":1.5,"neg_frac":-0.125,"exp":1e10,"exp_plus":1E+10,"exp_neg":1.5e-30,'
    '"tiny":0.0000001,"long":123456789012345678901234567890,'
    '"arr":[0,-1,1.5,1e10,-2.5E-3,0.0]}'
)

files["nested.json"] = (
    '{"a":{"b":{"c":{"d":{"e":{"f":{"g":{"h":[{"i":{"j":[1,[2,[3,[4,[5,'
    '{"k":"deep"}]]]]]}}]}}}}}}},"parallel":[[[[[1]]]],[[[[2]]]],[[[[3]]]]],'
    '"mixed":[{"x":[{"y":[{"z":{}}]}]}]}'
)

files["empties.json"] = (
    '{"obj":{},"arr":[],"str":"","nested_empty":{"a":{},"b":[],"c":[{}],'
    '"d":[[]],"e":{"f":{}}},"nulls":[null,null],"bools":[true,false],'
    '"single":[0]}'
)

files["messages.json"] = (
    '{"messages":[{"role":"system","content":"You are helpful."},'
    '{"role":"user","content":"What is 2+2?"},'
    '{"role":"assistant","content":"4"},{"role":"user","content":"Why?"},'
    '{"role":"assistant","content":"Because addition of two twos yields four.",'
    '"metadata":{"tokens":9,"cached":false,"logprobs":[-0.1,-2.3,-0.001]}}],'
    '"stream":true,"temperature":0.7,"max_tokens":1024,'
    '"stop":["\\n\\n","END"]}'
)

files["config.json"] = (
    '{"$schema":"https://json.schemastore.org/tsconfig","compilerOptions":'
    '{"target":"ES2022","module":"NodeNext","strict":true,"lib":["ES2022","DOM"],'
    '"paths":{"@app/*":["./src/*"],"@test/*":["./tests/*"]},"outDir":"./dist",'
    '"sourceMap":true,"noEmit":false},"include":["src/**/*.ts"],'
    '"exclude":["node_modules","**/*.spec.ts"],'
    '"references":[{"path":"./packages/core"},{"path":"./packages/cli"}]}'
)

rows = []
for i in range(40):
    rows.append(
        {
            "id": i,
            "name": "item-%d" % i,
            "tags": ["a", "b", "c"][: (i % 3) + 1],
            "score": round(i * 1.5 - 3, 3),
            "active": i % 2 == 0,
            "meta": None if i % 5 == 0 else {"note": "café", "rev": i},
        }
    )
files["records.json"] = json.dumps(
    {"records": rows, "total": len(rows), "cursor": None}, ensure_ascii=False
)

out_dir = "realworld"
for name in sorted(files):
    text = files[name]
    json.loads(text)  # the corpus must not lie about being valid
    io.open(os.path.join(out_dir, name), "w", encoding="utf-8", newline="").write(text)
    print("  ok  %-24s %6d bytes" % (name, len(text.encode("utf-8"))))

print(
    "total: %d bytes across %d files"
    % (sum(len(t.encode("utf-8")) for t in files.values()), len(files))
)
