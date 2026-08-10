import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Camera, ChevronDown, Paperclip, SendHorizonal, Square, X, Shield, ShieldCheck, Library, Plus, Volume2, VolumeX } from 'lucide-react';
import { useChatStore } from '@/store/useChatStore';
import { useSettingsStore } from '@/store/useSettingsStore';
import { captureScreen } from '@/lib/backend';
import { notifyError } from '@/lib/notify';
import { playKeypress, playSend } from '@/lib/sound';
import { getIsSpeaking } from '@/lib/tts';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import type { Attachment, ChatMessage, Persona, ReasoningMode } from '@/types';
import { contextWindowFor, estimateTokens, formatTokens } from '@/lib/tokens';
import { cn } from '@/lib/utils';

const MODES: { id: ReasoningMode; label: string; desc: string }[] = [
  { id: 'fast', label: '快速', desc: '低 tokens · 极速响应' },
  { id: 'standard', label: '标准', desc: '平衡质量与速度' },
  { id: 'deep', label: '深度思考', desc: '长思考链 · 适合复杂任务' },
];

/** 底部圆角卡片输入区 */
export const InputBox = memo(function InputBox() {
  const { send, generating, stopGenerating, currentPersonaId, personas, savePersona, currentConvId, setConversationModel, messages } = useChatStore();
  const { settings, allModels } = useSettingsStore();
  const [text, setText] = useState('');
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [listening, setListening] = useState(false);
  const [showTemplates, setShowTemplates] = useState(false);
  const [toolsOpen, setToolsOpen] = useState(false);
  const recognitionRef = useRef<any>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const persona = personas.find((p) => p.id === currentPersonaId);
  const [modelId, setModelId] = useState(persona?.modelId || settings.defaultModelId);
  const [mode, setMode] = useState<ReasoningMode>(persona?.mode ?? settings.defaultMode);

  useEffect(() => {
    setModelId(persona?.modelId || settings.defaultModelId);
    setMode(persona?.mode ?? settings.defaultMode);
  }, [persona?.modelId, persona?.mode, settings.defaultModelId, settings.defaultMode]);

  // textarea 自适应高度：最多 5 行
  const autoResize = useCallback(() => {
    const ta = taRef.current;
    if (!ta) return;
    ta.style.height = 'auto';
    const lineH = 22;
    ta.style.height = `${Math.min(ta.scrollHeight, lineH * 5 + 16)}px`;
    ta.style.overflowY = ta.scrollHeight > lineH * 5 + 16 ? 'auto' : 'hidden';
  }, []);

  useEffect(autoResize, [text, autoResize]);

  const doSend = useCallback(() => {
    const v = text.trim();
    if (!v && attachments.length === 0) return;
    // 把当前选择的模型/模式持久化到分身，并把模型同步到当前对话
    if (persona && (persona.modelId !== modelId || persona.mode !== mode)) {
      void savePersona({ ...persona, modelId, mode });
    }
    if (currentConvId) void setConversationModel(currentConvId, modelId);
    void send(v, attachments.length ? attachments : undefined);
    setText('');
    setAttachments([]);
  }, [text, attachments, send, persona, modelId, mode, savePersona, currentConvId, setConversationModel]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    const soundOn = useSettingsStore.getState().settings.soundEnabled;
    if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      if (soundOn) playSend();
      doSend();
    } else if (e.key.length === 1 && soundOn && !getIsSpeaking()) {
      playKeypress();
    }
  };

  // 粘贴处理：支持粘贴图片/文件
  const onPaste = useCallback((e: React.ClipboardEvent) => {
    const items = Array.from(e.clipboardData.items);
    const imageItems = items.filter((item) => item.type.startsWith('image/'));
    if (imageItems.length === 0) return;

    e.preventDefault();
    const atts: Attachment[] = [];
    let loaded = 0;
    const total = Math.min(imageItems.length, 5);
    for (let i = 0; i < total; i++) {
      const blob = imageItems[i].getAsFile();
      if (!blob) { loaded++; if (loaded === total) setAttachments(prev => [...prev, ...atts.filter(a => a.data)]); continue; }
      const name = `paste-${Date.now()}-${i}.png`;
      const reader = new FileReader();
      reader.onloadend = () => {
        const dataUrl = reader.result as string;
        if (dataUrl) atts.push({ kind: 'image', name, data: dataUrl, mime: blob.type });
        loaded++;
        if (loaded === total && atts.length > 0) setAttachments(prev => [...prev, ...atts]);
      };
      reader.readAsDataURL(blob);
    }
  }, []);

  // 语音识别：用 ref 保存完整转录，避免重复追加 (Bug #2 fix)
  const voiceTranscriptRef = useRef('');
  const voiceHoldTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const toggleVoice = useCallback(() => {
    const SpeechRecognition = (window as any).SpeechRecognition || (window as any).webkitSpeechRecognition;
    if (!SpeechRecognition) { notifyError('当前浏览器不支持语音输入'); return; }

    if (listening) {
      recognitionRef.current?.stop();
      setListening(false);
      return;
    }

    const rec = new SpeechRecognition();
    rec.lang = 'zh-CN';
    rec.interimResults = true;
    rec.continuous = true;
    recognitionRef.current = rec;
    voiceTranscriptRef.current = '';

    rec.onresult = (e: any) => {
      let transcript = '';
      for (let i = 0; i < e.results.length; i++) {
        transcript += e.results[i][0].transcript;
      }
      setText(transcript);  // Direct replace, not append (Bug #2 fixed)
    };
    rec.onerror = () => { setListening(false); };
    rec.onend = () => { setListening(false); };
    rec.start();
    setListening(true);
  }, [listening]);

  // 快捷提示词模板
  const PROMPT_TEMPLATES = [
    { icon: '📝', label: '翻译', prompt: '请将以下内容翻译成英文：\n' },
    { icon: '💻', label: '写代码', prompt: '请用 {语言} 写一个 {功能}：\n要求：' },
    { icon: '🐛', label: '修Bug', prompt: '这段代码有bug，请帮我修复并解释原因：\n```\n\n```' },
    { icon: '📖', label: '总结', prompt: '请总结以下内容的核心要点：\n' },
    { icon: '✍️', label: '润色', prompt: '请帮我润色以下文字，使其更加流畅专业：\n' },
    { icon: '🎨', label: '文生图', prompt: '/txt2img 生成一张图片：' },
    { icon: '🖌️', label: '图生图', prompt: '/img2img 基于上图修改：' },
    { icon: '🔍', label: '代码审查', prompt: '请审查以下代码的安全性和性能问题：\n```\n\n```' },
    { icon: '📊', label: '数据分析', prompt: '请分析以下数据并给出结论：\n' },
    { icon: '🎤', label: '演讲稿', prompt: '请帮我写一份关于 {主题} 的演讲稿，时长约 {分钟} 分钟。' },
  ];

  const pickFiles = (files: FileList | null) => {
    if (!files) return;
    void (async () => {
      const atts: Attachment[] = [];
      for (const f of Array.from(files).slice(0, 5)) {
        if (f.type.startsWith('image/')) {
          const dataUrl = await new Promise<string>((res) => {
            const r = new FileReader();
            r.onload = () => res(r.result as string);
            r.readAsDataURL(f);
          });
          atts.push({ kind: 'image', name: f.name, data: dataUrl, mime: f.type });
        } else {
          atts.push({ kind: 'file', name: f.name, data: f.name, mime: f.type });
        }
      }
      setAttachments((prev) => [...prev, ...atts]);
    })();
  };

  const doScreenshot = async () => {
    try {
      const b64 = await captureScreen();
      setAttachments((prev) => [...prev, { kind: 'image', name: `截图-${Date.now()}.png`, data: `data:image/png;base64,${b64}`, mime: 'image/png' }]);
    } catch (e) {
      notifyError((e as Error).message);
    }
  };

  const models = allModels();
  const currentModel = models.find((m) => m.id === modelId);

  // 上下文长度使用情况（随消息流式增长实时更新）
  const usedTokens = useMemo(() => {
    let sum = estimateTokens(persona?.systemPrompt ?? '') + estimateTokens(text);
    for (const m of messages) sum += estimateTokens(m.content);
    return sum;
  }, [messages, persona?.systemPrompt, text]);
  const ctxWindow = contextWindowFor(currentModel?.model ?? '');
  const ctxPct = Math.min(100, (usedTokens / ctxWindow) * 100);

  return (
    <div className="shrink-0 px-4 pb-3 pt-1">
      <div className="rounded-2xl border border-border/30 acrylic-card">
        {/* 提示词模板弹出面板 */}
        {showTemplates && (
          <div className="m-3 grid grid-cols-5 gap-1.5 rounded-lg border border-border/50 bg-card p-2">
            {PROMPT_TEMPLATES.map((tpl) => (
              <button
                key={tpl.label}
                className="flex flex-col items-center gap-0.5 rounded-md px-1 py-2 text-[10px] transition-colors hover:bg-accent"
                onClick={() => { setText(prev => prev ? prev + '\n' + tpl.prompt : tpl.prompt); setShowTemplates(false); taRef.current?.focus(); }}
              >
                <span className="text-base">{tpl.icon}</span>
                <span className="text-[10px] text-muted-foreground">{tpl.label}</span>
              </button>
            ))}
          </div>
        )}
        {/* 附件预览条 */}
        {attachments.length > 0 && (
          <div className="flex flex-wrap gap-2 px-3 pt-3">
            {attachments.map((a, i) => (
              <div key={i} className="group relative">
                {a.kind === 'image' ? (
                  <img src={a.data} alt={a.name} className="h-14 w-14 rounded-lg border border-border object-cover" />
                ) : (
                  <div className="flex h-14 items-center rounded-lg border border-border bg-muted px-3 text-xs">📎 {a.name}</div>
                )}
                <button
                  className="absolute -right-1.5 -top-1.5 hidden h-4 w-4 items-center justify-center rounded-full bg-red-500 text-white group-hover:flex"
                  onClick={() => setAttachments((prev) => prev.filter((_, j) => j !== i))}
                >
                  <X className="h-3 w-3" />
                </button>
              </div>
            ))}
          </div>
        )}

        <div className="flex items-end gap-2 p-3">
          {/* 左侧：+ 向上展开工具栏 */}
          <div className="relative flex shrink-0 items-end pb-0.5">
            <input ref={fileRef} type="file" multiple className="hidden" onChange={(e) => pickFiles(e.target.files)} />
            {/* 向上弹出：统一卡片面板 */}
            <div className={cn(
              'absolute bottom-full left-0 mb-2 flex flex-col overflow-hidden rounded-2xl bg-card shadow-lg transition-all duration-300 ease-out animate-pop-in',
              toolsOpen ? 'max-h-56 opacity-100 border border-border/60' : 'max-h-0 opacity-0 pointer-events-none border-0',
            )}>
              <button className="flex items-center gap-3 px-4 py-2.5 text-sm transition-colors hover:bg-accent whitespace-nowrap" onClick={() => { fileRef.current?.click(); setToolsOpen(false); }}>
                <Paperclip className="h-5 w-5" /> 上传文件
              </button>
              <div className="border-t border-border/40" />
              <button className="flex items-center gap-3 px-4 py-2.5 text-sm transition-colors hover:bg-accent whitespace-nowrap" onClick={() => { void doScreenshot(); setToolsOpen(false); }}>
                <Camera className="h-5 w-5" /> 截图
              </button>
              <div className="border-t border-border/40" />
              <button className="flex items-center gap-3 px-4 py-2.5 text-sm transition-colors hover:bg-accent whitespace-nowrap" onClick={() => { setShowTemplates(!showTemplates); setToolsOpen(false); }}>
                <Library className="h-5 w-5" /> 模板
              </button>
            </div>
            {/* + 号主按钮 */}
            <Button
              variant="ghost"
              size="icon"
              className={cn('h-10 w-10 rounded-full transition-all duration-300', toolsOpen && 'rotate-45 bg-accent')}
              title="更多工具"
              onClick={() => setToolsOpen(!toolsOpen)}
            >
              <Plus className="h-5 w-5" />
            </Button>
          </div>

          <textarea
            ref={taRef}
            className={cn(
              'max-h-32 min-h-[38px] flex-1 resize-none bg-transparent py-2 text-sm leading-[22px] outline-none placeholder:text-muted-foreground/60',
              listening && 'text-red-400 placeholder:text-red-400/40',
            )}
            placeholder={listening ? '🎤 正在聆听…松开空格结束' : '输入消息，Enter 发送，Shift+Enter 换行，长按空格语音'}
            value={text}
            rows={1}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === ' ' && !e.nativeEvent.isComposing && taRef.current && taRef.current.selectionStart === taRef.current.selectionEnd && !text.trim() && !listening) {
                e.preventDefault();
                // Long-press hold: 400ms delay before voice activates (Bug #1 fix)
                voiceHoldTimerRef.current = setTimeout(() => toggleVoice(), 400);
                return;
              }
              onKeyDown(e);
            }}
            onKeyUp={(e) => {
              if (e.key === ' ') {
                if (voiceHoldTimerRef.current) { clearTimeout(voiceHoldTimerRef.current); voiceHoldTimerRef.current = null; }
                if (listening) toggleVoice();
              }
            }}
            onPaste={onPaste}
          />

          {/* 右下：暂停 / 发送 */}
          <div className="shrink-0 pb-0.5">
            {generating ? (
              <Button size="icon" variant="destructive" className="h-9 w-9 rounded-full" title="暂停生成" onClick={stopGenerating}>
                <Square className="h-3.5 w-3.5" />
              </Button>
            ) : (
              <Button size="icon" className="h-9 w-9 rounded-full" title="发送" onClick={doSend} disabled={!text.trim() && attachments.length === 0}>
                <SendHorizonal className="h-4 w-4" />
              </Button>
            )}
          </div>
        </div>

        {/* 左下工具栏：模型 + 模式 + TTS朗读 + 权限 + 上下文 */}
        <div className="flex items-center gap-1.5 border-t border-border/60 px-3 py-1.5">
          {/* 模型选择 */}
          <div className="rounded-lg bg-accent/50 px-2 py-1">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
                  {modelId === '__auto__' ? <span className="flex items-center gap-1">🔄 智能路由<RoutedModelHint text={text} /></span> : (currentModel?.label ?? modelId)}
                  <ChevronDown className="h-3 w-3" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent>
                <DropdownMenuItem onClick={() => setModelId('__auto__')}>
                  <span className="flex items-center gap-1.5"><span className="text-xs">🔄</span> 智能路由</span>
                  <span className="ml-2 text-[10px] text-muted-foreground">简单→Flash 复杂→Pro</span>
                  {modelId === '__auto__' && <span className="ml-auto text-xs text-primary">✓</span>}
                </DropdownMenuItem>
                <div className="my-1 border-t border-border/40" />
                {models.map((m) => (
                  <DropdownMenuItem key={m.id} onClick={() => setModelId(m.id)}>
                    {m.label}
                    {!m.builtin && <span className="ml-2 text-[10px] text-muted-foreground">自定义</span>}
                    {m.id === modelId && <span className="ml-auto text-xs text-primary">✓</span>}
                  </DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
          </div>

          {/* 推理模式 */}
          <div className="flex rounded-lg bg-accent/50 p-0.5">
            {MODES.map((m) => (
              <button
                key={m.id}
                title={m.desc}
                className={cn(
                  'rounded-md px-2.5 py-1 text-[11px] transition-colors',
                  mode === m.id ? 'bg-background text-foreground shadow-sm' : 'text-muted-foreground hover:text-foreground',
                )}
                onClick={() => setMode(m.id)}
              >{m.label}</button>
            ))}
          </div>

          {/* AI 朗读开关 */}
          <button
            className={cn(
              'flex items-center gap-1.5 rounded-lg px-2.5 py-1 text-[11px] transition-colors',
              settings.ttsEnabled ? 'bg-primary/15 text-primary' : 'bg-accent/50 text-muted-foreground hover:text-foreground',
            )}
            onClick={() => {
              const next = !useSettingsStore.getState().settings.ttsEnabled;
              void useSettingsStore.getState().update({ ttsEnabled: next });
              // 关闭朗读时立即停止当前正在朗读的内容 (Bug fix)
              if (!next) {
                void import('@/lib/tts').then(({ stopSpeaking }) => stopSpeaking());
              }
            }}
            title={settings.ttsEnabled ? 'AI 朗读已开启' : 'AI 朗读已关闭'}
          >
            {settings.ttsEnabled ? <Volume2 className="h-3.5 w-3.5" /> : <VolumeX className="h-3.5 w-3.5" />}
            朗读
          </button>

          {/* 权限开关 */}
          <PermissionToggle />

          {/* 上下文用量 */}
          <ContextPopover
            usedTokens={usedTokens}
            ctxWindow={ctxWindow}
            ctxPct={ctxPct}
            messages={messages}
            persona={persona}
          />
        </div>
      </div>
    </div>
  );
});

/** 上下文用量悬停面板 — 鼠标悬停显示各部分占比 */
function ContextPopover({ usedTokens, ctxWindow, ctxPct, messages, persona }: {
  usedTokens: number; ctxWindow: number; ctxPct: number;
  messages: ChatMessage[]; persona: Persona | undefined;
}) {
  const sysTokens = estimateTokens(persona?.systemPrompt ?? '');
  const toolDefTokens = 800;
  const msgTokens = messages.reduce((s, m) => s + estimateTokens(m.content), 0);
  const toolResultTokens = messages.filter(m => m.role === 'system').reduce((s, m) => s + estimateTokens(m.content), 0);
  const denom = Math.max(usedTokens, 1);
  const sysPct = ((sysTokens / denom) * 100).toFixed(1);
  const toolDefPct = ((toolDefTokens / denom) * 100).toFixed(1);
  const msgPct = ((msgTokens / denom) * 100).toFixed(1);
  const resultPct = ((toolResultTokens / denom) * 100).toFixed(1);

  return (
    <div className="group relative ml-auto flex items-center gap-1.5">
      <div className="h-1 w-16 overflow-hidden rounded-full bg-muted cursor-help">
        <div className={cn('h-full rounded-full transition-all', ctxPct > 85 ? 'bg-red-500' : ctxPct > 60 ? 'bg-yellow-500' : 'bg-green-500')}
          style={{ width: `${Math.max(2, ctxPct)}%` }} />
      </div>
      <span className={cn('text-[10px]', ctxPct > 85 ? 'text-red-400' : ctxPct > 60 ? 'text-yellow-400' : 'text-muted-foreground')}>
        {formatTokens(usedTokens)}/{formatTokens(ctxWindow)}
      </span>
      <div className="absolute bottom-full right-0 mb-2 hidden w-56 rounded-lg border border-border/50 bg-card p-3 shadow-lg group-hover:block z-50">
        <p className="mb-2 text-[11px] font-medium">会话信息</p>
        <div className="mb-1 flex justify-between text-[10px] text-muted-foreground"><span>{formatTokens(usedTokens)}/{formatTokens(ctxWindow)}</span><span>{ctxPct.toFixed(0)}%</span></div>
        <div className="mb-1.5 h-1.5 w-full overflow-hidden rounded-full bg-muted">
          <div className="h-full rounded-full bg-blue-500/30" style={{ width: `${Math.min(ctxPct + 15, 100)}%` }} />
          <div className="h-full -mt-1.5 rounded-full bg-green-500" style={{ width: `${Math.max(2, ctxPct)}%` }} />
        </div>
        {[{ label: '系统指令', pct: sysPct, tokens: sysTokens },{ label: '工具定义', pct: toolDefPct, tokens: toolDefTokens },{ label: '消息', pct: msgPct, tokens: msgTokens },{ label: '工具结果', pct: resultPct, tokens: toolResultTokens }].map(row => (
          <div key={row.label} className="mb-1 flex items-center justify-between text-[10px]">
            <span className="text-muted-foreground">{row.label}</span>
            <div className="flex items-center gap-1.5">
              <div className="h-1 w-12 overflow-hidden rounded-full bg-muted"><div className="h-full rounded-full bg-primary/50" style={{ width: `${Math.min(parseFloat(row.pct), 100)}%` }} /></div>
              <span className="w-8 text-right">{row.pct}%</span>
            </div>
          </div>
        ))}
        {ctxPct > 80 && <p className="mt-2 text-[10px] text-red-400">⚠️ 上下文接近上限，建议新对话或压缩历史</p>}
      </div>
    </div>
  );
}

/** 实时路由预览：根据输入框文字显示将使用的模型 */
function RoutedModelHint({ text }: { text: string }) {
  const { routeModel } = useSettingsStore();
  if (!text.trim()) return null;
  const model = routeModel(text);
  const label = model.id === 'deepseek-v4-flash' ? '⚡Flash' : '🧠Pro';
  const color = model.id === 'deepseek-v4-flash' ? 'text-green-400' : 'text-amber-400';
  return <span className={`ml-0.5 text-[10px] ${color}`}>→{label}</span>;
}

/** 权限小开关 —— 放在深度思考右边，一条紧凑按钮 */
function PermissionToggle() {
  const { settings, update } = useSettingsStore();
  const isAllowAll = settings.permissionMode === 'allow_all';
  return (
    <button
      onClick={() => void update({ permissionMode: isAllowAll ? 'confirm_each' : 'allow_all' })}
      title={isAllowAll ? '所有操作自动执行（点击切换为每次确认）' : '敏感操作需确认（点击切换为全部允许）'}
      className={`flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] transition-colors ${
        isAllowAll ? 'bg-amber-500/10 text-amber-400 hover:bg-amber-500/20' : 'bg-green-500/10 text-green-400 hover:bg-green-500/20'
      }`}
    >
      {isAllowAll ? <ShieldCheck className="h-3 w-3" /> : <Shield className="h-3 w-3" />}
      {isAllowAll ? '全部允许' : '每次确认'}
    </button>
  );
}
