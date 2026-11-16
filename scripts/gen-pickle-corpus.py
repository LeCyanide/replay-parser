#!/usr/bin/env python3
"""Generates tests/pickle_corpus.json: pickles plus the value tree the Rust
reader must produce for each.

The expected tree comes from CPython's own Unpickler with `find_class`
replaced by recorders, so no game or test class is ever imported or run, and
with fix_imports off so module names stay as written (Python 2 pickles say
`copy_reg`, not `copyreg`). Run with -I from the repository root:

    python3 -I scripts/gen-pickle-corpus.py
"""
import io, json, pickle, struct, sys

class Rec:
    """Stands in for any class or function a pickle names."""
    def __init__(self, module, name):
        self.module, self.name = module, name
    def __call__(self, *args):
        return Obj(self, args)

def make_class(module, name):
    rec = Rec(module, name)
    def new(cls, *args):
        o = object.__new__(Inst)
        o.obj = Obj(rec, args)
        return o
    Inst = type(name, (), {"__new__": new, "__setstate__": lambda self, st: setattr(self.obj, "state", st)})
    Inst._rec = rec
    return Inst

class Obj:
    def __init__(self, callable_, args):
        self.callable, self.args, self.state = callable_, args, None
    def __setstate__(self, st):
        self.state = st

class Recorder(pickle.Unpickler):
    def find_class(self, module, name):
        return make_class(module, name)

def canon(v):
    if v is None or isinstance(v, bool): return v
    if isinstance(v, int):
        # Minimal little-endian two's complement, as pickle's LONG1 writes it.
        return v if -2**63 <= v < 2**63 else {"bigint": pickle.encode_long(v).hex()}
    if isinstance(v, float): return {"float": struct.pack("<d", v).hex()}
    if isinstance(v, (bytes, bytearray)): return {"bytes": bytes(v).hex()}
    if isinstance(v, str): return v
    if isinstance(v, list): return {"list": [canon(x) for x in v]}
    if isinstance(v, tuple): return {"tuple": [canon(x) for x in v]}
    if isinstance(v, dict): return {"dict": [[canon(k), canon(x)] for k, x in v.items()]}
    if isinstance(v, (set, frozenset)): return {"set": [canon(x) for x in v]}
    if isinstance(v, type) and hasattr(v, "_rec"): return {"global": f"{v._rec.module}.{v._rec.name}"}
    if isinstance(v, Rec): return {"global": f"{v.module}.{v.name}"}
    if hasattr(v, "obj"): v = v.obj
    if isinstance(v, Obj):
        return {"object": canon(v.callable), "args": canon(tuple(v.args)), "state": canon(v.state)}
    raise TypeError(type(v))

def expected(data):
    return canon(Recorder(io.BytesIO(data), fix_imports=False, encoding="bytes").load())

# Module-level classes so they pickle by reference.
class Ship:
    def __init__(self):
        self.name, self.tier, self.guns = "PJSB018", 10, [{"caliber": 0.46, "count": 9}]
class Slotted:
    __slots__ = ("a", "b")
    def __init__(self): self.a, self.b = 1, "two"
class Reducer:
    def __reduce__(self): return (Reducer, (1, "x"), {"s": True})

shared = [1, 2]
values = [
    None, True, False, 0, 1, -1, 255, 256, 65535, 65536, -2**31, 2**31, 2**63 - 1, -2**63, 2**64, -2**80,
    0.0, -1.5, 1e300, float("inf"),
    "", "ascii", "ünïcødé", "line\nbreak", "\\u escape",
    b"", b"\x00\xffbytes",
    [], [1, [2, [3]]], (), (1,), (1, 2), (1, 2, 3), (1, 2, 3, 4),
    {}, {"a": 1, "b": {"c": [None]}}, {1: "int key", (1, 2): "tuple key"},
    {1, 2, 3}, frozenset({"x"}),
    [shared, shared, {"again": shared}],
    Ship(), [Ship(), Ship()], Slotted(), Reducer(),
    list(range(1001)), {f"k{i}": i for i in range(1001)},  # past the 1000-item batch size
]

cases = []
for proto in range(0, 6):
    for i, v in enumerate(values):
        if proto == 0 and isinstance(v, Slotted):
            continue  # protocol 0/1 cannot pickle __slots__ without __getstate__
        if proto < 2 and isinstance(v, Slotted):
            continue
        data = pickle.dumps(v, protocol=proto)
        cases.append({"name": f"p{proto}-{i}", "hex": data.hex(), "expected": expected(data)})

# Python 2 `str` opcodes, written by hand since Python 3 never emits them.
py2 = {
    "py2-STRING": b"S'it\\'s \\x41\\101'\np0\n.",
    "py2-BINSTRING": b"\x80\x02T\x05\x00\x00\x00hello.",
    "py2-SHORT_BINSTRING": b"\x80\x02U\x03abcq\x00.",
    # copy_reg._reconstructor object with a dict state, as GameParams writes them.
    "py2-reconstructor": b"ccopy_reg\n_reconstructor\np0\n(cGameParams\nGPData\np1\nc__builtin__\nobject\np2\nNtp3\nRp4\n(dp5\nS'id'\np6\nI4181604048\nsS'name'\np7\nS'PJSB018'\np8\nsb.",
    "py2-INT-bools": b"(I01\nI00\nI42\nL12345678901234567890L\nl.".replace(b"L12345678901234567890L\n", b"L1234567890123456789L\n"),
}
for name, data in py2.items():
    cases.append({"name": name, "hex": data.hex(), "expected": expected(data)})

with open("tests/pickle_corpus.json", "w") as f:
    json.dump(cases, f, separators=(",", ":"))
print(f"{len(cases)} cases", file=sys.stderr)
