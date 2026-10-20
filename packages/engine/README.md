# packages/engine

**O motor próprio da IA Nativa**: crate `orchestrator-nativa`
([ADR-0027](../../docs/adr/0027-ia-nativa.md), plano em
[`docs/native-ai.md`](../../docs/native-ai.md)).

Escrito pelo projeto, sem biblioteca de inferência ou de treino: só ideias
publicadas, com a fonte de cada uma na tabela de `docs/native-ai.md`.

| Módulo | O que faz |
| ------ | --------- |
| `format.rs` | o arquivo `.nativa`: versão, cabeçalho JSON (config, tokenizador, índice), tensores alinhados em 64 bytes; recusa arquivos de versão mais nova |
| `tokenizer.rs` | BPE por bytes (qualquer língua e escrita) e tokens especiais do protocolo da Nativa |
| `ops.rs` | contas em f32, caminho escalar: matriz × vetor, RMSNorm, softmax, SiLU, RoPE |
| `model.rs` | o transformer (atenção com grupos de KV, SwiGLU) e a `Session` com o cache KV |
| `sampler.rs` | gulosa, temperatura, top-p, min-p, com gerador próprio (mesma semente, mesmo texto) |
| `generate.rs` | o laço de geração: tokens de parada, limite, contexto cheio, cancelamento |

Estado: fase **M1** concluída (caminho escalar em f32). Próxima: M2 —
`mmap`, quantização, kernels SIMD, multithread, orçamento de memória.

Testes: `cargo test -p orchestrator-nativa`. `tests/reference.rs` confere
o motor contra o gabarito independente em `training/reference/nativa_ref.py`
(numpy, float64): todos os logits de todas as posições e os ids do
tokenizador. Para refazer o gabarito:

```sh
python3 training/reference/nativa_ref.py packages/engine/tests/fixtures
```
