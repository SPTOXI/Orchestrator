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
| `simd.rs` | kernels AVX2 + FMA e NEON: produto inteiro (1 ou 4 linhas por vez), produto f32, `axpy` (1 ou 8 linhas por vez) |
| `pool.rs` | pool de threads persistente para dividir as multiplicações |
| `convert.rs` | quantizar um modelo |
| `tokenizer.rs` | BPE por bytes (qualquer língua e escrita) e tokens especiais do protocolo da Nativa |
| `ops.rs` | contas em f32, caminho escalar: matriz × vetor, RMSNorm, softmax, SiLU, RoPE |
| `model.rs` | o transformer (atenção com grupos de KV, SwiGLU), a `Session` com o cache KV (f32 ou 8 bits) e o prompt em lote (`prefill`), o orçamento de memória, multiplicações em lote para o prompt e o treino |
| `sampler.rs` | gulosa, temperatura, top-p, min-p, com gerador próprio (mesma semente, mesmo texto) |
| `generate.rs` | o laço de geração: tokens de parada, limite, contexto cheio, cancelamento; `generate_json` com gramática |
| `grammar.rs` | JSON Schema → verificador byte a byte e árvore do vocabulário: saída sempre válida |
| `lora.rs` | adaptadores LoRA: criar, aplicar, salvar, carregar |
| `train.rs` | treino do adaptador no CPU: passada com ativações, retropropagação à mão, AdamW |

Estado: fases **M1** (caminho escalar em f32), **M2** (velocidade e
memória) e **M3** (gramática, embeddings, treino LoRA) concluídas, e o
processamento em lote (prompt e treino) que vem antes da T1.

Programas:

```sh
cargo run --release -p orchestrator-nativa --bin nativa-bench -- --dtype t2 --train 3 --prompt 256
cargo run --release -p orchestrator-nativa --bin nativa-quantize -- in.nativa out.nativa q4
```

Testes: `cargo test -p orchestrator-nativa`. `tests/batch.rs` confere que
o prompt em lote dá os mesmos números que um token por vez. `tests/m2.rs` confere os
pesos quantizados, o cache KV em 8 bits, 1 contra 4 threads, `mmap` contra
leitura normal e o orçamento de memória. `tests/m3_grammar.rs` gera 1.000
chamadas com gramática e valida todas; `tests/m3_lora.rs` treina um
adaptador numa tarefa sintética, salva, carrega e confere os embeddings.
`tests/reference.rs` confere
o motor contra o gabarito independente em `training/reference/nativa_ref.py`
(numpy, float64): todos os logits de todas as posições e os ids do
tokenizador. Para refazer o gabarito:

```sh
python3 training/reference/nativa_ref.py packages/engine/tests/fixtures
```
