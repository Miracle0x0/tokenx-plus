# Tokenx

[English](README.md) | [简体中文](README.zh-cn.md)

> 本地 AI 编码客户端用量统计，强调明确的数据语义，以及在大型 transcript 集合上的可预测资源占用。

Tokenx 从本地 AI 编码客户端读取带 token 信息的记录，并通过交互式 TUI 或确定性的 CLI 输出展示结果。本地 transcript 和数据库始终留在当前机器上；只有可选的 Subscription 视图会访问供应商账户服务。

## 安装

```bash
npm install --global @juya-ai/tokenx
tokenx
```

npm 预构建包支持 Apple 芯片 Mac、使用 glibc 的 Linux x64，以及 Windows x64。其他目标可使用 Bun 和稳定 Rust 工具链从源码构建：

```bash
git clone https://github.com/makoMakoGo/tokenx.git
cd tokenx
bun install --frozen-lockfile
bun run build
bun run cli
```

## 使用

```bash
# 交互式 TUI
tokenx
tokenx tui --tab models

# 适合脚本的用量投影
tokenx models --no-spinner
tokenx models --json --no-spinner
tokenx models --client codex --group-by client,provider,model --no-spinner

# 日期过滤
tokenx tui --client opencode,claude --week
tokenx models --since 2026-01-01 --until 2026-01-31 --no-spinner

# 定价目录
tokenx pricing lookup claude-sonnet-4-5 --no-spinner
tokenx pricing overrides --json
```

从源码运行时，把 `tokenx` 替换为 `bun run cli --`。

## 行为边界

- **本地用量留在本地。** Token 统计只从已接受的本地记录派生；供应商花费、积分、余额和只有金额的记录不会混入本地 token 成本。
- **失败保持可见。** 无法读取的输入、被拒绝的记录、未知归属和无法匹配的价格会作为错误或 Data Health 诊断显示，而不是伪装成成功。
- **身份保持稳定。** 同一份生成式客户端 catalog 定义命令 ID、展示名称、integration 绑定、缓存和投影。
- **视图共享同一 generation。** CLI Models 与 TUI 视图投影同一份不可变采集结果；切换视图不会重新扫描输入。
- **缓存可随时丢弃。** 输入 shard 和 generation cache 只用于加速读取，不会替代权威客户端数据。

## 支持的客户端

Tokenx 支持 OpenCode、Claude Code、Codex、Gemini CLI、Amp 等多种编码客户端的本地数据。可执行 catalog 位于 `crates/tokenx-engine/client-catalog.json`；完整当前列表、平台路径、schema 边界和 Data Health 行为见[支持的客户端和数据位置](docs/clients.md)。

## 定价

Tokenx 在分组和定价前会规范化模型 ID。它先检查精确自定义覆盖，再检查已配置公共目录中的精确匹配；不会按前缀、子串或模糊匹配猜测价格。无法定价的模型会保留 token 用量，并把派生成本报告为 `$0.00`。

目录优先级、只有总量的 token 分配方式和成本边界见[定价语义](docs/pricing.md)。

## 文档

- [支持的客户端和数据位置](docs/clients.md)
- [CLI 用法](docs/cli.md)
- [配置](docs/configuration.md)
- [定价语义](docs/pricing.md)
- [开发和测试](docs/development.md)
- [发布流程](docs/releases.md)
- [架构决策](docs/adr/README.md)

## 许可和署名

Tokenx 起源于 Junho Yeo 的 [Tokscale](https://github.com/junhoyeo/tokscale)，并保留其 MIT 署名。见 [LICENSE](LICENSE) 和 [NOTICE](NOTICE)。
