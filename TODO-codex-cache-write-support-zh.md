# Codex cache write 支持：核查与实现结果

核查日期：2026-09-07。解析、统计与计费支持已实现。

## 1. 已核实的信息

本机 9 个 Codex CLI `0.153.4` 会话文件中发现 469 条 `token_count` 记录，
`total_token_usage` 和 `last_token_usage` 都包含 `cache_write_input_tokens`，
观测值均为 `0`。这些日志证明字段存在；非零行为通过合成的同格式测试验证。

Codex 上游提交 [Track prompt cache write token usage](https://github.com/openai/codex/commit/2edad72de3e4fb12a7519027d5eb3cbda45eea6c)
将 Responses `input_tokens_details.cache_write_tokens` 映射为协议字段
`TokenUsage.cache_write_input_tokens`，该字段缺失时按上游定义为 `0`。

[OpenAI Prompt Caching 文档](https://developers.openai.com/api/docs/guides/prompt-caching)
明确区分普通输入、缓存读取和缓存写入，计算口径为：

```text
ordinary_input_tokens = input_tokens - cached_tokens - cache_write_tokens
```

本地 Tokenx LiteLLM 定价缓存已有 GPT 系列缓存写入价格，例如
`gpt-5.6-sol` 的 `cache_creation_input_token_cost = 0.000005`，即 $5/百万
Token。具体计费继续使用所选目录或自定义价格，不在解析器中固定单价。

## 2. 已完成的改造

`crates/tokenx-engine/src/integrations/codex/decode.rs` 读取
`cache_write_input_tokens` 并保存到 `TokenBreakdown.cache_write`。普通输入同时
扣除缓存读取和写入量；缓存读取仍取现有两个读取字段的较大值。

单次计费量继续来自 `last_token_usage`。累计快照中的写入量参与相等性判断、
单调性判断、陈旧快照识别、fork 继承基线和跨文件去重。累计快照可能因上下文
压缩而改写，因此不能直接用累计差值代替单次用量。仅含 cache write 的记录
也会保留。

负数、读写之和超过输入量、计数求和溢出都会进入现有 malformed-record 诊断。
不使用 `.max(0)` 将无效数据变成正常结果，也不增加未经证实的
`cache_creation_input_tokens` / `cache_write_tokens` 日志字段别名。

现有聚合层、CLI JSON、TUI 和定价层已支持 `cache_write`，现在能够接收 Codex
传来的非零值。CLI 回归案例验证 `input=200`、`cacheRead=2000`、
`cacheWrite=400`、`output=100`，按测试中的分桶单价得到 `$0.0056`，冷解析和
缓存读取结果一致。

benchmark 生成器已增加写入字段及其累计值，并确保缓存读取与写入之和不超过
输入量。原生成器的缓存读取量可能大于输入量，也已在生成源头修正。

## 3. 缓存与验证

Codex decoder 源码参与 decoder contract 指纹。此次修改使旧 input-record
shard 及其追加解析状态在下次 acquisition 中失效，并重新解析会话；无需手动
清理缓存或兼容旧的内部解析状态。

回归覆盖非零写入、字段缺失、纯写入、累计快照去重、陈旧快照与重置、fork
继承、序列化后的追加解析、非法计数及 CLI 分桶计费。格式检查与 Clippy 均通过；
完整 Rust 测试为 2216 项通过、5 项忽略。另生成 80 条 Codex benchmark 记录，
逐条校验累计值和读写约束，并与 CLI 实际输出逐桶对账通过。

维护中的字段依据见 [Codex token usage](docs/facts/codex.md)，产品输入和计费
说明已同步到 [clients](docs/clients.md) 与 [pricing](docs/pricing.md)。
