# IA Nativa — projeto de implementação

Decisão em [ADR-0027](./adr/0027-ia-nativa.md). Este documento é o plano:
módulos, contratos, fases e critérios de aceite.

> Uma IA local, de 8 GB para baixo, que opera o Orchestrator e o
> computador, executa rotinas e aprende com o usuário.

## Visão geral

```text
          ┌──────────────── interface (painel NATIVA, ROTINAS, Gravar) ───────────────┐
          │                                                                           │
pedido ──▶│ NativeAgent ─▶ Roteador de grupo ─▶ ferramentas do grupo + habilidades    │
          │      │              (gramática)        recuperadas (embeddings)           │
          │      ▼                                                                    │
          │ llama-server (ADR-0025)  ◀── perfil por RAM, desliga ocioso               │
          │      │                                                                    │
          │      ▼                                                                    │
          │ AutonomyGate ─▶ AgentTools ─▶ EngineTools ─▶ RuntimeTools                 │
          │                                    ├─ orchestrator.* (gateway IPC)        │
          │                                    ├─ system.*       (novo)               │
          │                                    └─ filesystem/shell/git/web            │
          │                                                                           │
          │ Observador ─▶ trilhas (SQLite) ─▶ Minerador ─▶ propostas de rotina        │
          │ Agendador  ─▶ rotinas (cron/evento) ─▶ NativeAgent                        │
          └───────────────────────────────────────────────────────────────────────────┘
```

## Módulos

| Onde | O quê |
| ---- | ----- |
| `packages/native` (novo crate) | `NativeAgent`, roteador de grupo, perfis por RAM, recuperação de habilidades |
| `packages/system` (novo crate) | ferramentas `system.*` por SO (`cfg(target_os)`), com fallback para shell |
| `packages/local` | perfil `native`: escolha do modelo pela RAM, embeddings, desligar ocioso, teto de memória |
| `packages/memory` | tabelas `traces`, `skills`, `routines`, `routine_runs`, `preferences`, `embeddings` |
| `packages/orchestrator` | ferramentas `orchestrator.*` sobre os comandos do gateway; agendador de rotinas |
| `packages/core` | contratos `Routine`, `RoutineStep`, `Trigger`, `Skill`, `Trace`; eventos `routine.*`, `native.*` |
| `apps/desktop` | painéis NATIVA e ROTINAS, botão Gravar, propostas de aprendizado, atalho global de parada |

## Contratos (rascunho)

```rust
pub enum Trigger { Manual, Cron(String), Event(EventFilter), OnStartup }

pub enum RoutineStep {
    Tool { name: String, args: serde_json::Value }, // sem modelo
    Ask { prompt: String },                         // a Nativa decide
}

pub struct Routine { id, name, trigger: Trigger, steps: Vec<RoutineStep>,
                     autonomy: Option<AutonomyMode>, enabled: bool }

pub struct Trace { id, at, project: Option<ProjectId>, origin: CallOrigin,
                   tool: String, args: Value, ok: bool }

pub struct Skill { id, name, description, params: Vec<Param>,
                   steps: Vec<RoutineStep>, source: SkillSource /* gravada | minerada | escrita */ }
```

## Fases

### N1 — Motor e perfil para 8 GB
- Detectar RAM/CPU/GPU; escolher e baixar o modelo do perfil.
- `llama-server` com `json_schema`, KV `q8_0`, desligar ocioso, teto de RAM.
- **Aceite:** num computador de 8 GB, a Nativa responde em < 3 s ao
  primeiro token com o motor quente e o sistema não troca para o disco.

### N2 — Agente com gramática e roteamento
- `NativeAgent` com roteamento em dois passos e validação de argumentos.
- **Aceite:** conjunto de 100 pedidos de referência com ≥ 90% de chamadas
  corretas no modelo 3–4B (teste automatizado em `packages/native/tests`).

### N3 — `system.*` e `orchestrator.*`
- Ferramentas da ADR-0027 §3 para Windows, macOS e Linux; elevação pelo SO
  com senha opcional no cofre.
- Interruptor "A Nativa administra o Orchestrator".
- **Aceite:** "instale o VLC", "o que está usando a memória?", "abra o
  projeto X e crie uma task" funcionam nos três SOs (CI multiplataforma,
  ADR-0006, com testes que não mudam a máquina do CI).

### N4 — Rotinas
- Tabelas, agendador (cron + eventos), painel ROTINAS, histórico.
- **Aceite:** uma rotina com cron roda no horário com o app aberto,
  registra cada passo e pode ser pausada no meio.

### N5 — Aprendizado
- Observador de trilhas, botão Gravar, minerador de sequências, preferências
  a partir de correções, recuperação por embeddings.
- **Aceite:** depois de o usuário repetir a mesma sequência 3 vezes, a
  Nativa propõe a rotina com os parâmetros certos; uma preferência gravada
  muda a próxima resposta.

### N6 — Polimento
- Atalho global de parada, delegação para providers grandes (handoff),
  exportação opcional de trilhas para LoRA, documentação de usuário.

## Riscos

| Risco | Mitigação |
| ----- | --------- |
| Modelo pequeno erra ferramentas | gramática, roteamento por grupo, exemplos recuperados, nova tentativa com o erro |
| Falta de RAM trava o computador | perfil por RAM, teto de memória, motor desliga ocioso |
| Ação destrutiva por engano | o usuário escolhe o modo; Pause/Parar; auditoria; ponto de restauração antes de rotinas que mudam o Orchestrator |
| Privacidade das trilhas | só locais, por projeto, apagáveis, nunca enviadas sem pedido |
| Diferenças entre SOs | `packages/system` com implementação por SO e testes no CI dos três |
