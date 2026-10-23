//! Quantizing a model file: the matrices go to `q8`, `q4` or `t2`; norms
//! (and any matrix whose rows are not whole blocks) stay f32.

use std::io::Write;

use crate::error::Result;
use crate::format::{self, Dtype, NativaFile, TensorData};
use crate::quant::BLOCK;

/// Writes `src` with its matrices in `dtype`. Returns how many tensors were
/// quantized.
pub fn quantize(src: &NativaFile, dtype: Dtype, out: &mut impl Write) -> Result<usize> {
    let mut tensors = Vec::with_capacity(src.header.tensors.len());
    let mut quantized = 0;
    for info in &src.header.tensors {
        let values = src.tensor_f32(&info.name, &info.shape)?;
        let is_matrix = info.shape.len() == 2 && info.shape[1].is_multiple_of(BLOCK);
        let target = if is_matrix { dtype } else { Dtype::F32 };
        if target.is_quantized() {
            quantized += 1;
        }
        tensors.push(TensorData::quantized(
            &info.name,
            target,
            info.shape.clone(),
            &values,
        ));
    }
    let h = &src.header;
    let mut metadata = h.metadata.clone();
    metadata.insert("quantization".into(), format!("{dtype:?}").to_lowercase());
    format::write(out, &h.config, &h.tokenizer, &metadata, &tensors).map_err(|source| {
        crate::error::Error::Io {
            path: "<output>".into(),
            source,
        }
    })?;
    Ok(quantized)
}
