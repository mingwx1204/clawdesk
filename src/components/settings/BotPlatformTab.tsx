/**
 * 内置 ClawDesk Bot 平台控制台
 * 无需外部服务器，ClawDesk 自包含多平台 Bot 引擎
 */
import { useState, useEffect, useRef } from 'react';
import { Power, Globe, Copy, Check, Settings2, Activity, Zap, Loader2, ScanLine } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Label } from '@/components/ui/label';
import { Badge } from '@/components/ui/badge';
import { useSettingsStore } from '@/store/useSettingsStore';
import { notifySuccess, notifyError } from '@/lib/notify';
import {
  botServerStart,
  botServerStop,
  generateQrSvg,
  wechatBotStart,
  wechatBotStatus,
  wechatGetQr,
  wechatQrStatus,
  wechatVerifyCode,
  wechatRefreshQr,
  wechatLogout,
} from '@/lib/backend';
import type { BotPlatform } from '@/types';

/** 平台图标映射 */
const PLATFORM_ICONS: Record<string, string> = {
  wechat: '💬', feishu: '🐦', webhook: '🪝', dingtalk: '📌', slack: '💜', 'http-api': '🔌',
};

export function BotPlatformTab() {
  const { settings, update } = useSettingsStore();
  const { botPlatform } = settings;
  const [saving, setSaving] = useState(false);
  const [webhookPort, setWebhookPort] = useState(botPlatform.webhookPort ?? 19527);
  const [botName, setBotName] = useState(botPlatform.botName ?? 'ClawDesk');
  const [copied, setCopied] = useState(false);
  const [starting, setStarting] = useState(false);

  const isServerOn = botPlatform.enabled;

  const toggleServer = async () => {
    const next = !isServerOn;
    if (next) {
      setStarting(true);
      try {
        const result = await botServerStart(botPlatform);
        update({ botPlatform: { ...botPlatform, enabled: true } });
        notifySuccess(`🚀 Bot 引擎已启动 · 端口 ${result.port}`);
      } catch (e: any) {
        notifyError(`启动失败: ${e}`);
      }
      setStarting(false);
    } else {
      try {
        await botServerStop();
        update({ botPlatform: { ...botPlatform, enabled: false } });
        notifySuccess('⏹️ Bot 引擎已停止');
      } catch (e: any) {
        notifyError(`停止失败: ${e}`);
      }
    }
  };

  const togglePlatform = (platformId: string) => {
    const platforms = botPlatform.platforms.map((p) =>
      p.id === platformId ? { ...p, enabled: !p.enabled } : p,
    );
    update({ botPlatform: { ...botPlatform, platforms } });
  };

  const saveConfig = () => {
    setSaving(true);
    update({
      botPlatform: { ...botPlatform, webhookPort, botName },
    });
    setTimeout(() => { setSaving(false); notifySuccess('配置已保存'); }, 300);
  };

  const copyWebhookUrl = () => {
    const url = `http://localhost:${webhookPort}/webhook`;
    navigator.clipboard.writeText(url).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    });
  };

  const activePlatforms = botPlatform.platforms.filter((p) => p.enabled);
  const connectedCount = botPlatform.platforms.filter((p) => p.connected).length;

  return (
    <div className="space-y-5">
      {/* ─── 引擎状态卡片 ─── */}
      <div className={`rounded-xl border-2 p-4 transition-colors ${isServerOn ? 'border-green-500/40 bg-green-500/5' : 'border-border bg-card'}`}>
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className={`flex h-12 w-12 items-center justify-center rounded-xl ${isServerOn ? 'bg-green-500/20' : 'bg-muted'}`}>
              <Power className={`h-6 w-6 ${isServerOn ? 'text-green-400' : 'text-muted-foreground'}`} />
            </div>
            <div>
              <p className="font-semibold">ClawDesk Bot 引擎</p>
              <p className="text-xs text-muted-foreground">
                {isServerOn
                  ? `${activePlatforms.length} 个平台激活 · ${connectedCount} 个已连接`
                  : '内置多平台消息引擎，无需外部服务器'}
              </p>
            </div>
          </div>
          <Button
            variant={isServerOn ? 'destructive' : 'default'}
            size="sm"
            className="gap-1.5"
            disabled={starting}
            onClick={toggleServer}
          >
            {starting ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Power className="h-3.5 w-3.5" />}
            {starting ? '启动中…' : isServerOn ? '停止' : '启动'}
          </Button>
        </div>

        {isServerOn && (
          <div className="mt-3 flex items-center gap-2 text-xs text-muted-foreground">
            <Activity className="h-3 w-3 text-green-400" />
            <span>运行中 · 端口 {webhookPort}</span>
            <span className="mx-1">·</span>
            <span>本地地址 </span>
            <code className="rounded bg-muted px-1.5 py-0.5 text-[11px]">http://localhost:{webhookPort}</code>
          </div>
        )}
      </div>

      {/* ─── 平台列表 ─── */}
      <div>
        <div className="mb-3 flex items-center justify-between">
          <h3 className="text-sm font-semibold">消息平台</h3>
          <span className="text-[11px] text-muted-foreground">{activePlatforms.length}/{botPlatform.platforms.length} 启用</span>
        </div>
        <div className="grid gap-2">
          {botPlatform.platforms.map((platform) => (
            <PlatformCard
              key={platform.id}
              platform={platform}
              isServerOn={isServerOn}
              onToggle={() => togglePlatform(platform.id)}
              webhookPort={webhookPort}
            />
          ))}
        </div>
      </div>

      {/* ─── Webhook 配置 ─── */}
      <div className="rounded-xl border border-border bg-card p-4">
        <h3 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <Globe className="h-4 w-4" /> Webhook 接入
        </h3>
        <p className="mb-3 text-xs text-muted-foreground">
          外部服务通过 Webhook 将消息推送到 ClawDesk，AI 自动处理并回复。
        </p>
        <div className="flex items-center gap-2">
          <code className="flex-1 rounded-lg bg-muted px-3 py-2 text-xs">
            http://localhost:{webhookPort}/webhook
          </code>
          <Button variant="outline" size="sm" className="gap-1" onClick={copyWebhookUrl}>
            {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
            {copied ? '已复制' : '复制'}
          </Button>
        </div>
        {isServerOn && (
          <div className="mt-3 rounded-lg border border-dashed border-amber-500/40 bg-amber-500/5 p-3">
            <p className="text-[11px] text-amber-400">
              ⚠️ 仅本地访问。如需公网接入，请使用 ngrok 或 Cloudflare Tunnel 将端口 {webhookPort} 暴露到公网。
            </p>
          </div>
        )}
      </div>

      {/* ─── 高级设置 ─── */}
      <div className="rounded-xl border border-border bg-card p-4">
        <h3 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <Settings2 className="h-4 w-4" /> 高级设置
        </h3>
        <div className="space-y-3">
          <div className="flex items-center justify-between">
            <div>
              <Label className="text-sm">Bot 名称</Label>
              <p className="text-[11px] text-muted-foreground">AI 回复时使用的身份标识</p>
            </div>
            <Input
              className="w-40"
              value={botName}
              onChange={(e) => setBotName(e.target.value)}
            />
          </div>
          <div className="flex items-center justify-between">
            <div>
              <Label className="text-sm">Webhook 端口</Label>
              <p className="text-[11px] text-muted-foreground">内置 HTTP 服务器监听端口</p>
            </div>
            <Input
              type="number"
              className="w-32"
              value={webhookPort}
              onChange={(e) => setWebhookPort(Number(e.target.value))}
            />
          </div>
          <Button variant="secondary" size="sm" disabled={saving} onClick={saveConfig}>
            {saving ? '保存中…' : '保存配置'}
          </Button>
        </div>
      </div>

      {/* ─── 使用说明 ─── */}
      <div className="rounded-xl border border-dashed border-border p-4 text-xs leading-relaxed text-muted-foreground">
        <p className="mb-2 flex items-center gap-1.5 font-semibold text-foreground">
          <Zap className="h-3.5 w-3.5 text-yellow-400" /> 内置 Bot 使用说明
        </p>
        <ol className="list-inside list-decimal space-y-1">
          <li>点击上方「启动」开启 Bot 引擎</li>
          <li>在「消息平台」中启用需要的平台（微信 / 飞书 / Webhook 等）</li>
          <li>微信/飞书：将 Webhook URL 配置到对应的公众号/机器人后台</li>
          <li>外部消息到达 → AI 自动分析 → 智能回复 → 推回消息平台</li>
          <li>可在「插件商店 → SkillHub」安装社区技能增强 Bot 能力</li>
        </ol>
      </div>
    </div>
  );
}

/** 单个平台卡片 */
function PlatformCard({
  platform, isServerOn, onToggle, webhookPort,
}: {
  platform: BotPlatform;
  isServerOn: boolean;
  onToggle: () => void;
  webhookPort: number;
}) {
  const [showDetail, setShowDetail] = useState(false);
  // ── 微信 iLink 扫码登录状态 ──
  const [wechatState, setWechatState] = useState<'idle' | 'qr' | 'waiting' | 'scanned' | 'need_verifycode' | 'logged_in' | 'error'>('idle');
  const [qrSvg, setQrSvg] = useState('');
  const [qrUrl, setQrUrl] = useState('');
  const [loginMsg, setLoginMsg] = useState('');
  const [verifyCode, setVerifyCode] = useState('');
  const [msgCount, setMsgCount] = useState(0);
  const [connected, setConnected] = useState(false);
  const pollRef = useRef(false);

  // 微信平台启用时：轮询 Bot 状态（登录态 / 消息计数）
  useEffect(() => {
    if (platform.id !== 'wechat' || !platform.enabled || !isServerOn) {
      setWechatState('idle');
      setQrSvg('');
      setQrUrl('');
      return;
    }
    let stopped = false;
    const check = async () => {
      if (stopped) return;
      try {
        const s = await wechatBotStatus();
        if (stopped) return;
        setMsgCount(s.messageCount);
        setConnected(s.connected);
        if (s.loggedIn) {
          setWechatState((prev) => (prev === 'idle' ? 'logged_in' : prev));
        }
      } catch { /* 未就绪 */ }
    };
    check();
    const timer = setInterval(check, 3000);
    return () => { stopped = true; clearInterval(timer); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [platform.id, platform.enabled, isServerOn]);

  // 组件卸载时停止轮询
  useEffect(() => () => { pollRef.current = false; }, []);

  // 有二维码 URL 时生成 SVG
  useEffect(() => {
    if (qrUrl) {
      generateQrSvg(qrUrl).then(setQrSvg).catch(() => setQrSvg(''));
    }
  }, [qrUrl]);

  /** 开始扫码登录 */
  const startLogin = async () => {
    pollRef.current = false;
    setWechatState('qr');
    setLoginMsg('');
    setVerifyCode('');
    setQrSvg('');
    try {
      const qr = await wechatGetQr();
      setQrUrl(qr.qrcodeUrl);
      pollRef.current = true;
      void pollQr();
    } catch (e: any) {
      setWechatState('error');
      setLoginMsg(String(e?.message ?? e ?? '获取二维码失败'));
    }
  };

  /** 轮询扫码状态（循环直到登录成功 / 需配对码 / 失败） */
  const pollQr = async () => {
    while (pollRef.current) {
      try {
        const res = await wechatQrStatus();
        const status = String(res.status ?? 'wait');
        if (status === 'confirmed') {
          pollRef.current = false;
          setWechatState('logged_in');
          setConnected(true);
          setQrUrl('');
          setQrSvg('');
          setLoginMsg('');
          return;
        }
        if (status === 'scanned') {
          setWechatState('scanned');
        } else if (status === 'need_verifycode') {
          setWechatState('need_verifycode');
        } else if (status === 'verify_code_blocked') {
          pollRef.current = false;
          setWechatState('error');
          setLoginMsg('配对码多次错误，二维码已刷新，请重新扫码');
          await refreshQr();
          return;
        } else if (status === 'expired') {
          await refreshQr();
        } else if (status === 'wait' && wechatState === 'qr') {
          setWechatState('waiting');
        }
      } catch (e: any) {
        const msg = String(e?.message ?? e ?? '');
        if (msg.includes('先生成')) {
          // 二维码丢失（如已过期清理）→ 自动重新获取
          try {
            const qr = await wechatGetQr();
            setQrUrl(qr.qrcodeUrl);
          } catch { /* ignore */ }
        } else {
          setLoginMsg(msg);
        }
      }
      await new Promise((r) => setTimeout(r, 1000));
    }
  };

  /** 刷新二维码（过期 / 配对码失败后） */
  const refreshQr = async () => {
    try {
      const qr = await wechatRefreshQr();
      setQrUrl(qr.qrcodeUrl);
      setWechatState('waiting');
      setVerifyCode('');
    } catch (e: any) {
      setLoginMsg(String(e?.message ?? e ?? '刷新二维码失败'));
    }
  };

  /** 提交手机微信显示的配对码 */
  const submitVerify = async () => {
    if (!verifyCode.trim()) return;
    try {
      await wechatVerifyCode(verifyCode.trim());
      setVerifyCode('');
      setWechatState('waiting');
      pollRef.current = true;
      void pollQr();
    } catch (e: any) {
      setLoginMsg(String(e?.message ?? e ?? '提交失败'));
    }
  };

  /** 登出微信 */
  const logout = async () => {
    pollRef.current = false;
    await wechatLogout().catch(() => {});
    setWechatState('idle');
    setConnected(false);
    setQrUrl('');
    setQrSvg('');
    setMsgCount(0);
    setLoginMsg('');
  };

  /** 已登录但未连接时重新连接 */
  const reconnect = async () => {
    setLoginMsg('');
    try {
      await wechatBotStart({ apiBase: '', token: '', botName: 'ClawBot', pollIntervalSecs: 10 } as never);
      setConnected(true);
    } catch (e: any) {
      setLoginMsg(String(e?.message ?? e ?? '连接失败'));
    }
  };

  const statusLabel: Record<string, { text: string; color: string }> = {
    waiting: { text: '等待扫码', color: 'text-amber-400' },
    scanned: { text: '已扫码，请在手机上确认', color: 'text-blue-400' },
    need_verifycode: { text: '请输入手机显示的配对码', color: 'text-blue-400' },
    logged_in: { text: '已连接', color: 'text-green-400' },
    error: { text: '登录失败', color: 'text-red-400' },
  };

  return (
    <div className={`rounded-lg border p-3 transition-colors ${platform.enabled && isServerOn ? 'border-primary/30 bg-primary/5' : 'border-border/50 bg-card hover:bg-accent/30'}`}>
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <span className="text-xl">{PLATFORM_ICONS[platform.id] ?? '🔧'}</span>
          <div>
            <p className="text-sm font-medium">{platform.name}</p>
            <p className="text-[11px] text-muted-foreground">{platform.description}</p>
          </div>
        </div>
        <div className="flex items-center gap-2">
          {isServerOn && platform.enabled && wechatState === 'logged_in' && (
            <Badge variant="default" className="h-5 bg-green-500/20 text-[10px] text-green-400">{connected ? '已连接' : '已登录'}</Badge>
          )}
          {isServerOn && platform.enabled && platform.id === 'wechat' && ['waiting', 'scanned', 'need_verifycode'].includes(wechatState) && (
            <Badge variant="default" className="h-5 bg-amber-500/20 text-[10px] text-amber-400">待扫码</Badge>
          )}
          <Switch checked={platform.enabled && isServerOn} disabled={!isServerOn} onCheckedChange={onToggle} />
        </div>
      </div>

      {/* 微信扫码登录区 — 腾讯 iLink Bot 官方 API，纯 Rust 实现 */}
      {platform.id === 'wechat' && platform.enabled && isServerOn && (
        <div className="mt-3 space-y-3 border-t border-border/50 pt-3">

          {/* 未登录 → 扫码登录按钮 */}
          {wechatState === 'idle' && (
            <div className="flex items-center justify-between gap-3">
              <p className="text-xs text-muted-foreground">
                使用手机微信扫码登录，ClawDesk 即可收发你的个人微信消息（官方 iLink Bot，免费）
              </p>
              <Button size="sm" className="gap-1.5 shrink-0" onClick={startLogin}>
                <ScanLine className="h-3.5 w-3.5" /> 扫码登录
              </Button>
            </div>
          )}

          {/* 已登录 → 连接状态 + 登出 */}
          {wechatState === 'logged_in' && (
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <div className="space-y-1 text-xs">
                  <p className="flex items-center gap-1.5 font-medium text-green-400">
                    <span className={`inline-block h-2 w-2 rounded-full ${connected ? 'bg-green-400' : 'bg-amber-400'}`} />
                    {connected ? '✅ 微信已连接' : '已登录，未连接'}
                  </p>
                  <p className="text-muted-foreground">收到 {msgCount} 条消息</p>
                  <p className="text-muted-foreground">别人给你发微信 → AI 自动回复</p>
                </div>
                <div className="flex flex-col items-end gap-2">
                  {!connected && (
                    <Button size="sm" variant="outline" onClick={reconnect}>重新连接</Button>
                  )}
                  <Button size="sm" variant="ghost" className="text-red-400 hover:text-red-300" onClick={logout}>
                    退出登录
                  </Button>
                </div>
              </div>
            </div>
          )}

          {/* 扫码中 / 等待扫码 / 已扫码 → 显示二维码 */}
          {['qr', 'waiting', 'scanned'].includes(wechatState) && (
            <div className="flex items-start gap-4">
              {qrSvg ? (
                <div
                  className="shrink-0 overflow-hidden rounded-lg border-2 border-primary/30 bg-white p-2"
                  style={{ width: 170, height: 170 }}
                >
                  {/* SVG 带 viewBox，设置 100% 自适应容器，避免被裁剪 */}
                  <div
                    className="h-full w-full [&_svg]:h-full [&_svg]:w-full"
                    dangerouslySetInnerHTML={{ __html: qrSvg }}
                  />
                </div>
              ) : (
                <div className="flex h-[170px] w-[170px] shrink-0 items-center justify-center rounded-lg border-2 border-dashed border-border bg-muted">
                  <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
                </div>
              )}
              <div className="min-w-0 space-y-2 text-xs">
                <p className="flex items-center gap-1 font-medium text-foreground">
                  <ScanLine className="h-3.5 w-3.5" /> 扫码登录微信
                </p>
                <p className={`font-medium ${statusLabel[wechatState]?.color}`}>
                  {statusLabel[wechatState]?.text}
                </p>
                <ol className="list-inside list-decimal space-y-1 text-muted-foreground">
                  <li>打开手机微信「扫一扫」</li>
                  <li>扫描左侧二维码</li>
                  <li>在手机上点「确认登录」</li>
                  <li>登录成功即连上 ClawDesk AI</li>
                </ol>
                {loginMsg && <p className="text-amber-400">{loginMsg}</p>}
                <div className="flex gap-2 pt-1">
                  <Button size="sm" variant="outline" onClick={refreshQr}>刷新二维码</Button>
                  <Button size="sm" variant="ghost" onClick={() => { pollRef.current = false; setWechatState('idle'); setQrUrl(''); setQrSvg(''); }}>
                    取消
                  </Button>
                </div>
              </div>
            </div>
          )}

          {/* 需要配对码 → 输入手机显示的 6 位数字 */}
          {wechatState === 'need_verifycode' && (
            <div className="flex items-center gap-3 rounded-lg border border-dashed border-primary/40 bg-primary/5 p-3">
              <div className="min-w-0 flex-1 space-y-1">
                <p className="text-xs font-medium text-foreground">🔢 输入配对码</p>
                <p className="text-[11px] text-muted-foreground">手机微信会显示一串数字，输入后继续连接</p>
              </div>
              <Input
                className="w-32 text-center tracking-widest"
                placeholder="配对码"
                value={verifyCode}
                onChange={(e) => setVerifyCode(e.target.value.replace(/\D/g, ''))}
                onKeyDown={(e) => e.key === 'Enter' && submitVerify()}
              />
              <Button size="sm" onClick={submitVerify} disabled={!verifyCode.trim()}>确认</Button>
            </div>
          )}

          {/* 登录失败 */}
          {wechatState === 'error' && (
            <div className="flex items-start gap-3 rounded-lg border border-dashed border-red-500/40 bg-red-500/5 p-3">
              <span className="mt-0.5 text-red-400">⚠️</span>
              <div className="space-y-1 text-xs">
                <p className="font-medium text-red-400">{loginMsg || '登录失败'}</p>
                <button className="text-primary hover:underline" onClick={startLogin}>
                  重新扫码登录
                </button>
              </div>
            </div>
          )}
        </div>
      )}

      {/* 其他平台展开详情 */}
      {platform.enabled && isServerOn && showDetail && platform.id !== 'wechat' && (
        <div className="mt-3 space-y-2 border-t border-border/50 pt-3">
          {platform.id === 'webhook' && (
            <div className="flex items-center gap-2 text-[11px]">
              <span className="text-muted-foreground">Webhook URL:</span>
              <code className="rounded bg-muted px-1.5 py-0.5">http://localhost:{webhookPort}/webhook/{platform.id}</code>
            </div>
          )}
          {platform.id === 'feishu' && (
            <div className="space-y-2">
              <p className="text-[11px] text-muted-foreground">
                在飞书开放平台创建企业自建应用 → 事件订阅 → 配置请求网址。
              </p>
              <div className="flex items-center gap-2 text-[11px]">
                <span className="text-muted-foreground">事件回调:</span>
                <code className="rounded bg-muted px-1.5 py-0.5">http://YOUR_DOMAIN:{webhookPort}/webhook/feishu</code>
              </div>
            </div>
          )}
          {platform.id === 'dingtalk' && (
            <div className="space-y-2">
              <p className="text-[11px] text-muted-foreground">
                在钉钉开放平台创建机器人 → 消息接收模式选 HTTP。
              </p>
              <div className="flex items-center gap-2 text-[11px]">
                <span className="text-muted-foreground">消息接收:</span>
                <code className="rounded bg-muted px-1.5 py-0.5">http://YOUR_DOMAIN:{webhookPort}/webhook/dingtalk</code>
              </div>
            </div>
          )}
          {platform.id === 'http-api' && (
            <div className="space-y-2">
              <p className="text-[11px] text-muted-foreground">直接 POST JSON 到 API 端点调用 ClawDesk：</p>
              <pre className="overflow-auto rounded bg-muted p-2 text-[11px]">{`POST http://localhost:${webhookPort}/api/chat
Content-Type: application/json

{ "message": "你好", "model": "auto" }`}</pre>
            </div>
          )}
          {platform.id === 'slack' && (
            <div className="space-y-2">
              <p className="text-[11px] text-muted-foreground">
                在 Slack API 创建 Bot → Event Subscriptions → 填入 Request URL。
              </p>
              <div className="flex items-center gap-2 text-[11px]">
                <span className="text-muted-foreground">Request URL:</span>
                <code className="rounded bg-muted px-1.5 py-0.5">http://YOUR_DOMAIN:{webhookPort}/webhook/slack</code>
              </div>
            </div>
          )}
        </div>
      )}

      {/* 展开/收起按钮（非微信平台） */}
      {platform.enabled && isServerOn && platform.id !== 'wechat' && (
        <button
          className="mt-1 text-[10px] text-primary hover:underline"
          onClick={() => setShowDetail(!showDetail)}
        >
          {showDetail ? '收起配置' : '查看配置 →'}
        </button>
      )}
    </div>
  );
}
