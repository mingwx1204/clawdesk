/**
 * ClawDesk 媒体生成引擎
 * 统一接入：ComfyUI 本地 + Stability AI + Replicate
 * 支持：文生图 / 图生图 / 文生视频 / 图生视频
 */
import type { Attachment } from '@/types';

// ─── 生成配置 ───
export interface ImageGenConfig {
  provider: 'pollinations' | 'comfyui' | 'stability' | 'replicate';
  comfyuiUrl: string;
  stabilityKey: string;
  replicateKey: string;
  defaultWidth: number;
  defaultHeight: number;
  defaultSteps: number;
  defaultCfg: number;
}

// ─── 生成参数 ───
export interface GenParams {
  prompt: string;
  negativePrompt?: string;
  width?: number;
  height?: number;
  steps?: number;
  cfg?: number;
  seed?: number;
  /** 图生图/图生视频的输入图 */
  initImage?: string; // base64 data URL
  /** 文生视频/图生视频的帧数 */
  numFrames?: number;
  /** 视频帧率 */
  fps?: number;
}

export interface GenResult {
  images: string[];   // base64 data URLs
  videoUrl?: string;
  seed: number;
  provider: string;
}

// ─── ComfyUI 工作流 ───

/** 通过 ComfyUI API 提交 prompt 并等待结果 */
async function comfyuiGenerate(workflow: Record<string, any>, serverUrl: string): Promise<string[]> {
  const base = serverUrl.replace(/\/$/, '');
  try {
    // 1. 提交工作流
    const promptResp = await fetch(`${base}/prompt`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ prompt: workflow }),
      signal: AbortSignal.timeout(120000),
    });
    if (!promptResp.ok) throw new Error(`ComfyUI prompt failed: ${promptResp.status}`);
    const { prompt_id } = await promptResp.json() as { prompt_id: string };

    // 2. 轮询结果
    const outputs: string[] = [];
    let retries = 60; // 最多等 2 分钟
    while (retries-- > 0) {
      await sleep(2000);
      const histResp = await fetch(`${base}/history/${prompt_id}`);
      if (!histResp.ok) continue;
      const hist = await histResp.json() as Record<string, any>;
      const entry = hist[prompt_id];
      if (!entry?.outputs) continue;

      for (const nodeId of Object.keys(entry.outputs)) {
        const out = entry.outputs[nodeId];
        if (out.images) {
          for (const img of out.images) {
            const imgResp = await fetch(`${base}/view?filename=${img.filename}&type=${img.type}`);
            if (imgResp.ok) {
              const buf = await imgResp.arrayBuffer();
              const b64 = btoa(String.fromCharCode(...new Uint8Array(buf)));
              outputs.push(`data:image/png;base64,${b64}`);
            }
          }
        }
      }
      if (outputs.length > 0) return outputs;
    }
    throw new Error('ComfyUI 生成超时');
  } catch (e: any) {
    throw new Error(`ComfyUI: ${e.message}`);
  }
}

/** 构建 ComfyUI 文生图工作流 */
function buildTxt2ImgWorkflow(params: GenParams): Record<string, any> {
  return {
    "3": { "class_type": "KSampler", "inputs": { "seed": params.seed ?? Math.floor(Math.random() * 1e9), "steps": params.steps ?? 20, "cfg": params.cfg ?? 7, "sampler_name": "euler", "scheduler": "normal", "denoise": 1, "model": ["4", 0], "positive": ["6", 0], "negative": ["7", 0], "latent_image": ["5", 0] } },
    "4": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": "sd_xl_base_1.0.safetensors" } },
    "5": { "class_type": "EmptyLatentImage", "inputs": { "width": params.width ?? 1024, "height": params.height ?? 1024, "batch_size": 1 } },
    "6": { "class_type": "CLIPTextEncode", "inputs": { "text": params.prompt, "clip": ["4", 1] } },
    "7": { "class_type": "CLIPTextEncode", "inputs": { "text": params.negativePrompt ?? "ugly, blurry, low quality", "clip": ["4", 1] } },
    "8": { "class_type": "VAEDecode", "inputs": { "samples": ["3", 0], "vae": ["4", 2] } },
    "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": "ClawDesk", "images": ["8", 0] } },
  };
}

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

// ─── 公开 API ───

/** 文生图 */
export async function textToImage(config: ImageGenConfig, params: GenParams): Promise<GenResult> {
  // 1. Pollinations.ai — 免费优先
  if (config.provider === 'pollinations') {
    return pollinationsGenerate(params);
  }
  // 2. ComfyUI 本地
  if (config.provider === 'comfyui' && config.comfyuiUrl) {
    const wf = buildTxt2ImgWorkflow(params);
    const images = await comfyuiGenerate(wf, config.comfyuiUrl);
    return { images, seed: 0, provider: 'comfyui' };
  }
  // 3. ComfyUI 不可用时回退到 Pollinations
  try {
    return await pollinationsGenerate(params);
  } catch {
    throw new Error('请配置 ComfyUI 地址或 API Key');
  }
}

/** Pollinations.ai 免费生成 — 返回直接 URL（浏览器跨域加载图片无需 fetch） */
async function pollinationsGenerate(params: GenParams): Promise<GenResult> {
  const encoded = encodeURIComponent(params.prompt);
  const seed = params.seed ?? Math.floor(Math.random() * 1e9);
  const w = params.width ?? 1024;
  const h = params.height ?? 1024;
  // 直接返回图片 URL — 浏览器用 <img> 加载不受 CORS 限制
  const url = `https://image.pollinations.ai/prompt/${encoded}?width=${w}&height=${h}&seed=${seed}&nologo=true`;
  return { images: [url], seed, provider: 'pollinations' };
}

/** 图生图 */
export async function imageToImage(config: ImageGenConfig, params: GenParams): Promise<GenResult> {
  if (!params.initImage) throw new Error('图生图需要 initImage');
  // TODO: ComfyUI img2img workflow
  throw new Error('图生图功能开发中');
}

/** 文生视频 */
export async function textToVideo(config: ImageGenConfig, params: GenParams): Promise<GenResult> {
  // TODO: Replicate / ComfyUI AnimateDiff
  throw new Error('文生视频功能开发中');
}

/** 图生视频 */
export async function imageToVideo(config: ImageGenConfig, params: GenParams): Promise<GenResult> {
  if (!params.initImage) throw new Error('图生视频需要 initImage');
  // TODO: Replicate / ComfyUI AnimateDiff
  throw new Error('图生视频功能开发中');
}

/** 检测 ComfyUI 是否可用 */
export async function checkComfyUI(url: string): Promise<boolean> {
  try {
    const resp = await fetch(`${url.replace(/\/$/, '')}/system_stats`, { signal: AbortSignal.timeout(3000) });
    return resp.ok;
  } catch { return false; }
}

/** 获取 ComfyUI 可用模型 */
export async function listComfyUIModels(url: string): Promise<string[]> {
  try {
    const resp = await fetch(`${url.replace(/\/$/, '')}/object_info`, { signal: AbortSignal.timeout(5000) });
    if (!resp.ok) return [];
    const info = await resp.json() as Record<string, any>;
    const checkpoints = info.CheckpointLoaderSimple?.input?.required?.ckpt_name?.[0] ?? [];
    return checkpoints as string[];
  } catch { return []; }
}
