import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { notifyInfo, notifyError } from '@/lib/notify';
import { CLAWDESK_VERSION } from '@/lib/version';
import { checkForUpdates, GITHUB_REPO } from '@/lib/updater';

export function AboutTab() {
  const [checking, setChecking] = useState(false);

  const checkUpdate = async () => {
    if (checking) return;
    setChecking(true);
    const r = await checkForUpdates(CLAWDESK_VERSION);
    setChecking(false);
    if (r.error) {
      notifyError(`检查更新失败：${r.error}`);
      return;
    }
    if (r.available) {
      notifyInfo(`发现新版本 v${r.latest}，正在打开下载页…`);
      setTimeout(() => window.open(r.url, '_blank'), 1200);
    } else {
      notifyInfo(`已是最新版本 v${CLAWDESK_VERSION}`);
    }
  };

  return (
    <div className="space-y-4 text-sm">
      <div className="flex items-center gap-3">
        <span className="text-3xl">🐾</span>
        <div>
          <p className="font-medium">ClawDesk</p>
          <p className="text-xs text-muted-foreground">AI 桌面秘书 · v{CLAWDESK_VERSION}</p>
        </div>
      </div>

      {/* 当前版本核心能力 */}
      <div className="space-y-1.5 rounded-lg border border-border bg-card p-3 text-xs text-muted-foreground">
        <p className="font-medium text-foreground">核心能力</p>
        <p>🧠 永久记忆 · 自动记忆 / 进化 / 语义检索（RAG）</p>
        <p>💬 微信 Bot · 扫码登录个人微信，手机随时布置任务，AI 自动回复</p>
        <p>🛠️ 智能体 · 文件读写 / 终端命令 / 联网搜索 / 多工具调用</p>
        <p>🗣️ 语音朗读 · 系统多音色 TTS（跟随默认音频设备）</p>
        <p>📱 手机桥接 · 局域网扫码，手机端同步对话</p>
        <p>💾 本地存储 · 对话 / 记忆全量保存于 SQLite，开机自动恢复</p>
      </div>

      <p className="text-muted-foreground">技术栈：Tauri 2.0 + Rust + React 18 + TypeScript + Zustand + Tailwind CSS + SQLite</p>
      <p className="text-muted-foreground">微信接入：腾讯 iLink Bot API（官方协议，扫码登录，无需第三方服务）</p>
      <p className="text-muted-foreground">开源协议：MIT</p>

      <div className="flex gap-2">
        <Button variant="outline" size="sm" onClick={() => window.open(`https://github.com/${GITHUB_REPO}`, '_blank')}>
          官网 / 仓库
        </Button>
        <Button variant="outline" size="sm" disabled={checking} onClick={checkUpdate}>
          {checking ? '检查中…' : '检查更新'}
        </Button>
      </div>
    </div>
  );
}
