"""Reference implementation of the Native AI model (ADR-0027, phase M1).

This is the answer key the engine is checked against, not part of the app.
It is written separately from the Rust engine, in float64 with numpy, from
the same published ideas (see the sources table in docs/native-ai.md).

    python3 training/reference/nativa_ref.py packages/engine/tests/fixtures

writes `tiny.nativa` (a small model with random weights) and
`tiny.expected.json` (token ids for a few texts and the logits at every
position of one prompt). The Rust test `tests/reference.rs` loads both and
requires the engine to match.
"""

import json
import struct
import sys
from pathlib import Path

import numpy as np

MAGIC = b"NATV"
FORMAT_VERSION = 1
ALIGN = 64


# ---------------------------------------------------------------- format


def align_up(n):
    return (n + ALIGN - 1) // ALIGN * ALIGN


def write_nativa(path, config, tokenizer, metadata, tensors):
    """tensors: list of (name, ndarray). Stored as little-endian float32."""
    index, blobs, offset = [], [], 0
    for name, array in tensors:
        data = np.ascontiguousarray(array, dtype="<f4").tobytes()
        index.append(
            {"name": name, "dtype": "f32", "shape": list(array.shape), "offset": offset}
        )
        blobs.append(data + b"\0" * (align_up(len(data)) - len(data)))
        offset += align_up(len(data))
    header = json.dumps(
        {"config": config, "tokenizer": tokenizer, "metadata": metadata, "tensors": index},
        separators=(",", ":"),
    ).encode()
    with open(path, "wb") as f:
        f.write(MAGIC)
        f.write(struct.pack("<IQ", FORMAT_VERSION, len(header)))
        f.write(header)
        end = 16 + len(header)
        f.write(b"\0" * (align_up(end) - end))
        for blob in blobs:
            f.write(blob)


# ------------------------------------------------------------- tokenizer


class Tokenizer:
    def __init__(self, merges, special):
        self.merges = [tuple(m) for m in merges]
        self.special = list(special)
        self.ranks = {pair: k for k, pair in enumerate(self.merges)}
        self.base = 256 + len(self.merges)

    def encode(self, text):
        out = []
        while text:
            hits = [(text.find(s), -len(s), j) for j, s in enumerate(self.special) if s in text]
            if not hits:
                out += self._bpe(text)
                break
            at, neg_len, j = min(hits)
            out += self._bpe(text[:at])
            out.append(self.base + j)
            text = text[at - neg_len :]
        return out

    def _bpe(self, text):
        ids = list(text.encode("utf-8"))
        while True:
            ranked = [self.ranks[p] for p in zip(ids, ids[1:]) if p in self.ranks]
            if not ranked:
                return ids
            rank = min(ranked)
            a, b = self.merges[rank]
            merged, i = [], 0
            while i < len(ids):
                if i + 1 < len(ids) and ids[i] == a and ids[i + 1] == b:
                    merged.append(256 + rank)
                    i += 2
                else:
                    merged.append(ids[i])
                    i += 1
            ids = merged


# ----------------------------------------------------------------- model


def rmsnorm(x, w, eps):
    return x / np.sqrt(np.mean(x * x) + eps) * w


def rope(v, pos, head_dim, theta):
    """v: [n_heads, head_dim]; rotates pairs (2i, 2i+1)."""
    out = v.copy()
    i = np.arange(head_dim // 2)
    angle = pos * theta ** (-(2 * i) / head_dim)
    cos, sin = np.cos(angle), np.sin(angle)
    a, b = v[:, 0::2], v[:, 1::2]
    out[:, 0::2] = a * cos - b * sin
    out[:, 1::2] = a * sin + b * cos
    return out


def silu(x):
    return x / (1 + np.exp(-x))


def softmax(x):
    e = np.exp(x - x.max())
    return e / e.sum()


def forward(cfg, w, tokens):
    """Logits at every position, attending over the whole prefix at once."""
    dim, nh, nkv = cfg["dim"], cfg["n_heads"], cfg["n_kv_heads"]
    hd = dim // nh
    group = nh // nkv
    eps, theta = cfg["norm_eps"], cfg["rope_theta"]
    keys = [[] for _ in range(cfg["n_layers"])]
    values = [[] for _ in range(cfg["n_layers"])]
    all_logits = []
    for pos, tok in enumerate(tokens):
        x = w["tok_embeddings"][tok].copy()
        for l in range(cfg["n_layers"]):
            p = f"layers.{l}."
            h = rmsnorm(x, w[p + "attn_norm"], eps)
            q = rope((w[p + "wq"] @ h).reshape(nh, hd), pos, hd, theta)
            k = rope((w[p + "wk"] @ h).reshape(nkv, hd), pos, hd, theta)
            v = (w[p + "wv"] @ h).reshape(nkv, hd)
            keys[l].append(k)
            values[l].append(v)
            K, V = np.stack(keys[l]), np.stack(values[l])  # [pos+1, nkv, hd]
            heads = []
            for head in range(nh):
                g = head // group
                att = softmax(K[:, g, :] @ q[head] / np.sqrt(hd))
                heads.append(att @ V[:, g, :])
            x = x + w[p + "wo"] @ np.concatenate(heads)
            h = rmsnorm(x, w[p + "ffn_norm"], eps)
            x = x + w[p + "w2"] @ (silu(w[p + "w1"] @ h) * (w[p + "w3"] @ h))
        out = w.get("output", w["tok_embeddings"])
        all_logits.append(out @ rmsnorm(x, w["norm"], eps))
    return np.stack(all_logits)


# --------------------------------------------------------------- fixture


def make_fixture(out_dir):
    rng = np.random.default_rng(20261020)
    special = ["<|ferramenta|>", "<|resultado|>", "<|rotina|>", "<|fim|>"]
    # A handful of merges learned from nothing in particular: enough to
    # exercise multi-level merges in Portuguese and English.
    merges = []

    def merge(a, b):
        merges.append([a, b])
        return 255 + len(merges)

    o_ = merge(ord("o"), ord(" "))
    de = merge(ord("d"), ord("e"))
    de_ = merge(de, ord(" "))
    th = merge(ord("t"), ord("h"))
    the = merge(th, ord("e"))
    ao = merge(ord("a"), ord("o"))
    merge(*"ç".encode())  # the two UTF-8 bytes of "ç" as one id
    merge(ao, ord(" "))
    merge(the, ord(" "))
    merge(de_, o_)
    tok = Tokenizer(merges, special)

    cfg = {
        "vocab_size": 256 + len(merges) + len(special),
        "dim": 32,
        "n_layers": 2,
        "n_heads": 4,
        "n_kv_heads": 2,
        "hidden_dim": 64,
        "max_seq_len": 64,
        "rope_theta": 10000.0,
        "norm_eps": 1e-5,
    }
    dim, hidden, vocab = cfg["dim"], cfg["hidden_dim"], cfg["vocab_size"]
    kv = cfg["n_kv_heads"] * dim // cfg["n_heads"]

    def r(*shape, scale=0.2):
        return (rng.standard_normal(shape) * scale).astype(np.float32)

    tensors = [("tok_embeddings", r(vocab, dim, scale=1.0))]
    for l in range(cfg["n_layers"]):
        p = f"layers.{l}."
        tensors += [
            (p + "attn_norm", (1 + r(dim, scale=0.1))),
            (p + "wq", r(dim, dim)),
            (p + "wk", r(kv, dim)),
            (p + "wv", r(kv, dim)),
            (p + "wo", r(dim, dim)),
            (p + "ffn_norm", (1 + r(dim, scale=0.1))),
            (p + "w1", r(hidden, dim)),
            (p + "w2", r(dim, hidden)),
            (p + "w3", r(hidden, dim)),
        ]
    tensors += [("norm", 1 + r(dim, scale=0.1)), ("output", r(vocab, dim))]

    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    write_nativa(
        out_dir / "tiny.nativa",
        cfg,
        {"merges": merges, "special": special},
        {"name": "tiny-reference", "purpose": "engine tests (phase M1)"},
        tensors,
    )

    # The engine computes in float32; the key uses the same float32 weights
    # promoted to float64.
    w = {name: a.astype(np.float64) for name, a in tensors}
    prompt_text = "<|rotina|>the ação de o fim<|ferramenta|>"
    prompt = tok.encode(prompt_text)
    logits = forward(cfg, w, prompt)
    texts = [
        "the the", "de o de o", "ação", "olá, mundo", "こんにちは",
        "a<|resultado|>b<|fim|>", prompt_text, "",
    ]
    expected = {
        "encodings": [{"text": t, "ids": tok.encode(t)} for t in texts],
        "prompt": prompt,
        "logits": logits.round(7).tolist(),
    }
    (out_dir / "tiny.expected.json").write_text(json.dumps(expected, ensure_ascii=False))
    print(f"wrote {out_dir}/tiny.nativa and tiny.expected.json "
          f"({len(prompt)} prompt tokens, vocab {vocab})")


if __name__ == "__main__":
    make_fixture(sys.argv[1] if len(sys.argv) > 1 else "packages/engine/tests/fixtures")
