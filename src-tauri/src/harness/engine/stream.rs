//! SSE 流解析 —— 从 CodeWhale tui/src/client/chat.rs + core/engine/streaming.rs 剥离重构。
//!
//! 内置 BUG 修复：
//!   1. 跨 chunk 的 UTF-8 边界容错（BytesMut 拼接），杜绝 `error decoding response body`；
//!   2. 行缓冲：半行数据暂存等待续帧，不立即报错；
//!   3. 单块空闲超时（idle_timeout）驱动 Poll 返回错误而非挂死。
//!
//! 说明：本类型所有字段均满足 `Unpin`（`Pin<Box<dyn Stream>>` 是 Unpin），
//! 因此无需 pin-project 依赖，直接 `self.get_mut()` 即可。

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::Result;
use bytes::{Bytes, BytesMut};
use futures_util::Stream;
use serde_json::Value;

/// 解析后的一条 SSE 事件。
#[derive(Debug, Clone)]
pub enum SseEvent {
    TextDelta { content: String },
    ThinkingDelta { content: String },
    ToolCallStart { id: String, name: String, index: u32 },
    ToolCallDelta { id: String, arguments: String, index: u32 },
    MessageStop,
    Usage { input_tokens: u64, output_tokens: u64 },
    Error { message: String },
}

/// SSE 字节流解析器。
pub struct SseStream {
    inner: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send + 'static>>,
    /// 半行/半字节缓冲（跨 chunk 拼接）。
    leftover: BytesMut,
    /// 当前事件的 data 累积（多 data: 行拼接）。
    data_buffer: String,
    /// 当前事件类型（event: 行）。
    event_type: String,
    /// ★ 已解析但尚未被消费的事件队列。
    ///   一个大 chunk 可能含多个 SSE 事件（如 `切出 8 行`），若 poll_next 处理到
    ///   第一个事件就 return，剩余行会全部丢失 → 回答跳字。用队列暂存所有事件，
    ///   每次 poll 取一个，其余保留到下次 poll，确保不丢字。
    pending: std::collections::VecDeque<SseEvent>,
    /// 单块空闲超时。
    idle_timeout: Duration,
    /// 上一次取得数据的单调时钟。
    last_progress: std::time::Instant,
}

impl SseStream {
    pub fn new<S>(stream: S, idle_timeout: Duration) -> Self
    where
        S: Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
    {
        Self {
            inner: Box::pin(stream),
            leftover: BytesMut::new(),
            data_buffer: String::new(),
            event_type: String::new(),
            pending: std::collections::VecDeque::new(),
            idle_timeout,
            last_progress: std::time::Instant::now(),
        }
    }

    /// 从字节缓冲切出完整行；返回 (行列表, 已消费字节数)。
    fn extract_lines(buf: &[u8]) -> (Vec<String>, usize) {
        let mut lines = Vec::new();
        let mut start = 0;
        let mut consumed = 0;
        for (i, &b) in buf.iter().enumerate() {
            if b == b'\n' {
                let line = &buf[start..i];
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                // 容错：行内无效 UTF-8 用 lossy 替换，不 panic、不报错
                lines.push(String::from_utf8_lossy(line).into_owned());
                consumed = i + 1;
                start = i + 1;
            }
        }
        (lines, consumed)
    }

    /// 解析单行 SSE；累积跨行 data，空行时提交事件。
    fn process_line(&mut self, line: &str) -> Option<SseEvent> {
        let line = line.trim();
        if line.is_empty() {
            if self.data_buffer.is_empty() {
                return None;
            }
            let data = std::mem::take(&mut self.data_buffer);
            let _ = std::mem::take(&mut self.event_type);
            return parse_data_event(&data);
        }
        if line.starts_with(':') {
            return None; // 注释
        }
        if let Some(et) = line.strip_prefix("event:") {
            self.event_type = et.trim().to_string();
            return None;
        }
        if let Some(data) = line.strip_prefix("data:") {
            if !self.data_buffer.is_empty() {
                self.data_buffer.push('\n');
            }
            self.data_buffer.push_str(data.trim());
            return None;
        }
        None
    }

    /// 提交缓冲（上游关闭时调用）。
    fn flush(&mut self) -> Option<SseEvent> {
        if self.data_buffer.is_empty() {
            return None;
        }
        let data = std::mem::take(&mut self.data_buffer);
        let _ = std::mem::take(&mut self.event_type);
        parse_data_event(&data)
    }
}

/// 解析 data JSON → 事件。
fn parse_data_event(data: &str) -> Option<SseEvent> {
    if data == "[DONE]" {
        return Some(SseEvent::MessageStop);
    }
    let parsed: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(target: "engine.stream", "SSE data 解析失败(跳过): {e} → {data}");
            return None; // 容错：坏 JSON 跳过而非终止流
        }
    };

    if let Some(err) = parsed.get("error") {
        return Some(SseEvent::Error {
            message: err["message"].as_str().unwrap_or("未知错误").to_string(),
        });
    }

    let choices = parsed.get("choices")?.as_array()?;
    let first = choices.first()?;
    let delta = first.get("delta")?;

    if let Some(content) = delta.get("content").and_then(Value::as_str) {
        return Some(SseEvent::TextDelta {
            content: content.to_string(),
        });
    }
    if let Some(reasoning) = delta.get("reasoning_content").and_then(Value::as_str) {
        return Some(SseEvent::ThinkingDelta {
            content: reasoning.to_string(),
        });
    }
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        if let Some(tc) = tool_calls.first() {
            let id = tc["id"].as_str().unwrap_or("").to_string();
            // ★ index：OpenAI 流式 tool_calls 每个 chunk 带 index（并行工具按序号区分，修复参数错位）
            let index = tc["index"].as_u64().unwrap_or(0) as u32;
            if let Some(func) = tc.get("function") {
                if let Some(name) = func.get("name").and_then(Value::as_str) {
                    return Some(SseEvent::ToolCallStart {
                        id,
                        name: name.to_string(),
                        index,
                    });
                }
            }
            if let Some(args) = tc["function"]["arguments"].as_str() {
                return Some(SseEvent::ToolCallDelta {
                    id,
                    arguments: args.to_string(),
                    index,
                });
            }
        }
    }
    if let Some(usage) = parsed.get("usage") {
        return Some(SseEvent::Usage {
            input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
        });
    }
    None
}

impl Stream for SseStream {
    type Item = Result<SseEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // SseStream 字段全部 Unpin，可直接 &mut 访问
        let this = self.get_mut();

        // 空闲超时守卫
        if this.last_progress.elapsed() > this.idle_timeout {
            return Poll::Ready(Some(Err(anyhow::anyhow!(
                "SSE 流空闲超过 {}s，无数据到达",
                this.idle_timeout.as_secs()
            ))));
        }

        // 0) 先消费已解析但未返回的事件队列（★ 跳字根治：绝不因提前 return 丢行）
        if let Some(ev) = this.pending.pop_front() {
            return Poll::Ready(Some(Ok(ev)));
        }

        loop {
            // 1) 处理缓冲中的所有完整行，把所有事件 push 进 pending（不提前 return）
            if !this.leftover.is_empty() {
                let buf = std::mem::take(&mut this.leftover);
                let (lines, consumed) = Self::extract_lines(&buf);
                if consumed < buf.len() {
                    this.leftover = BytesMut::from(&buf[consumed..]);
                }
                eprintln!("[SSE] 处理缓冲 {} 字节, 切出 {} 行, 消费 {}", buf.len(), lines.len(), consumed);
                for line in lines {
                    if let Some(event) = this.process_line(&line) {
                        this.last_progress = std::time::Instant::now();
                        eprintln!("[SSE] 事件: {:?}", event);
                        this.pending.push_back(event);
                    }
                }
                // ★ 若本批产生了事件，返回第一个，其余下次 poll 再取（不丢行）
                if let Some(ev) = this.pending.pop_front() {
                    return Poll::Ready(Some(Ok(ev)));
                }
                // ★ BUG 修复：不要在这里因"半行残留"提前 return Pending！
                // 提前返回会导致 inner（reqwest）的 waker 不更新，后续数据到达时
                // wake 到旧 waker（tokio::select! 每次迭代会替换 waker），流永久挂起。
                // 正确做法：继续往下 poll inner，把外层 waker 传给 inner。
            }

            // 2) 拉取上游
            match this.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    this.last_progress = std::time::Instant::now();
                    eprintln!("[SSE] 上游收到 {} 字节: {:?}", bytes.len(), String::from_utf8_lossy(&bytes[..bytes.len().min(100)]));
                    this.leftover.extend_from_slice(&bytes);
                    // 继续循环处理新数据
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(anyhow::anyhow!("SSE 流读取错误: {e}"))));
                }
                Poll::Ready(None) => {
                    // 上游关闭：先把 pending 队列里已解析的事件全部返回，再 flush 残留
                    if let Some(ev) = this.pending.pop_front() {
                        return Poll::Ready(Some(Ok(ev)));
                    }
                    return Poll::Ready(if let Some(ev) = this.flush() {
                        Some(Ok(ev))
                    } else {
                        None
                    });
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
