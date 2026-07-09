# DeepSeek Chat Completion API 结构文档

根据提供的 API 文档截图，以下是 **Request（请求）** 和 **Response（回复）** 的 JSON 嵌套结构及字段说明。

---

## 1. Request Body (请求体)

**Content-Type:** `application/json`

```json
{
  "model": "string (REQUIRED)", 
  "messages": [
    {
      "role": "string (REQUIRED)", 
      "content": "string | null (REQUIRED)",
      "name": "string (OPTIONAL)",
      "tool_calls": [], 
      "tool_call_id": "string" 
    }
  ],
  "thinking": {
    "type": "string" 
  },
  "reasoning_effort": "string",
  "max_tokens": "integer",
  "temperature": "number",
  "top_p": "number",
  "stream": "boolean",
  "stop": "string | array",
  "tools": [
    {
      "type": "string",
      "function": {
        "name": "string",
        "description": "string",
        "parameters": "object"
      }
    }
  ],
  "tool_choice": "string | object",
  "response_format": {
    "type": "string"
  },
  "stream_options": {
    "include_usage": "boolean"
  },
  "logprobs": "boolean",
  "top_logprobs": "integer",
  "user_id": "string"
}
```

### 请求参数字段详解

*   **`model`** (string, **REQUIRED**)
    *   使用的模型 ID。
    *   可选值：`deepseek-v4-flash`, `deepseek-v4-pro`。
*   **`messages`** (object[], **REQUIRED**)
    *   对话的消息列表，长度需 `>= 1`。
    *   支持 `oneOf`: System message, User message, Assistant message, Tool message。
    *   **System message**: `role`: "system", `content`: 系统消息内容。
    *   **User message**: `role`: "user", `content`: 用户消息内容。
*   **`thinking`** (object, NULLABLE)
    *   控制思考模式与非思考模式的转换。
    *   `type`: "enabled" (默认) | "disabled"。
*   **`reasoning_effort`** (string)
    *   控制模型的推理强度。
    *   可选值：`high` (默认), `max`。
    *   说明：`low`/`medium` 会映射为 `high`，`xhigh` 会映射为 `max`。
*   **`max_tokens`** (integer, NULLABLE)
    *   限制一次请求中模型生成 completion 的最大 token 数。
*   **`temperature`** (number, NULLABLE)
    *   采样温度，介于 0 和 2 之间。默认值：1。
*   **`top_p`** (number, NULLABLE)
    *   核采样参数，默认值：1。
*   **`stream`** (boolean, NULLABLE)
    *   如果设置为 true，将以 SSE 形式流式发送消息。
*   **`stop`** (object, nullable)
    *   停止生成的序列。
*   **`stream_options`** (object, NULLABLE)
    *   流式输出相关选项。
    *   `include_usage` (boolean): 如果为 true，在流式消息最后的 `data: [DONE]` 之前传输一个额外的块，包含 usage 信息。
*   **`tools`** (object[], NULLABLE)
    *   模型可能会调用的 tool 列表。目前仅支持 `function`。
    *   `type`: "function" (**REQUIRED**)。
    *   `function`: 包含 `name`, `description`, `parameters` 的对象。
*   **`tool_choice`** (object|string, nullable)
    *   控制模型调用 tool 的行为。
    *   可选字符串值：`none` (不调用), `auto` (自动选择), `required` (必须调用)。
    *   对象结构：`{"type": "function", "function": {"name": "my_function"}}`。
*   **`response_format`** (object, NULLABLE)
    *   指定返回格式，例如 `{"type": "text"}` 或 `{"type": "json_object"}`。
*   **`logprobs`** (boolean, NULLABLE)
    *   是否返回所输出 token 的对数概率。
*   **`top_logprobs`** (integer, NULLABLE)
    *   指定每个输出位置返回输出概率 top N 的 token (0-20)。需配合 `logprobs: true` 使用。
*   **`user_id`** (string, NULLABLE)
    *   自定义用户 ID，用于区分用户身份及 KVCache 缓存隔离。

---

## 2. Response Body (回复体)

**Content-Type:** `application/json`

```json
{
  "id": "string (REQUIRED)",
  "object": "chat.completion (REQUIRED)",
  "created": "integer (REQUIRED)",
  "model": "string (REQUIRED)",
  "choices": [
    {
      "index": "integer (REQUIRED)",
      "finish_reason": "string (REQUIRED)",
      "message": {
        "role": "assistant (REQUIRED)",
        "content": "string | null",
        "reasoning_content": "string | null",
        "tool_calls": [
          {
            "id": "string (REQUIRED)",
            "type": "function (REQUIRED)",
            "function": {
              "name": "string",
              "arguments": "string"
            }
          }
        ],
        "logprobs": {
           "content": [ ... ],
           "reasoning_content": [ ... ]
        }
      }
    }
  ],
  "usage": {
    "prompt_tokens": "integer (REQUIRED)",
    "completion_tokens": "integer (REQUIRED)",
    "total_tokens": "integer (REQUIRED)",
    "prompt_cache_hit_tokens": "integer (REQUIRED)",
    "prompt_cache_miss_tokens": "integer (REQUIRED)",
    "completion_tokens_details": {
      "reasoning_tokens": "integer"
    }
  },
  "system_fingerprint": "string (REQUIRED)"
}
```

### 回复参数字段详解

*   **`id`** (string, **REQUIRED**)
    *   该对话的唯一标识符。
*   **`object`** (string, **REQUIRED**)
    *   对象类型，值为 `chat.completion`。
*   **`created`** (integer, **REQUIRED**)
    *   创建聊天完成时的 Unix 时间戳（秒）。
*   **`model`** (string, **REQUIRED**)
    *   生成该 completion 的模型名。
*   **`choices`** (object[], **REQUIRED**)
    *   模型生成的 completion 的选择列表。
    *   **`index`**: 该 completion 在列表中的索引。
    *   **`finish_reason`**: 模型停止生成 token 的原因。
        *   可选值：`stop` (自然停止), `length` (达到长度限制), `content_filter` (内容过滤), `tool_calls` (调用工具), `insufficient_system_resource` (资源不足)。
    *   **`message`** (object, **REQUIRED**):
        *   `role`: "assistant"。
        *   `content`: 该 completion 的内容 (Nullable)。
        *   `reasoning_content`: 仅适用于思考模式，包含最终答案之前的推理内容 (Nullable)。
        *   `tool_calls`: 模型生成的 tool 调用列表。
        *   `logprobs`: 该 choice 的对数概率信息 (Nullable)。
            *   `content`: 输出 token 的对数概率列表。
                *   包含 `token`, `logprob`, `bytes`, `top_logprobs`。
            *   `reasoning_content`: 推理过程 token 的对数概率列表 (Nullable)。
                *   包含 `token`, `logprob`, `bytes`。
*   **`usage`** (object)
    *   该对话补全请求的用量信息。
    *   `completion_tokens`: 模型 completion 产生的 token 数。
    *   `prompt_tokens`: 用户 prompt 所包含的 token 数 (= hit + miss)。
    *   `prompt_cache_hit_tokens`: 命中上下文缓存的 token 数。
    *   `prompt_cache_miss_tokens`: 未命中上下文缓存的 token 数。
    *   `total_tokens`: 该请求中所有 token 的数量 (prompt + completion)。
    *   `completion_tokens_details`:
        *   `reasoning_tokens`: 推理模型所产生的思维链 token 数量。
*   **`system_fingerprint`** (string, **REQUIRED**)
    *   代表模型运行的后端配置指纹。
