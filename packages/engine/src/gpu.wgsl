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

// Tiled kernels: a workgroup of 256 invocations loads a tile of weights
// (dequantized once) and a tile of activations into workgroup memory; each
// invocation then reuses them for 4 outputs.

// Positions per tile, and per invocation.
const TP: u32 = 16u;
const PER: u32 = 4u;

// matmul: 64 rows × 16 positions per workgroup, one block of 32 columns at
// a time. Row stride 33 keeps the 64 rows in different memory banks.
var<workgroup> w_tile: array<f32, 2112>;  // 64 × 33
var<workgroup> x_tile: array<f32, 512>;   // 16 × 32

@compute @workgroup_size(256)
fn matmul(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let r0 = wg.x * 64u;
    let t0 = wg.y * TP;
    let bw = block_words();
    let blocks = p.cols / 32u;
    let my_r = li % 64u;
    let my_t = (li / 64u) * PER;
    var acc: array<f32, 4>;
    for (var b = 0u; b < blocks; b++) {
        // Weights: invocation li decodes 8 values of row li/4.
        let lr = li / 4u;
        let lj = (li % 4u) * 8u;
        let row = r0 + lr;
        for (var j = 0u; j < 8u; j++) {
            var v = 0.0;
            if row < p.rows {
                v = weight(row * p.row_words + b * bw, lj + j);
            }
            w_tile[lr * 33u + lj + j] = v;
        }
        // Activations: 2 values each.
        for (var e = 0u; e < 2u; e++) {
            let idx = li * 2u + e;
            let t = t0 + idx / 32u;
            var v = 0.0;
            if t < p.t_len {
                v = x[t * p.cols + b * 32u + idx % 32u];
            }
            x_tile[idx] = v;
        }
        workgroupBarrier();
        for (var j = 0u; j < 32u; j++) {
            let wv = w_tile[my_r * 33u + j];
            for (var k = 0u; k < PER; k++) {
                acc[k] += wv * x_tile[(my_t + k) * 32u + j];
            }
        }
        workgroupBarrier();
    }
    let r = r0 + my_r;
    if r < p.rows {
        for (var k = 0u; k < PER; k++) {
            let t = t0 + my_t + k;
            if t < p.t_len {
                y[t * p.rows + r] = acc[k];
            }
        }
    }
}

// matmul_t: 64 columns × 16 positions per workgroup, 32 rows at a time.
var<workgroup> wt_tile: array<f32, 2048>;  // 32 rows × 64 columns
var<workgroup> d_tile: array<f32, 512>;    // 16 positions × 32 rows

@compute @workgroup_size(256)
fn matmul_t(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let c0 = wg.x * 64u;
    let t0 = wg.y * TP;
    let bw = block_words();
    let my_c = li % 64u;
    let my_t = (li / 64u) * PER;
    var acc: array<f32, 4>;
    for (var r0 = 0u; r0 < p.rows; r0 += 32u) {
        // Weights: invocation li decodes 8 values: row li/8, columns
        // (li%8)*8 .. +8 of the tile (inside one block of 32).
        let lr = li / 8u;
        let lc = (li % 8u) * 8u;
        let row = r0 + lr;
        let c = c0 + lc;
        for (var j = 0u; j < 8u; j++) {
            var v = 0.0;
            if row < p.rows && c + j < p.cols {
                let cc = c + j;
                v = weight(row * p.row_words + (cc / 32u) * bw, cc % 32u);
            }
            wt_tile[lr * 64u + lc + j] = v;
        }
        for (var e = 0u; e < 2u; e++) {
            let idx = li * 2u + e;
            let t = t0 + idx / 32u;
            let rr = r0 + idx % 32u;
            var v = 0.0;
            if t < p.t_len && rr < p.rows {
                v = x[t * p.rows + rr];
            }
            d_tile[idx] = v;
        }
        workgroupBarrier();
        for (var j = 0u; j < 32u; j++) {
            let wv = wt_tile[j * 64u + my_c];
            for (var k = 0u; k < PER; k++) {
                acc[k] += wv * d_tile[(my_t + k) * 32u + j];
            }
        }
        workgroupBarrier();
    }
    let c = c0 + my_c;
    if c < p.cols {
        for (var k = 0u; k < PER; k++) {
            let t = t0 + my_t + k;
            if t < p.t_len {
                y[t * p.cols + c] = acc[k];
            }
        }
    }
}

// The first kernels (one invocation per row or column, no workgroup
// memory), kept to compare on real GPUs: NATIVA_GPU_KERNELS=1.

// Positions per invocation: each weight is decoded once for all of them.
const TT: u32 = 8u;

// One invocation per weight row, TT positions each.
@compute @workgroup_size(64)
fn matmul_v1(@builtin(global_invocation_id) id: vec3<u32>) {
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
fn matmul_t_v1(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
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
