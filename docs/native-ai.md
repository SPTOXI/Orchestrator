# IA Nativa — projeto de implementação

Decisão em [ADR-0027](./adr/0027-ia-nativa.md). Este documento é o plano:
módulos, contratos, fases e critérios de aceite.

> Motor próprio, modelo próprio. Uma IA de 8 GB para baixo que opera o
> Orchestrator e o computador, executa rotinas e aprende com o usuário.

## Visão geral

```text
                        ┌──────── training/ (fora do app, GPUs alugadas) ────────┐
                        │ tokenizador BPE → pré-treino → SFT ferramentas → DPO   │
                        │            └──▶ exporta pesos .nativa + testes         │
                        └───────────────────────────┬────────────────────────────┘
                                                    ▼ release (SHA-256)
┌──────────────────────────── app (Tauri) ───────────────────────────────────────┐
│ painéis NATIVA · ROTINAS · Gravar · propostas de aprendizado · Parar global    │
│                                                                                │
│ NativeAgent ─▶ roteador de grupo ─▶ ferramentas do grupo + habilidades         │
│      │                                                                         │
│      ▼                                                                         │
│ orchestrator-engine (Rust, no processo)                                        │
│   .nativa mmap · kernels Q4/Q8 SIMD · cache KV q8 · gramática · embeddings     │
│   LoRA pessoal (treino ocioso, validado antes de trocar)                       │
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
| `training/` (novo, Python) | tokenizador, dados, pré-treino, SFT, DPO, avaliação, exportação `.nativa`, implementação de referência para os testes de equivalência |
| `packages/native` (novo) | `NativeAgent`, roteador de grupo, recuperação de habilidades, agendamento do treino LoRA |
| `packages/system` (novo) | ferramentas `system.*` por SO (`cfg(target_os)`), com fallback para shell |
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

Motor e modelo andam em paralelo; o agente usa a Nativa-Mini até a
Nativa-1 ficar pronta.

### M1 — Motor: base
- Formato `.nativa`, `mmap`, transformer em f32 no caminho escalar,
  amostragem, tokenizador.
- **Aceite:** logits iguais aos da referência PyTorch (tolerância 1e-4)
  num modelo de teste aleatório.

### M2 — Motor: velocidade e memória
- Quantização Q4/Q8, kernels AVX2/NEON, multithread, cache KV q8,
  orçamento de memória, cancelamento.
- **Aceite:** com a Nativa-1 em 4 bits num notebook de 8 GB (CPU de 4
  núcleos), ≥ 15 tokens/s na geração e uso total < 2 GB.

### M3 — Motor: gramática, embeddings, LoRA
- JSON Schema → autômato; embeddings; treino LoRA no CPU.
- **Aceite:** 0 chamadas malformadas em 1.000 gerações com gramática; um
  LoRA de teste aprende uma tarefa sintética em < 1 h num CPU de 4 núcleos.

### T1 — Modelo: Nativa-Mini
- Tokenizador BPE, pipeline de dados com licença rastreada, pré-treino de
  ~150 M, exportação `.nativa`.
- **Aceite:** roda no motor (M1–M2) com as mesmas saídas da referência.

### T2 — Modelo: Nativa-1
- Ambiente simulado (VMs) para gerar e validar trilhas de ferramentas;
  pré-treino ~1,2 B, SFT, DPO; conjunto de avaliação fixo.
- **Aceite:** ≥ 90% de chamadas corretas no conjunto de 500 pedidos de
  referência do Orchestrator e de `system.*`, em português e inglês.

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
- Backend GPU por `wgpu`, Nativa-2, atalho global de parada, delegação
  para providers grandes, documentação de usuário.

## Riscos

| Risco | Mitigação |
| ----- | --------- |
| Motor lento no CPU | kernels SIMD por arquitetura, tokenizador econômico, modelo de 1,2 B, testes de desempenho no CI |
| Bugs numéricos no motor | testes de equivalência kernel a kernel e contra a referência PyTorch |
| Modelo pequeno erra | treino focado nas ferramentas reais, gramática, roteamento, exemplos recuperados, LoRA pessoal |
| Custo e tempo do treino | começar pela Nativa-Mini; pipeline reproduzível; só treinar a próxima versão com o ganho medido |
| Licença dos dados | só corpora que permitem treinar e redistribuir; origem marcada em cada exemplo |
| Ação destrutiva por engano | o usuário escolhe o modo; Pause/Parar; auditoria; ponto de restauração; DPO contra ações destrutivas não pedidas |
| Adaptador pessoal piora a IA | só troca se passar na avaliação; volta ao anterior ou ao de fábrica a qualquer momento |
| Privacidade | trilhas e adaptador só locais, apagáveis, nunca enviados sem pedido |
