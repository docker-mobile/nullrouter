# 🩺 Manual de Diagnóstico e Resolução de Problemas do NullRouter

Este manual contém o procedimento completo de triagem, dicionário de códigos de erro e soluções passo a passo para o NullRouter.

---

## 🧭 Triagem Rápida em 4 Passos

Execute estes comandos no terminal para identificar o subsistema com falha:

```bash
# Passo 1: Verificar se o gateway Pingora está respondendo na porta 20128
curl -I http://127.0.0.1:20128/api/health

# Passo 2: Checar o serviço de estado e configurações
curl -s http://127.0.0.1:20128/api/state | jq .status

# Passo 3: Listar modelos ativos e provedores conectados
curl -s http://127.0.0.1:20128/v1/models | jq '.data | length'

# Passo 4: Testar o streaming de tokens SSE
curl -N -X POST http://127.0.0.1:20128/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"auto","messages":[{"role":"user","content":"ping"}],"stream":true}'
```

---

## 📚 Dicionário de Códigos de Erro

| Código | Subsistema | Descrição | Ação Recomendada |
| :--- | :--- | :--- | :--- |
| **`E1001`** | Gateway (Pingora) | Porta `20128` ocupada por outro processo | Encerrar o processo conflitante ou usar `--port` |
| **`E1002`** | Gateway (Pingora) | Conexão recusada / Falha no DNS upstream | Verificar conexão com a internet, proxy corporativo ou firewall |
| **`E1003`** | Gateway (Pingora) | 504 Gateway Timeout (>60s sem resposta) | Verificar status do provedor ou aumentar o timeout |
| **`E1004`** | Gateway (Pingora) | Desconexão do cliente | Requisição cancelada pelo usuário no editor ou IDE |
| **`E2001`** | Runtime (Actix) | Nenhuma rota ativa disponível | Adicionar ao menos uma chave de API válida no painel |
| **`E2002`** | Runtime (Actix) | Falha de autenticação (HTTP 401/403) | Verificar chave de API ou saldo na conta do provedor |
| **`E2003`** | Runtime (Actix) | Limite de taxa 429 atingido | Habilitar pool multi-contas ou fallback para modelos gratuitos |
| **`E2004`** | Runtime (Actix) | Limite de contexto excedido | Habilitar compressão de tokens RTK no dashboard |
| **`E3001`** | Translate (SSE) | Chunk JSON malformado do upstream | Verificar se há portal cativo interceptando o tráfego HTTP |
| **`E3002`** | Translate (SSE) | Estouro de buffer / Erro de quebra CRLF | Atualizar o NullRouter para a versão mais recente |
| **`E3003`** | Translate (SSE) | Incompatibilidade de schema de tool call | Verificar suporte a chamadas de ferramentas no cliente |
| **`E4001`** | State (Actix) | Concorrência de trava no arquivo de estado | Garantir que apenas uma instância do NullRouter acesse o estado |
| **`E4002`** | State (Actix) | Arquivo `nullrouter-state.json` corrompido | Restaurar do backup ou recriar estado limpo |
| **`E4003`** | State (Actix) | Erro de migração de formato legado | Limpar cache temporário e reiniciar o assistente |

---

## 🔍 Resoluções Rápidas

### Resolução de Conflito de Porta (Erro `E1001`)
- **Linux**: `sudo ss -tulpn | grep 20128` seguido de `sudo kill -9 <PID>`
- **macOS**: `sudo lsof -nP -iTCP:20128 -sTCP:LISTEN` e `sudo kill -9 <PID>`
- **Windows**: `netstat -ano | findstr :20128` e `taskkill /F /PID <PID>`
