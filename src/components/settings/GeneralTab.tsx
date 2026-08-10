import { useEffect, useState } from 'react';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import { Slider } from '@/components/ui/slider';
import { Input } from '@/components/ui/input';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { useSettingsStore } from '@/store/useSettingsStore';
import { listVoices, setVoice, type TtsVoice } from '@/lib/tts';

export function GeneralTab() {
  const { settings, update } = useSettingsStore();
  const [voices, setVoices] = useState<TtsVoice[]>([]);

  // 加载系统可用音色
  useEffect(() => {
    void listVoices().then((v) => setVoices(v)).catch(() => {});
  }, []);

  // 应用启动时同步已保存音色到 TTS
  useEffect(() => {
    if (settings.ttsVoice) void setVoice(settings.ttsVoice);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <div className="space-y-5">
      <div className="space-y-2">
        <Label>主题</Label>
        <Select value={settings.theme} onValueChange={(v) => void update({ theme: v as 'dark' | 'light' | 'system' })}>
          <SelectTrigger className="w-48"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="dark">暗色（默认）</SelectItem>
            <SelectItem value="light">亮色</SelectItem>
            <SelectItem value="system">跟随系统</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <div className="space-y-2">
        <Label>字体大小：{settings.fontSize}px</Label>
        <Slider
          className="w-64" min={12} max={18} step={1}
          value={[settings.fontSize]}
          onValueChange={([v]) => void update({ fontSize: v })}
        />
      </div>
      <div className="space-y-2">
        <Label>主题色</Label>
        <input
          type="color"
          value={(() => {
            // Convert HSL to Hex for HTML color input (Bug #22 fix)
            const h = settings.accentHue / 60;
            const c = 0.7 * 0.5; // saturation * lightness
            const x = c * (1 - Math.abs((h % 2) - 1));
            const m = 0.5 - c;
            let r = 0, g = 0, b = 0;
            if (h < 1) { r = c; g = x; }
            else if (h < 2) { r = x; g = c; }
            else if (h < 3) { g = c; b = x; }
            else if (h < 4) { g = x; b = c; }
            else if (h < 5) { r = x; b = c; }
            else { r = c; b = x; }
            const toHex = (v: number) => Math.round((v + m) * 255).toString(16).padStart(2, '0');
            return `#${toHex(r)}${toHex(g)}${toHex(b)}`;
          })()}
          onChange={(e) => {
            const hex = e.target.value;
            const r = parseInt(hex.slice(1,3), 16);
            const g = parseInt(hex.slice(3,5), 16);
            const b = parseInt(hex.slice(5,7), 16);
            const max = Math.max(r,g,b), min = Math.min(r,g,b);
            let h = 0;
            if (max !== min) {
              const d = max - min;
              if (max === r) h = ((g - b) / d + (g < b ? 6 : 0)) * 60;
              else if (max === g) h = ((b - r) / d + 2) * 60;
              else h = ((r - g) / d + 4) * 60;
            }
            void update({ accentHue: Math.round(h) });
          }}
          className="h-9 w-12 cursor-pointer rounded border border-border bg-transparent p-0.5"
        />
      </div>
      <div className="space-y-2">
        <Label>自定义背景</Label>
        <div className="flex flex-wrap gap-2 mb-2">
          <label className="flex h-10 w-24 cursor-pointer items-center justify-center rounded-lg border border-dashed border-border text-xs text-muted-foreground hover:border-primary/50 transition-all">
            📁 选择背景图片
            <input
              type="file"
              accept="image/*"
              className="hidden"
              onChange={(e) => {
                const f = e.target.files?.[0];
                if (!f) return;
                const r = new FileReader();
                r.onload = () => void update({ customBackground: r.result as string });
                r.readAsDataURL(f);
              }}
            />
          </label>
          {settings.customBackground && (
            <button
              className="h-10 rounded-lg border border-border px-3 text-[10px] text-muted-foreground hover:border-red-500/50 hover:text-red-400 transition-all"
              onClick={() => void update({ customBackground: '' })}
              title="移除自定义背景"
            >
              ✕ 移除背景
            </button>
          )}
        </div>
        <Input
          placeholder="https://... 或留空使用默认"
          value={settings.customBackground}
          onChange={(e) => void update({ customBackground: e.target.value })}
        />
      </div>
      <div className="space-y-2">
        <Label>背景不透明度：{settings.backgroundOpacity}%</Label>
        <Slider
          className="w-64" min={20} max={100} step={5}
          value={[settings.backgroundOpacity]}
          onValueChange={([v]) => void update({ backgroundOpacity: v })}
        />
      </div>
      {([  
        ['autoStart', '开机自启'],
        ['alwaysOnTop', '窗口置顶'],
        ['closeToTray', '关闭时最小化到托盘'],
        ['soundEnabled', '音效（按键/打字/输出）'],
        ['ttsEnabled', 'AI 朗读（输出完自动朗读，默认开启）'],
        ['autoSaveChat', '对话自动保存'],
        ['autoEvolve', '自动进化（对话后提取经验，消耗API）'],
      ] as const).map(([key, label]) => (
        <div key={key} className="flex items-center justify-between">
          <Label>{label}</Label>
          <Switch checked={settings[key as keyof typeof settings] as boolean} onCheckedChange={(v) => void update({ [key]: v })} />
        </div>
      ))}
      {settings.ttsEnabled && (
        <div className="space-y-2">
          <Label>朗读音色</Label>
          <Select
            value={settings.ttsVoice || '__auto__'}
            onValueChange={(v) => {
              const name = v === '__auto__' ? '' : v;
              void update({ ttsVoice: name });
              void setVoice(name);
            }}
          >
            <SelectTrigger className="w-64"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="__auto__">🤖 自动（中文优先）</SelectItem>
              {voices.map((v) => (
                <SelectItem key={v.name} value={v.name}>
                  {v.name.replace(/^Microsoft\s*/i, '')}{v.lang ? ` (${v.lang})` : ''}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {voices.length === 0 && (
            <p className="text-xs text-muted-foreground">未检测到可用音色（Windows 语音设置中可添加）</p>
          )}
        </div>
      )}
      {settings.autoSaveChat && (
        <div className="flex items-center gap-2">
          <Label className="shrink-0 text-xs">保存路径</Label>
          <Input
            className="h-7 text-xs"
            value={settings.savePath}
            onChange={(e) => void update({ savePath: e.target.value })}
            placeholder="D:\数据库"
          />
        </div>
      )}
    </div>
  );
}
