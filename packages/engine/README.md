# packages/engine

**O motor próprio da IA Nativa**: crate `orchestrator-nativa`
([ADR-0027](../../docs/adr/0027-ia-nativa.md), plano em
[`docs/native-ai.md`](../../docs/native-ai.md)).

Escrito pelo projeto, sem biblioteca de inferência ou de treino: só ideias
publicadas, com a fonte de cada uma na tabela de `docs/native-ai.md`.

| Módulo | O que faz |
| ------ | --------- |
| `format.rs` | o arquivo `.nativa`: versão (1: f32; 2: quantizados), cabeçalho JSON (config, tokenizador, índice), tensores alinhados em 64 bytes; recusa arquivos de versão mais nova |
| `mmap.rs` | o arquivo mapeado na memória (Linux, macOS, Windows): os pesos não são copiados |
| `quant.rs` | pesos `q8`, `q4` e ternário `t2` em blocos de 32 com escala; caminho escalar de referência |
| `simd.rs` | kernels AVX2 + FMA e NEON, iguais ao caminho escalar bit a bit nas contas inteiras |
| `pool.rs` | pool de threads persistente para dividir as multiplicações |
| `convert.rs` | quantizar um modelo |
| `tokenizer.rs` | BPE por bytes (qualquer língua e escrita) e tokens especiais do protocolo da Nativa |
| `ops.rs` | contas em f32, caminho escalar: matriz × vetor, RMSNorm, softmax, SiLU, RoPE |
| `model.rs` | o transformer (atenção com grupos de KV, SwiGLU), a `Session` com o cache KV (f32 ou 8 bits) e o orçamento de memória |
| `sampler.rs` | gulosa, temperatura, top-p, min-p, com gerador próprio (mesma semente, mesmo texto) |
| `generate.rs` | o laço de geração: tokens de parada, limite, contexto cheio, cancelamento |

Estado: fases **M1** (caminho escalar em f32) e **M2** (velocidade e
memória) concluídas. Próxima: M3 — gramática, embeddings, treino LoRA.

Programas:

```sh
cargo run --release -p orchestrator-nativa --bin nativa-bench -- --dtype t2
cargo run --release -p orchestrator-nativa --bin nativa-quantize -- in.nativa out.nativa q4
```

Testes: `cargo test -p orchestrator-nativa`. `tests/m2.rs` confere os
pesos quantizados, o cache KV em 8 bits, 1 contra 4 threads, `mmap` contra
leitura normal e o orçamento de memória. `tests/reference.rs` confere
o motor contra o gabarito independente em `training/reference/nativa_ref.py`
(numpy, float64): todos os logits de todas as posições e os ids do
tokenizador. Para refazer o gabarito:

```sh
python3 training/reference/nativa_ref.py packages/engine/tests/fixtures
```
