//! Differential test against CPython's unpickler. Regenerate the corpus with
//! `python3 -I scripts/gen-pickle-corpus.py`.

use replay_parser::pickle::Value;
use replay_parser::pickle::from_slice;
use serde_json::Value as J;
use serde_json::json;

/// The same canonical form the generator writes.
fn canon(v: &Value) -> J {
    match v {
        Value::None => J::Null,
        Value::Bool(b) => json!(b),
        Value::Int(i) => json!(i),
        Value::BigInt(b) => json!({ "bigint": hex(b) }),
        Value::Float(f) => json!({ "float": hex(&f.to_le_bytes()) }),
        Value::Bytes(b) => json!({ "bytes": hex(b) }),
        Value::Str(s) => json!(s),
        Value::List(l) => json!({ "list": l.iter().map(canon).collect::<Vec<_>>() }),
        Value::Tuple(t) => json!({ "tuple": t.iter().map(canon).collect::<Vec<_>>() }),
        Value::Dict(d) => json!({ "dict": d.iter().map(|(k, v)| json!([canon(k), canon(v)])).collect::<Vec<_>>() }),
        Value::Set(s) => json!({ "set": s.iter().map(canon).collect::<Vec<_>>() }),
        Value::Global { module, name } => json!({ "global": format!("{module}.{name}") }),
        Value::Object { callable, args, state } => json!({
            "object": canon(callable),
            "args": canon(args),
            "state": state.as_deref().map_or(J::Null, canon),
        }),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn matches_cpython_on_every_case() {
    let corpus: Vec<J> = serde_json::from_str(include_str!("pickle_corpus.json")).unwrap();
    let mut failures = Vec::new();
    for case in &corpus {
        let name = case["name"].as_str().unwrap();
        let data = unhex(case["hex"].as_str().unwrap());
        match from_slice(&data) {
            Ok(v) if canon(&v) == case["expected"] => {}
            Ok(v) => failures.push(format!("{name}: got {}, want {}", canon(&v), case["expected"])),
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ:\n{}", failures.len(), corpus.len(), failures.join("\n"));
}

#[test]
fn truncations_never_panic() {
    let corpus: Vec<J> = serde_json::from_str(include_str!("pickle_corpus.json")).unwrap();
    for case in corpus.iter().step_by(7) {
        let data = unhex(case["hex"].as_str().unwrap());
        for cut in 0..data.len() {
            let _ = from_slice(&data[..cut]);
        }
    }
}
