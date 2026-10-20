// The Native AI's GPU kernels (phase G1): weight matrices in their stored
// layout (f32, or q8/q4/t2 blocks — see quant.rs), dequantized in place,
// times f32 activations, with f32 sums.
//
//   matmul:   y[t, r]  = Σ_c W[r, c] · x[t, c]
//   matmul_t: dx[t, c] = Σ_r W[r, c] · dy[t, r]

struct Params {
    rows: u32,
    cols: u32,
    t_len: u32,
    row_words: u32,
    // 0 f32, 1 q8, 2 q4, 3 t2
    kind: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> w: array<u32>;
@group(0) @binding(2) var<storage, read> x: array<f32>;
@group(0) @binding(3) var<storage, read_write> y: array<f32>;

// Positions per invocation: each weight is decoded once for all of them.
const TT: u32 = 8u;

// Words in a block of 32 values (an f32 "block" has no scale).
fn block_words() -> u32 {
    switch p.kind {
        case 1u: { return 9u; }
        case 2u: { return 5u; }
        case 3u: { return 3u; }
        default: { return 32u; }
    }
}

// Value `j` (0..32) of the block starting at word `base`.
fn weight(base: u32, j: u32) -> f32 {
    switch p.kind {
        case 1u: {
            let q = extractBits(bitcast<i32>(w[base + 1u + j / 4u]), (j % 4u) * 8u, 8u);
            return bitcast<f32>(w[base]) * f32(q);
        }
        case 2u: {
            let n = extractBits(w[base + 1u + j / 8u], (j % 8u) * 4u, 4u);
            return bitcast<f32>(w[base]) * (f32(n) - 8.0);
        }
        case 3u: {
            let c = extractBits(w[base + 1u + j / 16u], (j % 16u) * 2u, 2u);
            var v = 0.0;
            if c == 1u { v = 1.0; } else if c == 2u { v = -1.0; }
            return bitcast<f32>(w[base]) * v;
        }
        default: {
            return bitcast<f32>(w[base + j]);
        }
    }
}

// One invocation per weight row, TT positions each.
@compute @workgroup_size(64)
fn matmul(@builtin(global_invocation_id) id: vec3<u32>) {
    let r = id.x;
    let t0 = id.y * TT;
    if r >= p.rows { return; }
    let bw = block_words();
    let blocks = p.cols / 32u;
    var acc: array<f32, TT>;
    var wv: array<f32, 32>;
    for (var b = 0u; b < blocks; b++) {
        let base = r * p.row_words + b * bw;
        for (var j = 0u; j < 32u; j++) {
            wv[j] = weight(base, j);
        }
        for (var t = 0u; t < TT; t++) {
            if t0 + t >= p.t_len { break; }
            let xo = (t0 + t) * p.cols + b * 32u;
            var s = 0.0;
            for (var j = 0u; j < 32u; j++) {
                s += wv[j] * x[xo + j];
            }
            acc[t] += s;
        }
    }
    for (var t = 0u; t < TT; t++) {
        if t0 + t >= p.t_len { break; }
        y[(t0 + t) * p.rows + r] = acc[t];
    }
}

// One workgroup per block of 32 columns: each invocation owns a column and
// walks every row (the 32 invocations read the same block words).
@compute @workgroup_size(32)
fn matmul_t(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let b = wg.x;
    let j = li.x;
    let t0 = wg.y * TT;
    let bw = block_words();
    var acc: array<f32, TT>;
    for (var r = 0u; r < p.rows; r++) {
        let v = weight(r * p.row_words + b * bw, j);
        for (var t = 0u; t < TT; t++) {
            if t0 + t >= p.t_len { break; }
            acc[t] += v * x[(t0 + t) * p.rows + r];
        }
    }
    let c = b * 32u + j;
    for (var t = 0u; t < TT; t++) {
        if t0 + t >= p.t_len { break; }
        y[(t0 + t) * p.cols + c] = acc[t];
    }
}
