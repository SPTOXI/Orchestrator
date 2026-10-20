# IA Nativa — projeto de implementação

Decisão em [ADR-0027](./adr/0027-ia-nativa.md). Este documento é o plano:
módulos, contratos, fases e critérios de aceite.

> Motor próprio, modelo próprio. Uma IA de 8 GB para baixo que opera o
> Orchestrator e o computador, executa rotinas e aprende com o usuário.

## Independência tecnológica

Regra do projeto ([ADR-0027 §0](./adr/0027-ia-nativa.md)): nenhum código
de outro motor ou framework; só ideias publicadas, com a fonte citada na
tabela abaixo (preenchida a cada técnica implementada).

| Técnica | Fonte (artigo/especificação) | Onde no código |
| ------- | ---------------------------- | -------------- |
| Byte-pair encoding (tokenizador por bytes) | Sennrich, Haddow e Birch, "Neural Machine Translation of Rare Words with Subword Units" (2016) | `packages/engine/src/tokenizer.rs` |
| Transformer decoder, atenção | Vaswani et al., "Attention Is All You Need" (2017) | `packages/engine/src/model.rs` |
| Atenção com grupos de KV (GQA) | Ainslie et al., "GQA: Training Generalized Multi-Query Transformer Models from Multi-Head Checkpoints" (2023) | `packages/engine/src/model.rs` |
| RMSNorm | Zhang e Sennrich, "Root Mean Square Layer Normalization" (2019) | `packages/engine/src/ops.rs` |
| Posições rotativas (RoPE) | Su et al., "RoFormer: Enhanced Transformer with Rotary Position Embedding" (2021) | `packages/engine/src/ops.rs` |
| SwiGLU | Shazeer, "GLU Variants Improve Transformer" (2020) | `packages/engine/src/model.rs` |
| SiLU | Elfwing, Uchibe e Doya (2017); Ramachandran, Zoph e Le (2017) | `packages/engine/src/ops.rs` |
| Top-p (núcleo) | Holtzman et al., "The Curious Case of Neural Text Degeneration" (2019) | `packages/engine/src/sampler.rs` |
| Min-p | Nguyen et al., "Turning Up the Heat: Min-p Sampling for Creative and Coherent LLM Outputs" (2024) | `packages/engine/src/sampler.rs` |
| Gerador SplitMix64 | Steele, Lea e Flood, "Fast Splittable Pseudorandom Number Generators" (2014) | `packages/engine/src/sampler.rs` |
| Quantização simétrica por bloco (escala absmax) | Dettmers et al., "LLM.int8(): 8-bit Matrix Multiplication for Transformers at Scale" (2022) | `packages/engine/src/quant.rs` |
| Pesos ternários com escala absmean | Ma et al., "The Era of 1-bit LLMs: All Large Language Models are in 1.58 Bits" (2024) | `packages/engine/src/quant.rs` |
| Instruções SIMD (AVX2, FMA, NEON) | manuais dos fabricantes: Intel Intrinsics Guide; Arm Neon Intrinsics Reference | `packages/engine/src/simd.rs` |

## Visão geral

```text
┌──────────────────────────── app (Tauri) ───────────────────────────────────────┐
│ painéis NATIVA · ROTINAS · Gravar · propostas de aprendizado · Parar global    │
│                                                                                │
│ NativeAgent ─▶ roteador de grupo ─▶ ferramentas do grupo + habilidades         │
│      │                                                                         │
│      ▼                                                                         │
│ orchestrator-engine (Rust, no processo)                                        │
│   .nativa mmap · núcleo + especialistas sob demanda · ternário/Q4/Q8 SIMD     │
│   cache KV q8 · gramática · embeddings                                         │
│ trainer (no app): treino bloco a bloco · CPU + GPU 1 GB · sono noturno         │
│   especialistas de língua e pessoal · validado antes de trocar                 │
│      │                                                                         │
│      ▼                                                                         │
│ AutonomyGate ─▶ AgentTools ─▶ EngineTools ─▶ RuntimeTools                      │
│                                   ├─ orchestrator.*   ├─ system.*              │
│                                   └─ filesystem / shell / git / web            │
│                                                                                │
│ Observador ─▶ trilhas ─▶ Minerador ─▶ propostas │ Agendador ─▶ rotinas         │
└────────────────────────────────────────────────────────────────────────────────┘
```

## Módulos

| Onde | O quê |
| ---- | ----- |
| `packages/engine` (crate `orchestrator-nativa`) | motor: formato `.nativa`, quantização, kernels SIMD, transformer, cache KV, amostragem, gramática, embeddings, LoRA |
| `packages/trainer` (novo) | treino no próprio app: dados, currículo, treino bloco a bloco, especialistas, consolidação noturna, avaliação, exportação `.nativa` |
| `training/reference` (Python + numpy) | só o gabarito para os testes de equivalência, escrito à parte do motor; não é necessário para treinar nem para usar |
| `packages/native` (novo) | `NativeAgent`, roteador de grupo, recuperação de habilidades, agendamento do treino LoRA |
| `packages/system` (novo) | ferramentas `system.*` por SO (`cfg(target_os)`), com fallback para shell |
| `packages/library` (novo) | Biblioteca no disco: pacotes de conhecimento, índice BM25 + vetores por `mmap`, atualização, citações |
| `packages/memory` | tabelas `traces`, `skills`, `routines`, `routine_runs`, `preferences`, `embeddings`, `adapters` |
| `packages/orchestrator` | ferramentas `orchestrator.*` sobre o gateway; agendador de rotinas |
| `packages/core` | contratos `Routine`, `RoutineStep`, `Trigger`, `Skill`, `Trace`; eventos `routine.*`, `native.*` |
| `apps/desktop` | painéis, botão Gravar, propostas, atalho global de parada |

## Contratos (rascunho)

```rust
// motor
pub struct Engine { /* pesos mmap, pool de threads, orçamento de memória */ }
impl Engine {
    pub fn load(path: &Path, budget: MemoryBudget) -> Result<Self>;
    pub fn generate(&self, req: GenerateRequest, cancel: CancelToken)
        -> impl Stream<Item = Token>;               // com Grammar opcional
    pub fn embed(&self, text: &str) -> Vec<f32>;
    pub fn train_lora(&self, data: &[Example], cfg: LoraConfig,
        cancel: CancelToken) -> Result<Adapter>;
}

// rotinas e aprendizado
pub enum Trigger { Manual, Cron(String), Event(EventFilter), OnStartup }
pub enum RoutineStep { Tool { name: String, args: Value }, Ask { prompt: String } }
pub struct Routine { id, name, trigger: Trigger, steps: Vec<RoutineStep>,
                     autonomy: Option<AutonomyMode>, enabled: bool }
pub struct Trace { id, at, project: Option<ProjectId>, origin: CallOrigin,
                   tool: String, args: Value, ok: bool }
pub struct Skill { id, name, description, params: Vec<Param>,
                   steps: Vec<RoutineStep>, source: SkillSource }
```

## Fases

Motor e modelo andam em paralelo; o agente usa a núcleo da Semente
enquanto os especialistas são treinados.

### M1 — Motor: base ✅
- Formato `.nativa` v1, transformer em f32 no caminho escalar, tokenizador
  por bytes com tokens especiais, amostragem (gulosa, temperatura, top-p,
  min-p, semente) e geração com parada, limite, contexto cheio e
  cancelamento. O `mmap` passou para a M2, junto com o orçamento de memória.
- **Aceite:** logits iguais aos da referência (numpy, float64) em todas as
  posições de um modelo de teste aleatório, tolerância 1e-4 — a diferença
  medida é ~2×10⁻⁶; os ids do tokenizador também batem.
  `cargo test -p orchestrator-nativa`; o gabarito é refeito com
  `python3 training/reference/nativa_ref.py packages/engine/tests/fixtures`.

### M2 — Motor: velocidade e memória ✅
- Formato `.nativa` v2 com pesos quantizados: `q8`, `q4` e ternário `t2`
  (blocos de 32 valores com uma escala); arquivos v1 continuam abrindo.
- Pesos lidos direto do arquivo por `mmap` (Linux, macOS e Windows), sem
  cópia na memória do processo.
- Kernels AVX2 + FMA (Intel/AMD, escolhidos na hora) e NEON (ARM, Apple
  M); as contas com inteiros dão exatamente o mesmo resultado do caminho
  escalar, bit a bit.
- Pool de threads persistente: as linhas de cada multiplicação são
  divididas entre os núcleos; o resultado não muda com o número de threads.
- Cache KV em 8 bits (padrão) ou f32; orçamento de memória
  (`SessionOptions::memory_budget`, `Model::memory`, `Model::fit_context`)
  que recusa ou reduz o contexto.
- Ferramentas: `nativa-quantize` (converte um modelo) e `nativa-bench`
  (mede velocidade e memória num modelo sintético).
- **Aceite:** meta ≥ 30 tokens/s e < 1 GB. Medido com `nativa-bench`
  (68 M parâmetros, forma da Semente: dim 512, 12 camadas, vocabulário de
  32 mil) numa máquina virtual Xeon de 4 núcleos com AVX2 — não é ainda o
  notebook de referência:

  | Pesos | Arquivo | Geração | Memória residente |
  | ----- | ------- | ------- | ----------------- |
  | `t2` | 26 MB | ~89 tokens/s | ~47 MB |
  | `q4` | 43 MB | ~87 tokens/s | ~74 MB |
  | `q8` | 77 MB | ~83 tokens/s | ~127 MB |
  | `f32` (antes da M2) | 273 MB | ~65 tokens/s (4 threads), ~40 (1 thread) | ~362 MB |

  Pendências anotadas: o ganho de várias threads nos pesos quantizados é
  pequeno (78 → 88 tokens/s com `t2`), porque o gargalo passou a ser
  desempacotar os blocos; o prompt ainda é processado token a token (em
  lotes fica para depois). Os kernels NEON passam pelos testes no CI do
  macOS (ARM).

### M3 — Motor: gramática, embeddings, LoRA
- JSON Schema → autômato; embeddings; treino LoRA no CPU.
- **Aceite:** 0 chamadas malformadas em 1.000 gerações com gramática; um
  LoRA de teste aprende uma tarefa sintética em < 1 h num CPU de 4 núcleos.

### T1 — Treino no notebook: base
- Tokenizador com base em bytes, treino bloco a bloco com pesos ternários,
  gradientes em 8 bits, *checkpointing*, GPU de 1 GB por blocos.
- **Aceite:** num notebook de 8 GB / 1 GB de vídeo, um modelo de ~40 M
  processa ≥ 300 milhões de tokens por semana de noites, sem passar de
  6 GB de RAM; a perda empata (± 5%) com o mesmo modelo treinado de forma
  densa clássica, como gabarito.

### T2 — Semente: núcleo + especialistas
- Núcleo ~40 M, roteador esparso, especialistas de função (`system`,
  `orchestrator`, shell, rotinas) e de língua (pt, en, es); dados
  sintéticos verificáveis gerados e conferidos em sandbox; crescimento
  progressivo e poda.
- **Aceite:** ≥ 85% de chamadas corretas no conjunto de 500 pedidos de
  referência do Orchestrator e de `system.*`, nas três línguas; modelo de
  fábrica < 100 MB.

### T3 — Línguas novas pelo usuário
- Escolher a língua, baixar corpus público, treinar o especialista em 1–2
  noites, traduzir ferramentas e rotinas, testar antes de ativar.
- **Aceite:** uma língua fora das três de fábrica (ex.: italiano) chega a
  ≥ 75% de chamadas corretas depois de 2 noites.

### B1 — Biblioteca
- Formato dos pacotes, índice no disco, busca híbrida (palavras + vetores),
  pacotes iniciais (documentação de SO e comandos, Wikipédia pt/en/es),
  pastas do usuário, citações.
- **Aceite:** com 20 GB de Biblioteca, uma busca responde em < 200 ms
  usando < 300 MB de RAM; respostas com fatos citam a fonte.

### N1 — Agente e ferramentas
- `NativeAgent` com roteamento; `system.*` nos três SOs; `orchestrator.*`
  com o interruptor de administração.
- **Aceite:** "instale o VLC", "o que está usando a memória?", "abra o
  projeto X e crie uma task" funcionam nos três SOs (CI da ADR-0006 com
  testes que não mudam a máquina).

### N2 — Rotinas
- Tabelas, agendador (cron + eventos), painel ROTINAS, histórico.
- **Aceite:** uma rotina com cron roda no horário, registra cada passo e
  pode ser pausada no meio.

### N3 — Aprendizado
- Trilhas, Gravar, minerador, preferências, recuperação, LoRA pessoal com
  validação e volta ao anterior.
- **Aceite:** depois de 3 repetições a Nativa propõe a rotina com os
  parâmetros certos; um adaptador que piora a avaliação é descartado
  sozinho.

### N4 — Polimento
- Atalho global de parada, delegação
  para providers grandes, documentação de usuário.

## Riscos

| Risco | Mitigação |
| ----- | --------- |
| Motor lento no CPU | kernels SIMD por arquitetura, tokenizador econômico, modelo de 1,2 B, testes de desempenho no CI |
| Bugs numéricos no motor | testes de equivalência kernel a kernel e contra a referência em numpy |
| Modelo pequeno erra | treino focado nas ferramentas reais, gramática, roteamento, exemplos recuperados, LoRA pessoal |
| Técnicas de treino eficiente não renderem | cada uma entra só depois de medida contra o treino denso clássico (T1); se uma falhar, troca-se por outra sem mudar o resto |
| Treino lento demais no notebook | peças pequenas (1 noite cada), dados sintéticos verificáveis, crescimento progressivo |
| Biblioteca ocupa muito disco | pacotes escolhidos pelo usuário, com tamanho à vista; texto comprimido |
| Licença dos dados | só corpora que permitem treinar e redistribuir; origem marcada em cada exemplo |
| Ação destrutiva por engano | o usuário escolhe o modo; Pause/Parar; auditoria; ponto de restauração; exemplos negativos no treino contra ações destrutivas não pedidas |
| Adaptador pessoal piora a IA | só troca se passar na avaliação; volta ao anterior ou ao de fábrica a qualquer momento |
| Privacidade | trilhas e adaptador só locais, apagáveis, nunca enviados sem pedido |

## Pendências para fases futuras

### Colmeia (treino distribuído, estilo SETI@home)
- Adiada até o projeto amadurecer ([ADR-0027 §2c](./adr/0027-ia-nativa.md)).
- Módulos previstos: `packages/hive` (cliente) e `services/hive`
  (coordenador).
- Coordenador, unidades de trabalho, treino de baixa comunicação,
  redundância e conjunto secreto, reputação, limites de ociosidade,
  temperatura e horário, painel e créditos.
- **Aceite:** com 50 participantes simulados (alguns sabotadores), um
  especialista treinado pela Colmeia empata com o treinado num só
  computador e os sabotadores são descartados; um participante nunca
  envia nada além da diferença de pesos da unidade.
