/**
 * 媒体生成设置 — 文生图 / 图生图 / 文生视频 / 图生视频
 * 支持：ComfyUI 本地 / Stability AI / Replicate
 */
import { useState, useEffect } from 'react';
import { Image, Video, Check, Loader2, Zap, ExternalLink } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import { Badge } from '@/components/ui/badge';
import { useSettingsStore } from '@/store/useSettingsStore';
import { notifySuccess, notifyError } from '@/lib/notify';
import { checkComfyUI, listComfyUIModels } from '@/lib/mediaGen';

export function MediaGenTab() {
  const { settings, update } = useSettingsStore();
  const { mediaGen } = settings;
  const [comfyuiUrl, setComfyuiUrl] = useState(mediaGen.comfyuiUrl);
  const [stabilityKey, setStabilityKey] = useState(mediaGen.stabilityKey);
  const [replicateKey, setReplicateKey] = useState(mediaGen.replicateKey);
  const [provider, setProvider] = useState(mediaGen.provider);
  const [checking, setChecking] = useState(false);
  const [comfyStatus, setComfyStatus] = useState<'idle' | 'ok' | 'fail'>('idle');
  const [models, setModels] = useState<string[]>([]);

  const checkComfy = async () => {
    setChecking(true);
    const ok = await checkComfyUI(comfyuiUrl);
    setComfyStatus(ok ? 'ok' : 'fail');
    if (ok) {
      const mdl = await listComfyUIModels(comfyuiUrl);
      setModels(mdl);
      notifySuccess(`ComfyUI 已连接，${mdl.length} 个模型可用`);
    } else {
      notifyError('无法连接 ComfyUI，请确认已启动');
    }
    setChecking(false);
  };

  const save = () => {
    update({
      mediaGen: {
        ...mediaGen,
        provider,
        comfyuiUrl,
        stabilityKey,
        replicateKey,
      },
    });
    notifySuccess('媒体生成配置已保存');
  };

  return (
    <div className="space-y-5">
      <div className="rounded-xl border-2 border-border bg-card p-4">
        <div className="flex items-center gap-3">
          <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-purple-500/20">
            <Image className="h-6 w-6 text-purple-400" />
          </div>
          <div>
            <p className="font-semibold">🎨 媒体生成引擎</p>
            <p className="text-xs text-muted-foreground">文生图 · 图生图 · 文生视频 · 图生视频</p>
          </div>
        </div>

        <div className="mt-4 space-y-3">
          <div className="flex items-center justify-between">
            <Label>引擎选择</Label>
            <div className="flex gap-1 rounded-lg bg-muted p-0.5 text-xs">
              {(['pollinations', 'comfyui', 'stability', 'replicate'] as const).map((p) => (
                <button
                  key={p}
                  className={`rounded-md px-3 py-1 ${provider === p ? 'bg-background shadow-sm text-foreground' : 'text-muted-foreground'}`}
                  onClick={() => setProvider(p)}
                >
                  {p === 'pollinations' ? '🌐 免费' : p === 'comfyui' ? '🖥️ ComfyUI' : p === 'stability' ? '☁️ Stability' : '☁️ Replicate'}
                </button>
              ))}
            </div>
          </div>

          {provider === 'comfyui' && (
            <div className="space-y-3 rounded-lg border border-border/50 bg-accent/20 p-3">
              <div className="flex items-end gap-2">
                <div className="flex-1 space-y-1">
                  <Label className="text-xs">ComfyUI 地址</Label>
                  <Input value={comfyuiUrl} onChange={(e) => setComfyuiUrl(e.target.value)} placeholder="http://127.0.0.1:8188" />
                </div>
                <Button variant="outline" size="sm" disabled={checking} onClick={checkComfy}>
                  {checking ? <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" /> : null}
                  {comfyStatus === 'ok' ? '✅ 已连接' : '检测连接'}
                </Button>
              </div>
              {models.length > 0 && (
                <div className="text-[11px] text-muted-foreground">
                  可用模型: {models.slice(0, 5).join(', ')}{models.length > 5 ? ` ...等${models.length}个` : ''}
                </div>
              )}
              {comfyStatus === 'fail' && (
                <p className="text-[11px] text-amber-400">
                  ⚠️ 请确认 ComfyUI 已启动（通常运行 run_nvidia_gpu.bat 或 run_cpu.bat）
                </p>
              )}
            </div>
          )}

          {provider === 'stability' && (
            <div className="space-y-1">
              <Label className="text-xs">Stability AI API Key</Label>
              <Input type="password" value={stabilityKey} onChange={(e) => setStabilityKey(e.target.value)} placeholder="sk-..." />
            </div>
          )}

          {provider === 'replicate' && (
            <div className="space-y-1">
              <Label className="text-xs">Replicate API Key</Label>
              <Input type="password" value={replicateKey} onChange={(e) => setReplicateKey(e.target.value)} placeholder="r8_..." />
            </div>
          )}

          <Button variant="secondary" size="sm" onClick={save}>保存配置</Button>
        </div>
      </div>

      {/* 使用说明 */}
      <div className="rounded-xl border border-dashed border-border p-3 text-xs text-muted-foreground">
        <p className="mb-2 font-semibold text-foreground">💡 在对话中使用</p>
        <div className="space-y-1">
          <p>• <code className="rounded bg-muted px-1">/txt2img 一只在月球上散步的猫</code> — 文生图</p>
          <p>• <code className="rounded bg-muted px-1">/img2img 把背景改成赛博朋克风格</code> — 需要先上传图片</p>
          <p>• <code className="rounded bg-muted px-1">/txt2video 海边日落延时摄影</code> — 文生视频</p>
          <p>• <code className="rounded bg-muted px-1">/img2video 让这张图动起来</code> — 需要先上传图片</p>
          <p className="mt-2 text-[10px]">提示：AI 会自动将 /txt2img 转换为 tool:text2img 调用，引擎设置需先保存。</p>
        </div>
      </div>
    </div>
  );
}
