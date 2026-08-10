import { useState, useEffect } from 'react';
import { Plus, Trash2, Check, Loader2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Slider } from '@/components/ui/slider';
import { Switch } from '@/components/ui/switch';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { useSettingsStore, BUILTIN_MODELS } from '@/store/useSettingsStore';
import { BalanceChecker, uid } from './BalanceChecker';
import { notifySuccess, notifyError } from '@/lib/notify';
import type { CustomModel, ReasoningMode } from '@/types';

export function ModelTab() {
  const { settings, update, allModels } = useSettingsStore();
  const [newModel, setNewModel] = useState({ label: '', apiBase: '', apiKey: '', model: '' });

  const addCustomModel = () => {
    const cleaned = {
      label: newModel.label.trim(),
      apiBase: newModel.apiBase.trim(),
      apiKey: newModel.apiKey.trim(),
      model: newModel.model.trim(),
    };
    if (!cleaned.label || !cleaned.apiBase || !cleaned.model) return;
    const m: CustomModel = { id: `custom-${uid()}`, builtin: false, ...cleaned };
    void update({ customModels: [...settings.customModels, m] });
    setNewModel({ label: '', apiBase: '', apiKey: '', model: '' });
  };

  return (
    <div className="space-y-6">
      <div className="grid grid-cols-2 gap-4">
        <div className="space-y-2">
          <Label>默认模型</Label>
          <Select value={settings.defaultModelId} onValueChange={(v) => void update({ defaultModelId: v })}>
            <SelectTrigger><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="__auto__">🔄 智能路由（简单→Flash 复杂→Pro）</SelectItem>
              {allModels().map((m) => <SelectItem key={m.id} value={m.id}>{m.label}</SelectItem>)}
            </SelectContent>
          </Select>
        </div>
        <div className="space-y-2">
          <Label>默认推理模式</Label>
          <Select value={settings.defaultMode} onValueChange={(v) => void update({ defaultMode: v as ReasoningMode })}>
            <SelectTrigger><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="fast">快速</SelectItem>
              <SelectItem value="standard">标准</SelectItem>
              <SelectItem value="deep">深度思考</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      <div className="space-y-3">
        <Label>API 密钥（内置模型）</Label>
        {BUILTIN_MODELS.map((m) => (
          <div key={m.id} className="flex items-center gap-2">
            <span className="w-28 shrink-0 text-sm">{m.label}</span>
            <Input
              type="text"
              autoComplete="off"
              className="font-mono text-xs"
              placeholder="sk-…"
              value={settings.apiKeys[m.id] ?? ''}
              onChange={(e) => void update({ apiKeys: { ...settings.apiKeys, [m.id]: e.target.value } })}
            />
            <BalanceChecker apiBase={m.apiBase} apiKey={settings.apiKeys[m.id] ?? ''} />
          </div>
        ))}
      </div>

      <div className="space-y-3">
        <Label>高级参数</Label>
        <div className="space-y-4">
          {([
            {
              key: 'temperature' as const,
              label: 'temperature（随机性）',
              value: settings.modelParams.temperature,
              min: 0, max: 2, step: 0.05,
              desc: '越低回答越确定保守，越高越有创意随机。代码/事实类建议 0~0.3，写作建议 0.7~1.0。思考模式下不生效。',
            },
            {
              key: 'maxTokens' as const,
              label: 'max_tokens（最大输出长度）',
              value: settings.modelParams.maxTokens,
              min: 256, max: 393216, step: 8192,
              desc: '单次回复最多生成多少 token。1 中文 ≈ 0.6 token，384K 约 64 万字。DeepSeek 上限 393216。',
            },
            {
              key: 'topP' as const,
              label: 'top_p（核采样）',
              value: settings.modelParams.topP,
              min: 0, max: 1, step: 0.05,
              desc: '控制词汇选择范围，值越小选词越集中保守。通常与 temperature 配合使用，建议 0.9。思考模式下不生效。',
            },
          ]).map(({ key, label, value, min, max, step, desc }) => (
            <div key={key} className="space-y-1.5">
              <div className="flex items-center gap-2 text-sm">
                <span className="shrink-0">{label}</span>
                <input
                  type="number"
                  className="h-6 w-20 rounded border border-border bg-muted px-1.5 text-xs text-center"
                  min={min} max={max} step={step}
                  value={value}
                  onChange={(e) => {
                    const v = parseFloat(e.target.value);
                    if (!isNaN(v)) {
                      void update({ modelParams: { ...settings.modelParams, [key]: Math.min(max, Math.max(min, v)) } });
                    }
                  }}
                />
                <span className="shrink-0 text-[10px] text-muted-foreground">{min} – {max}</span>
              </div>
              <Slider className="w-full" min={min} max={max} step={step} value={[value]}
                onValueChange={([v]) => void update({ modelParams: { ...settings.modelParams, [key]: v } })} />
              <p className="text-[11px] leading-relaxed text-muted-foreground">{desc}</p>
            </div>
          ))}
        </div>
      </div>

      <div className="space-y-3">
        <Label>自定义模型</Label>
        {settings.customModels.map((m) => (
          <div key={m.id} className="flex items-center gap-2 rounded-lg border border-border p-2 text-sm">
            <span className="flex-1">{m.label} <span className="text-xs text-muted-foreground">({m.model})</span></span>
            <BalanceChecker apiBase={m.apiBase} apiKey={m.apiKey} />
            <Button variant="ghost" size="icon" className="h-7 w-7 text-red-500"
              onClick={() => void update({ customModels: settings.customModels.filter((x) => x.id !== m.id) })}>
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </div>
        ))}
        <div className="grid grid-cols-2 gap-2 rounded-lg border border-dashed border-border p-3">
          <Input placeholder="显示名称" value={newModel.label} onChange={(e) => setNewModel({ ...newModel, label: e.target.value })} />
          <Input placeholder="模型名" value={newModel.model} onChange={(e) => setNewModel({ ...newModel, model: e.target.value })} />
          <Input placeholder="API 地址" value={newModel.apiBase} onChange={(e) => setNewModel({ ...newModel, apiBase: e.target.value })} />
          <Input placeholder="API Key" type="text" autoComplete="off" className="font-mono text-xs" value={newModel.apiKey} onChange={(e) => setNewModel({ ...newModel, apiKey: e.target.value })} />
          <Button className="col-span-2" variant="secondary" onClick={addCustomModel}>
            <Plus className="mr-1 h-4 w-4" /> 添加自定义模型
          </Button>
        </div>
      </div>

      {/* Ollama 本地模型 */}
      <div className="space-y-3 rounded-xl border border-border bg-card p-4">
        <div className="flex items-center justify-between">
          <div>
            <Label className="text-sm">🦙 Ollama 本地模型</Label>
            <p className="text-[11px] text-muted-foreground">离线运行，数据不离开本机</p>
          </div>
          <Switch
            checked={settings.ollama.enabled}
            onCheckedChange={(v) => update({ ollama: { ...settings.ollama, enabled: v } })}
          />
        </div>
        {settings.ollama.enabled && (
          <div className="space-y-2">
            <div className="flex items-end gap-2">
              <div className="flex-1 space-y-1">
                <Label className="text-xs">Ollama 地址</Label>
                <Input value={settings.ollama.baseUrl}
                  onChange={(e) => update({ ollama: { ...settings.ollama, baseUrl: e.target.value } })}
                  placeholder="http://127.0.0.1:11434" />
              </div>
              <Button variant="outline" size="sm" onClick={async () => {
                try {
                  const resp = await fetch(`${settings.ollama.baseUrl}/api/tags`);
                  if (resp.ok) { const data = await resp.json() as any; notifySuccess(`Ollama 已连接，${data.models?.length || 0} 个模型`); }
                } catch { notifyError('无法连接 Ollama'); }
              }}>检测</Button>
            </div>
            <div className="space-y-1">
              <Label className="text-xs">默认模型名（如 qwen2.5:7b, llama3.1:8b）</Label>
              <Input value={settings.ollama.defaultModel}
                onChange={(e) => update({ ollama: { ...settings.ollama, defaultModel: e.target.value } })}
                placeholder="qwen2.5:7b" />
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
