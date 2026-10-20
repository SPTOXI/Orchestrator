//! The GPU backend (phase G1), behind the `gpu` feature: the batched
//! matrix products of the prompt and of training run on the GPU, through
//! `wgpu` (Vulkan, Metal or DX12). The kernels are the project's own
//! (`gpu.wgsl`); `wgpu` only reaches the driver.
//!
//! Made for a notebook with 1 GB of video memory: the weights go to the
//! GPU *by blocks* — each matrix is uploaded the first time it is used and
//! stays while it fits the memory budget; past it, the least recently used
//! matrices are dropped. A matrix that is too big for the budget or for the
//! device stays on the CPU.
//!
//! The GPU multiplies f32 activations (the CPU kernels quantize them to
//! `q8` first), so its numbers differ from the CPU's by rounding; both are
//! held to the independent reference by the tests.

use std::collections::HashMap;
use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Instant;

use crate::error::{Error, Result};
use crate::format::Dtype;
use crate::model::GpuOptions;
use crate::quant;

/// Positions per invocation in the kernels (`TT` in `gpu.wgsl`).
const TT: usize = 8;

struct Resident {
    buf: wgpu::Buffer,
    bytes: u64,
    last: u64,
}

#[derive(Default)]
struct State {
    resident: HashMap<usize, Resident>,
    used: u64,
    clock: u64,
    /// Activations in, results out, and the buffer results are read through:
    /// grown as needed, reused across calls.
    input: Option<wgpu::Buffer>,
    output: Option<wgpu::Buffer>,
    read: Option<wgpu::Buffer>,
}

/// (dtype, rows, cols, transposed, positions rounded up).
type Shape = (Dtype, usize, usize, bool, usize);

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    forward: wgpu::ComputePipeline,
    transpose: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    name: String,
    budget: u64,
    max_binding: u64,
    failed: Arc<AtomicBool>,
    always: bool,
    /// Which side won for a kind of product — (dtype, rows, cols,
    /// direction, positions rounded up to a power of two): true for the
    /// GPU. Every layer has the same shapes, so a few measurements decide
    /// the whole model.
    choices: Mutex<HashMap<Shape, bool>>,
    on_gpu: AtomicU64,
    on_cpu: AtomicU64,
    state: Mutex<State>,
}

/// A matrix as the GPU sees it.
pub(crate) struct Weights<'a> {
    /// Stable for the model's lifetime: where its bytes live.
    pub key: usize,
    pub dtype: Dtype,
    pub rows: usize,
    pub cols: usize,
    pub bytes: &'a [u8],
}

impl Gpu {
    pub fn new(options: &GpuOptions) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| Error::Gpu(format!("no adapter: {e}")))?;
        let info = adapter.get_info();
        let limits = adapter.limits();
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("nativa"),
            required_limits: limits.clone(),
            ..Default::default()
        }))
        .map_err(|e| Error::Gpu(format!("no device: {e}")))?;
        let failed = Arc::new(AtomicBool::new(false));
        let flag = failed.clone();
        device.on_uncaptured_error(Arc::new(move |_| flag.store(true, Ordering::SeqCst)));

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nativa"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu.wgsl").into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage = |read_only| wgpu::BufferBindingType::Storage { read_only };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nativa"),
            entries: &[
                entry(0, wgpu::BufferBindingType::Uniform),
                entry(1, storage(true)),
                entry(2, storage(true)),
                entry(3, storage(false)),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nativa"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let forward = pipeline("matmul");
        let transpose = pipeline("matmul_t");
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        device.poll(wgpu::PollType::wait_indefinitely()).ok();
        if failed.load(Ordering::SeqCst) {
            return Err(Error::Gpu("the kernels did not build".into()));
        }
        Ok(Self {
            name: format!("{} ({:?})", info.name, info.backend),
            budget: options.memory,
            max_binding: limits
                .max_storage_buffer_binding_size
                .min(limits.max_buffer_size),
            device,
            queue,
            forward,
            transpose,
            layout,
            params,
            failed,
            always: options.always,
            choices: Mutex::new(HashMap::new()),
            on_gpu: AtomicU64::new(0),
            on_cpu: AtomicU64::new(0),
            state: Mutex::new(State::default()),
        })
    }

    /// The adapter in use, e.g. "llvmpipe (Vulkan)".
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Weight bytes now in video memory.
    pub fn resident_bytes(&self) -> u64 {
        self.state.lock().unwrap().used
    }

    /// Products run on each side: (GPU, CPU).
    pub fn share(&self) -> (u64, u64) {
        (
            self.on_gpu.load(Ordering::Relaxed),
            self.on_cpu.load(Ordering::Relaxed),
        )
    }

    /// Runs one product on the faster side, leaving the result in `out`;
    /// false means the caller must run it on the CPU itself. The first
    /// time a kind of product is seen, both sides run it (the GPU twice, so
    /// the upload is not counted) and the faster one is remembered: a GPU
    /// slower than the CPU is simply not used.
    pub(crate) fn choose(
        &self,
        w: &Weights,
        transposed: bool,
        t_len: usize,
        out: &mut [f32],
        gpu: &mut dyn FnMut(&mut [f32]) -> bool,
        cpu: &mut dyn FnMut(&mut [f32]),
    ) -> bool {
        let key = (
            w.dtype,
            w.rows,
            w.cols,
            transposed,
            t_len.next_power_of_two(),
        );
        let known = if self.always {
            Some(true)
        } else {
            self.choices.lock().unwrap().get(&key).copied()
        };
        let won = match known {
            Some(true) => gpu(out),
            Some(false) => false,
            None => {
                let mut won = gpu(out);
                if won {
                    let t = Instant::now();
                    won = gpu(out);
                    let gpu_time = t.elapsed();
                    let mut scratch = vec![0.0; out.len()];
                    let t = Instant::now();
                    cpu(&mut scratch);
                    let cpu_time = t.elapsed();
                    if won && cpu_time < gpu_time {
                        out.copy_from_slice(&scratch);
                        self.on_cpu.fetch_add(1, Ordering::Relaxed);
                        self.choices.lock().unwrap().insert(key, false);
                        return true;
                    }
                }
                self.choices.lock().unwrap().insert(key, won);
                won
            }
        };
        if won {
            self.on_gpu.fetch_add(1, Ordering::Relaxed);
        } else {
            self.on_cpu.fetch_add(1, Ordering::Relaxed);
        }
        won
    }

    /// Whether `w` can run here at all (the caller then uses the CPU).
    pub(crate) fn fits(&self, w: &Weights) -> bool {
        w.cols.is_multiple_of(quant::BLOCK)
            && (w.bytes.len() as u64) <= self.budget.min(self.max_binding)
    }

    /// `out[t] = W · x[t]`. False if the GPU could not do it (nothing in
    /// `out` is then meaningful).
    pub(crate) fn matmul(&self, w: &Weights, x: &[f32], out: &mut [f32]) -> bool {
        let t_len = x.len() / w.cols;
        self.run(w, false, x, out, (w.rows.div_ceil(64), t_len.div_ceil(TT)))
    }

    /// `out[t] = Wᵀ · dy[t]` (not added: the caller adds).
    pub(crate) fn matmul_t(&self, w: &Weights, dy: &[f32], out: &mut [f32]) -> bool {
        let t_len = dy.len() / w.rows;
        self.run(
            w,
            true,
            dy,
            out,
            (w.cols / quant::BLOCK, t_len.div_ceil(TT)),
        )
    }

    fn run(
        &self,
        w: &Weights,
        transposed: bool,
        input: &[f32],
        out: &mut [f32],
        groups: (usize, usize),
    ) -> bool {
        if !self.fits(w)
            || (input.len() as u64 * 4).max(out.len() as u64 * 4) > self.max_binding
            || groups.0 > 65535
            || groups.1 > 65535
            || self.failed.load(Ordering::SeqCst)
        {
            return false;
        }
        let mut st = self.state.lock().unwrap();
        let weights = self.upload(&mut st, w);
        let in_bytes = input.len() as u64 * 4;
        let out_bytes = out.len() as u64 * 4;
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let input_buf = grow(&self.device, &mut st.input, in_bytes, storage);
        let output_buf = grow(
            &self.device,
            &mut st.output,
            out_bytes,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let read_buf = grow(
            &self.device,
            &mut st.read,
            out_bytes,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let kind = match w.dtype {
            Dtype::F32 => 0u32,
            Dtype::Q8 => 1,
            Dtype::Q4 => 2,
            Dtype::T2 => 3,
        };
        let t_len = if transposed {
            input.len() / w.rows
        } else {
            input.len() / w.cols
        };
        let params = [
            w.rows as u32,
            w.cols as u32,
            t_len as u32,
            (quant::row_bytes(w.dtype, w.cols) / 4) as u32,
            kind,
            0,
            0,
            0,
        ];
        self.queue
            .write_buffer(&self.params, 0, words_as_bytes(&params));
        self.queue.write_buffer(&input_buf, 0, f32_as_bytes(input));
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                bind(0, &self.params),
                bind(1, &weights),
                bind(2, &input_buf),
                bind(3, &output_buf),
            ],
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(if transposed {
                &self.transpose
            } else {
                &self.forward
            });
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(groups.0 as u32, groups.1 as u32, 1);
        }
        enc.copy_buffer_to_buffer(&output_buf, 0, &read_buf, 0, out_bytes);
        self.queue.submit([enc.finish()]);
        let slice = read_buf.slice(..out_bytes);
        let mapped = Arc::new(AtomicBool::new(false));
        let done = mapped.clone();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            done.store(r.is_ok(), Ordering::SeqCst)
        });
        if self
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .is_err()
            || !mapped.load(Ordering::SeqCst)
            || self.failed.load(Ordering::SeqCst)
        {
            return false;
        }
        {
            let Ok(view) = slice.get_mapped_range() else {
                return false;
            };
            for (o, b) in out.iter_mut().zip(view.chunks_exact(4)) {
                *o = f32::from_le_bytes(b.try_into().unwrap());
            }
        }
        read_buf.unmap();
        true
    }

    /// The matrix's buffer, uploaded now if it is not resident; the least
    /// recently used ones make room.
    fn upload(&self, st: &mut State, w: &Weights) -> wgpu::Buffer {
        st.clock += 1;
        let clock = st.clock;
        if let Some(r) = st.resident.get_mut(&w.key) {
            r.last = clock;
            return r.buf.clone();
        }
        let bytes = w.bytes.len() as u64;
        while st.used + bytes > self.budget {
            let oldest = st
                .resident
                .iter()
                .min_by_key(|(_, r)| r.last)
                .map(|(k, _)| *k);
            let Some(k) = oldest else { break };
            let r = st.resident.remove(&k).unwrap();
            st.used -= r.bytes;
            r.buf.destroy();
        }
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("weights"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&buf, 0, w.bytes);
        st.used += bytes;
        st.resident.insert(
            w.key,
            Resident {
                buf: buf.clone(),
                bytes,
                last: clock,
            },
        );
        buf
    }
}

fn bind(binding: u32, buf: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buf.as_entire_binding(),
    }
}

/// `slot` if it holds at least `bytes`, else a new buffer that does.
fn grow(
    device: &wgpu::Device,
    slot: &mut Option<wgpu::Buffer>,
    bytes: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let bytes = bytes.max(4);
    if let Some(b) = slot.as_ref().filter(|b| b.size() >= bytes) {
        return b.clone();
    }
    if let Some(old) = slot.take() {
        old.destroy();
    }
    let b = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        // Room to grow: fewer reallocations as prompts get longer.
        size: bytes.next_power_of_two(),
        usage,
        mapped_at_creation: false,
    });
    *slot = Some(b.clone());
    b
}

fn f32_as_bytes(v: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding and any byte is a valid u8.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn words_as_bytes(v: &[u32]) -> &[u8] {
    // SAFETY: as above.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

/// Runs a future to completion on this thread: `wgpu`'s native futures are
/// ready once the driver answers, so parking between polls is enough.
fn block_on<F: Future>(f: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}
