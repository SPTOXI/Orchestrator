# ADR-0010 — Providers por API com cadastro livre e nova ordem das Fases 4–5

- **Estado:** Aceita
- **Fase:** 4

## Contexto

O documento mestre previa OpenAI/Codex (Fase 4) e Claude Code (Fase 5) como
os primeiros providers. Ao fim da Fase 3, o usuário corrigiu a direção:

- a integração é **por API**, não por CLIs (Codex CLI, Claude Code);
- o usuário cadastra **quantas APIs quiser**, de qualquer fornecedor, e não
  necessariamente compatíveis com a API da OpenAI;
- um **Conselho** de IAs (ou uma única IA gerenciadora) deve recomendar o
  melhor modelo para cada atividade, com a opção **Full**, em que o conselho
  decide sozinho e alimenta os modos Autônomo e Acesso Irrestrito.

A camada da Fase 3 (ADR-0009) já é independente de fornecedor: cada
conexão cadastrada vira um provider no `ProviderRegistry`.

## Decisão

1. **Providers só por API.** Os diretórios planejados `providers/openai` e
   `providers/claude` dão lugar a um único crate, `packages/providers/api`
   (`orchestrator-provider-api`). Nele, cada protocolo é um módulo. O Claude
   passa a ser usado pela API da Anthropic.
2. **Cadastro livre de conexões.** Uma conexão tem id (vira o `ProviderId`),
   nome, tipo, URL base, credencial, headers extras, campos extras do corpo,
   modelos, modo de ferramentas e limites. Tipos:
   - `openai` — Chat Completions; também atende qualquer API compatível
     (OpenRouter, DeepSeek, Groq, Mistral, Ollama, LM Studio, vLLM…) trocando
     a URL base;
   - `anthropic` — Messages API;
   - `gemini` — Gemini API (`generateContent`);
   - `generic` — **qualquer API HTTP/JSON**, descrita por um perfil:
     - caminho e método;
     - autenticação por bearer, header ou parâmetro na URL;
     - modelo do corpo com marcadores (`{{model}}`, `{{messages}}`,
       `{{prompt}}`, `{{system}}`, `{{maxTokens}}`, `{{stream}}`);
     - formato das mensagens (lista de papéis ou texto único);
     - streaming SSE, NDJSON ou nenhum;
     - caminhos (notação com pontos) para o texto, o uso de tokens, o fim do
       stream e a lista de modelos.

   Os tipos nativos são atalhos, não limites: pode haver várias conexões de
   cada tipo, e o que um nativo não cobrir o genérico cobre.
3. **Ferramentas para qualquer modelo.** Cada conexão escolhe:
   - `native` — chamada de funções do protocolo (OpenAI, Anthropic, Gemini);
   - `prompt` — o Orchestrator descreve as ferramentas no prompt de sistema;
     o modelo responde com `<tool_call>{"tool": …, "args": …}</tool_call>` e
     recebe `<tool_result>`. O texto dos blocos não aparece na conversa
     mostrada ao usuário. Funciona com qualquer modelo de texto; é o padrão
     do tipo `generic`;
   - `none` — só conversa.

   Em todos os casos quem executa é o Orchestrator, via
   `TurnContext::call_tool` (ADR-0009). Nomes com ponto viram `grupo__acao`
   nos protocolos que não aceitam ponto.
4. **Schemas das ferramentas.** O catálogo passa a expor o JSON Schema dos
   argumentos de cada ferramenta (`ToolDefinition`), gerado com `schemars` a
   partir dos próprios tipos que o runtime desserializa. Assim o que o modelo
   vê é exatamente o que o runtime aceita.
5. **Credenciais.** A chave fica no **cofre do sistema operacional**
   (Windows Credential Manager, macOS Keychain, Secret Service no Linux,
   crate `keyring`) ou é lida de uma **variável de ambiente** indicada pelo
   usuário. Nunca é gravada em arquivo, em evento de histórico ou em log, e
   nunca volta para a UI (a UI só sabe se existe).
6. **Persistência e edição.** As conexões (sem segredos) ficam em
   `<app-data>/connections.json` até o SQLite da Fase 6. Salvar e remover
   geram `CONNECTION_SAVED` e `CONNECTION_REMOVED` no histórico. O registro
   ganha `unregister`/`replace`.
   - A sessão guarda o **id** do provider e pega a instância registrada a
     cada turno, retomada ou subagente. Editar uma conexão (nova chave, URL,
     modelos) vale para as sessões abertas a partir do próximo turno, sem
     perder a conversa: as instâncias de uma mesma conexão compartilham o
     histórico enviado à API.
   - Conexão removida, desativada ou renomeada: o próximo turno das sessões
     dela falha com uma mensagem clara e nada é enviado; a sessão continua
     listada. Reativada, a sessão volta a funcionar com a conversa.
   - Remover apaga também a chave guardada no cofre.
7. **Loop de ferramentas.** Um turno repete chamar o modelo → executar as
   ferramentas pedidas → devolver os resultados, até o modelo responder sem
   pedir ferramentas.
   - O limite de rodadas (`maxToolRounds`, padrão 50) é uma configuração
     visível da conexão, contra laços infinitos e gasto de tokens.
   - Ao atingi-lo, o turno termina com um aviso explícito no transcript. Não
     é uma restrição de operações: nenhuma ferramenta é bloqueada.
8. **Uso e custo.** O uso de tokens vem da resposta de cada API. O custo é
   calculado quando o usuário informa o preço por milhão de tokens de entrada
   e de saída do modelo (as APIs não informam preço). Os modelos também
   guardam contexto, suporte a ferramentas e etiquetas livres, que o
   Conselho usará.
9. **Testar conexão.** Envia uma mensagem curta e, se houver ferramentas,
   pede uma chamada a uma ferramenta de teste (`ping`, que não toca no
   sistema). Mostra latência, texto, uso e se a chamada de ferramenta
   funcionou. Uma configuração pode ser testada antes de ser salva.
10. **Nova ordem das fases.**
    - **Fase 4:** providers por API com cadastro livre (este ADR).
    - **Fase 5:** Roteador de modelos e **Conselho**, com decisão registrada
      em ADR próprio. O Conselho tem de 1 a N membros; com 1 membro, ele é o
      "gerenciador". Modos: *Sugerir* (o usuário aprova) e *Full* (o conselho
      decide e aplica; ligado aos modos Autônomo e Acesso Irrestrito na
      Fase 9). Deliberações registradas no histórico, com cache para
      economizar tokens.
    - **Fases 6–11:** sem mudança de escopo; as Fases 8–9 ligam o Conselho a
      tarefas, agentes e autonomia.
11. **Dependências novas:**
    - `reqwest` 0.12 (rustls com `ring` e certificados do sistema, streaming);
    - `schemars` 1;
    - `keyring` 3, só no app desktop: os crates do núcleo recebem um
      `SecretStore` e os testes usam um cofre em memória.

## Consequências

- Qualquer IA acessível por HTTP entra sem código novo (perfil genérico), e
  as principais têm suporte nativo completo.
- O núcleo, a UI de sessões e o Tool Runtime não mudam: conexões são
  providers como qualquer outro.
- Os testes usam um servidor HTTP falso local que imita cada protocolo
  (streaming, chamadas de ferramenta, erros). Testes com contas reais
  dependem das chaves do usuário.
- A conversa de cada sessão (histórico enviado à API) fica em memória no
  adapter até a Fase 6, quando será reconstruída a partir das mensagens
  persistidas. O Context Builder (Fase 7) passa a decidir o que enviar.
- No Linux sem Secret Service (alguns ambientes mínimos), o cofre não está
  disponível; a UI informa o erro e a opção de variável de ambiente
  continua funcionando.
