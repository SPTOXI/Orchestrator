//! `nativa-quantize <in.nativa> <out.nativa> <q8|q4|t2>`: quantizes the
//! matrices of a model file.

use std::io::BufWriter;
use std::path::Path;
use std::process::ExitCode;

use orchestrator_nativa::{convert, Dtype, NativaFile};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, dtype] = args.as_slice() else {
        eprintln!("usage: nativa-quantize <in.nativa> <out.nativa> <q8|q4|t2>");
        return ExitCode::from(2);
    };
    let dtype = match dtype.as_str() {
        "q8" => Dtype::Q8,
        "q4" => Dtype::Q4,
        "t2" => Dtype::T2,
        other => {
            eprintln!("unknown dtype {other}; use q8, q4 or t2");
            return ExitCode::from(2);
        }
    };
    let run = || -> Result<usize, Box<dyn std::error::Error>> {
        let src = NativaFile::open(Path::new(input))?;
        let mut out = BufWriter::new(std::fs::File::create(output)?);
        Ok(convert::quantize(&src, dtype, &mut out)?)
    };
    match run() {
        Ok(n) => {
            println!("{n} matrices quantized to {dtype:?} → {output}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("nativa-quantize: {e}");
            ExitCode::FAILURE
        }
    }
}
