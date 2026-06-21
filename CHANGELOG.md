# Changelog

## [0.1.11] - 2026-06-21

### wfgen — 使用 wp-core-connectors TcpArrowSink 发送数据

- **依赖**：添加 `wp-core-connectors`、`wp-connector-api`、`tokio` 依赖
- **重构**：`tcp_send.rs` 从原始 `TcpStream` + 手动 Arrow IPC 编码 → `TcpArrowSink::connect()` + `encode_batch_payload_with_tag()` + `send_payload()`
  - Arrow IPC 编码：使用 `encode_ipc_frame`（与 `wp_arrow::ipc::encode_ipc` 兼容）
  - Framing：RFC6587 octet-counted（`<len> <payload>`），匹配 wfusion `tcp_src` 的 `framing = "len"`
  - 传输层：`NetWriter` 带背压控制
- **异步化**：`cmd_stream`、`cmd_send`、`cmd_bench`、`cmd_gen` 全部改为 `async fn`
- **依赖升级**：`wp-core-connectors` 0.5.2 → 0.5.5（含 `encode_batch_payload_with_tag` 公开 API）
