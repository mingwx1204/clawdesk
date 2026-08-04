/**
 * 对话自动保存 — 每次对话完成后导出到本地目录
 */
import type { Conversation, ChatMessage } from '../types';

export async function autoSaveConversation(
  conv: Conversation | undefined,
  messages: ChatMessage[],
  saveDir: string,
): Promise<string | null> {
  if (!conv || messages.length < 2 || !saveDir) return null;

  try {
    // 构建导出数据
    const exportData = {
      exportedAt: new Date().toLocaleString('zh-CN'),
      conversation: {
        id: conv.id,
        title: conv.title,
        personaId: conv.personaId,
        createdAt: new Date(conv.createdAt).toLocaleString('zh-CN'),
        updatedAt: new Date(conv.updatedAt).toLocaleString('zh-CN'),
      },
      messageCount: messages.length,
      messages: messages.map((m) => ({
        role: m.role,
        content: m.content,
        reasoning: m.reasoning || undefined,
        createdAt: new Date(m.createdAt).toLocaleString('zh-CN'),
      })),
    };

    const timestamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
    const safeTitle = (conv.title || '对话').replace(/[\\/:*?"<>|]/g, '_').slice(0, 40);
    const filename = `${timestamp}_${safeTitle}.json`;
    const json = JSON.stringify(exportData, null, 2);

    // 浏览器 dev 模式：仅控制台提示，不弹下载窗口
    if (typeof window !== 'undefined' && !('__TAURI_INTERNALS__' in window)) {
      console.log(`[自动保存] ${filename} (${messages.length} 条消息) — 浏览器模式不写入磁盘`);
      return null;
    }

    // Tauri 模式：写入文件系统
    try {
      const { writeTextFile, exists, mkdir } = await import('@tauri-apps/plugin-fs');
      const { join } = await import('@tauri-apps/api/path');
      const dirExists = await exists(saveDir);
      if (!dirExists) {
        await mkdir(saveDir, { recursive: true });
      }
      const filePath = await join(saveDir, filename);
      await writeTextFile(filePath, json);
      return `${filePath} (${messages.length} 条消息)`;
    } catch {
      return null; // 静默失败
    }
  } catch (err) {
    console.error('autoSaveConversation error:', err);
    return null;
  }
}
