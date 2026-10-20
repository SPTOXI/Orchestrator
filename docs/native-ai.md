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
| — | — | — |

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
| `packages/engine` (novo) | motor: formato `.nativa`, quantização, kernels SIMD, transformer, cache KV, amostragem, gramática, embeddings, LoRA |
| `packages/trainer` (novo) | treino no próprio app: dados, currículo, treino bloco a bloco, especialistas, consolidação noturna, avaliação, exportação `.nativa` |
| `training/reference` (Python) | só o gabarito em PyTorch para os testes de equivalência; não é necessário para treinar |
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

### M1 — Motor: base
- Formato `.nativa`, `mmap`, transformer em f32 no caminho escalar,
  amostragem, tokenizador.
- **Aceite:** logits iguais aos da referência PyTorch (tolerância 1e-4)
  num modelo de teste aleatório.

### M2 — Motor: velocidade e memória
- Quantização Q4/Q8, kernels AVX2/NEON, multithread, cache KV q8,
  orçamento de memória, cancelamento.
- **Aceite:** com a Semente (~50 M ativos, ternária) num notebook de
  8 GB (CPU de 4 núcleos), ≥ 30 tokens/s na geração e uso total < 1 GB.

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
| Bugs numéricos no motor | testes de equivalência kernel a kernel e contra a referência PyTorch |
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
