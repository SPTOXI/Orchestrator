# ADR-0003 — Gateway IPC único (`runtime_invoke`) e canal de streaming do terminal

- **Estado:** Aceita
- **Fase:** 1

## Contexto

A UI precisa executar operações de sistema (listar arquivos, abrir terminal,
iniciar processos). Os providers, a partir da Fase 3, farão o mesmo via
`tool_call`. Se a UI tiver comandos Tauri próprios para cada operação, haverá
dois caminhos de execução — um deles possivelmente sem auditoria.

Por outro lado, a digitação humana num terminal gera um evento por tecla, e o
redimensionamento da janela gera muitos eventos de resize. Auditar cada um
deles como `TOOL_CALLED` inundaria o histórico sem valor de diagnóstico.

## Decisão

1. Um único comando Tauri, `runtime_invoke(tool, args)`, executa **qualquer**
   ferramenta do catálogo, construindo um `ToolCall` com `origin = user` e
   chamando `ToolRuntime::invoke`. É exatamente o mesmo caminho que os
   providers usarão (com `origin = agent`).
2. A interação humana de baixo nível com um terminal já aberto usa um canal de
   streaming dedicado:
   - `terminal_input(id, data)` — bytes digitados pelo usuário;
   - `terminal_resize(id, cols, rows)` — tamanho do PTY.
   Ambos vão direto ao `TerminalManager` do runtime (não contornam o runtime),
   mas **não** geram `TOOL_CALLED` por tecla. A saída resultante continua
   registrada no buffer do terminal, e criar/fechar terminais continua sendo
   tool call auditada.
3. Providers **não** têm acesso a esse canal de streaming: para eles existe a
   ferramenta auditada `terminal.write`.
4. O runtime publica eventos pelos canais Tauri `runtime://stream`
   (`StreamEvent`) e `runtime://audit` (`AuditEvent`).

## Consequências

- Uma única superfície para auditar, versionar e, na Fase 9, submeter ao gate
  de autonomia.
- A UI é um cliente do runtime como qualquer outro.
